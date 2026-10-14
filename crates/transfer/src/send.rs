//! Sender side of a bulk connection: serve one dropped item.

use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use bytes::BytesMut;
use osm_protocol::v1::bulk_frame::Body;
use osm_protocol::v1::{
    BulkFrame, EntryKind, FetchRequest, FileChunk, FileEnd, ManifestEnd, ManifestEntry,
    ManifestPage, TransferDone,
};
use osm_protocol::{CHUNK_SIZE, read_bulk, write_bulk};
use sha2::{Digest, Sha256};

use crate::error::TransferError;
use crate::manifest::{map_io, scan};
use crate::progress::{Phase, Progress};

/// Approximate encoded bytes per manifest page; well under `MAX_BULK_FRAME`
/// even for entries with maximal paths.
const PAGE_BUDGET: usize = 256 * 1024;

/// Reads the `FetchRequest` that opens a bulk connection.
pub fn read_request(stream: &mut impl Read) -> Result<FetchRequest, TransferError> {
    match read_bulk(stream, &mut Vec::new())? {
        Body::Fetch(f) => Ok(f),
        _ => Err(TransferError::Protocol("连接未以文件请求开始".into())),
    }
}

/// Streams `root` to the receiver. On any error after the request, a
/// `TransferFailed` is sent when the connection still allows it.
pub fn serve(
    stream: &mut impl Write,
    root: &Path,
    progress: &Progress,
) -> Result<(), TransferError> {
    let mut scratch = BytesMut::new();
    let result = serve_inner(stream, root, progress, &mut scratch);
    match &result {
        Ok(()) => progress.set_phase(Phase::Finished),
        Err(TransferError::Io(_)) | Err(TransferError::Cancelled) => {}
        Err(e) => {
            let _ = write_bulk(stream, &frame(Body::Failed(e.to_wire())), &mut scratch);
        }
    }
    result
}

/// Answers a request this side cannot serve (unknown drag, bad token).
pub fn refuse(stream: &mut impl Write, err: &TransferError) {
    let _ = write_bulk(
        stream,
        &frame(Body::Failed(err.to_wire())),
        &mut BytesMut::new(),
    );
}

fn frame(body: Body) -> BulkFrame {
    BulkFrame { body: Some(body) }
}

fn serve_inner(
    stream: &mut impl Write,
    root: &Path,
    progress: &Progress,
    scratch: &mut BytesMut,
) -> Result<(), TransferError> {
    progress.set_phase(Phase::Scanning);
    let entries = scan(root)?;
    let base = root.parent().ok_or(TransferError::NotFound)?.to_path_buf();

    let mut total: u64 = 0;
    let mut files: u64 = 0;
    for e in &entries {
        if e.kind == EntryKind::File as i32 {
            total = total
                .checked_add(e.size)
                .ok_or_else(|| TransferError::Limit("总大小溢出".into()))?;
            files += 1;
        }
    }
    progress.set_totals(total, files);

    let mut page = Vec::new();
    let mut budget = 0usize;
    for e in &entries {
        budget += e.path.len() + e.link_target.len() + 48;
        page.push(e.clone());
        if budget >= PAGE_BUDGET {
            let entries = std::mem::take(&mut page);
            write_bulk(
                stream,
                &frame(Body::Manifest(ManifestPage { entries })),
                scratch,
            )?;
            budget = 0;
        }
    }
    if !page.is_empty() {
        write_bulk(
            stream,
            &frame(Body::Manifest(ManifestPage { entries: page })),
            scratch,
        )?;
    }
    write_bulk(
        stream,
        &frame(Body::ManifestEnd(ManifestEnd {
            entry_count: entries.len() as u64,
            total_bytes: total,
        })),
        scratch,
    )?;

    progress.set_phase(Phase::Transferring);
    let mut chunk = frame(Body::Chunk(FileChunk {
        data: Vec::with_capacity(CHUNK_SIZE),
    }));
    for e in entries.iter().filter(|e| e.kind == EntryKind::File as i32) {
        send_file(stream, &base, e, &mut chunk, progress, scratch)?;
        progress.file_done();
    }
    write_bulk(stream, &frame(Body::Done(TransferDone {})), scratch)?;
    stream.flush()?;
    Ok(())
}

fn send_file(
    stream: &mut impl Write,
    base: &Path,
    entry: &ManifestEntry,
    chunk: &mut BulkFrame,
    progress: &Progress,
    scratch: &mut BytesMut,
) -> Result<(), TransferError> {
    let path: PathBuf = base.join(&entry.path);
    let mut file = crate::fsx::open_source(&path).map_err(|e| map_io(e, &entry.path))?;
    let changed = || TransferError::SourceChanged(entry.path.clone());
    let unchanged = |m: &std::fs::Metadata| {
        m.is_file()
            && m.len() == entry.size
            && m.mtime() == entry.mtime_sec
            && m.mtime_nsec().clamp(0, 999_999_999) as u32 == entry.mtime_nsec
    };
    if !unchanged(&file.metadata()?) {
        return Err(changed());
    }

    let mut hasher = Sha256::new();
    let mut sent: u64 = 0;
    loop {
        if progress.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let Some(Body::Chunk(c)) = &mut chunk.body else {
            unreachable!("chunk frame holds a chunk")
        };
        c.data.resize(CHUNK_SIZE, 0);
        let n = read_full(&mut file, &mut c.data)?;
        c.data.truncate(n);
        if n == 0 {
            break;
        }
        sent += n as u64;
        if sent > entry.size {
            return Err(changed());
        }
        hasher.update(&c.data);
        write_bulk(stream, chunk, scratch)?;
        progress.add_bytes(n as u64);
    }
    if sent != entry.size || !unchanged(&file.metadata()?) {
        return Err(changed());
    }
    write_bulk(
        stream,
        &frame(Body::FileEnd(FileEnd {
            sha256: hasher.finalize().to_vec(),
        })),
        scratch,
    )?;
    Ok(())
}

/// Fills `buf` unless the file ends first; returns the bytes read.
fn read_full(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}
