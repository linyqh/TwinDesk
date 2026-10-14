//! Receiver side of a bulk connection: fetch one dropped item into a
//! destination directory.
//!
//! Everything lands in a hidden staging directory inside the destination
//! (same volume, so the final rename is atomic) and only the verified item
//! is renamed into place. A failed or cancelled transfer leaves nothing
//! visible in the destination (report §10.1, §10.3).

use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use bytes::BytesMut;
use osm_protocol::v1::bulk_frame::Body;
use osm_protocol::v1::{BulkFrame, EntryKind, FetchRequest, ManifestEntry};
use osm_protocol::{read_bulk, write_bulk};
use sha2::{Digest, Sha256};

use crate::error::TransferError;
use crate::fsx;
use crate::manifest::{MAX_ENTRIES, MAX_PATH_BYTES};
use crate::path::validate_macos_path;
use crate::progress::{Phase, Progress};

/// Free space kept beyond the payload so a copy never fills the disk
/// completely (D).
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;
const STAGING_PREFIX: &str = ".osm-staging-";
/// Attempts at "name 2", "name 3", … before giving up.
const MAX_RENAME_ATTEMPTS: u32 = 999;

/// Sends the request that opens a bulk connection.
pub fn send_request(stream: &mut impl Write, req: FetchRequest) -> Result<(), TransferError> {
    write_bulk(
        stream,
        &BulkFrame {
            body: Some(Body::Fetch(req)),
        },
        &mut BytesMut::new(),
    )?;
    Ok(())
}

/// Receives the offered item `name` into `dest_dir` as `dest_name` and
/// returns where it was committed. The name gets a " 2"-style suffix if
/// `dest_dir` already has an item by that name; nothing existing is ever
/// replaced.
pub fn receive(
    stream: &mut impl Read,
    dest_dir: &Path,
    name: &str,
    dest_name: &str,
    progress: &Progress,
) -> Result<PathBuf, TransferError> {
    validate_macos_path(dest_name)
        .ok()
        .filter(|p| p.len() == 1)
        .ok_or_else(|| TransferError::InvalidManifest("目标文件名无效".into()))?;
    progress.set_phase(Phase::Scanning);
    let dest = fsx::open_dir(dest_dir)?;
    let mut buf = Vec::new();
    let (entries, total) = read_manifest(stream, name, &mut buf)?;
    let files = entries
        .iter()
        .filter(|e| e.kind == EntryKind::File as i32)
        .count() as u64;
    progress.set_totals(total, files);

    let available = fsx::available_bytes(&dest)?;
    let needed = total.saturating_add(SPACE_MARGIN);
    if available < needed {
        return Err(TransferError::DiskFull {
            needed: total,
            available,
        });
    }

    let staging_name = format!(
        "{STAGING_PREFIX}{}-{:016x}",
        std::process::id(),
        random_u64()
    );
    fsx::mkdir_at(&dest, &staging_name, 0o700)?;
    let staging_path = dest_dir.join(&staging_name);
    let result = (|| {
        let staging = fsx::open_dir_at(&dest, &staging_name)?;
        build(stream, &staging, &entries, progress, &mut buf)?;
        progress.set_phase(Phase::Committing);
        let final_name = commit(&staging, name, &dest, dest_name)?;
        // Applied after the rename: moving a read-only directory fails.
        let root = &entries[0];
        if root.kind == EntryKind::Directory as i32 {
            let fd = fsx::open_dir_at(&dest, &final_name)?;
            fsx::set_meta(&fd, root.mode | 0o700, root.mtime_sec, root.mtime_nsec);
        }
        Ok(dest_dir.join(final_name))
    })();
    if let Err(e) = fsx::remove_tree(&staging_path) {
        tracing::warn!("could not remove staging directory: {e}");
    }
    if result.is_ok() {
        progress.set_phase(Phase::Finished);
    }
    result
}

fn read_manifest(
    stream: &mut impl Read,
    name: &str,
    buf: &mut Vec<u8>,
) -> Result<(Vec<ManifestEntry>, u64), TransferError> {
    let bad = |m: String| TransferError::InvalidManifest(m);
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut dirs: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut path_bytes = 0usize;
    let mut total: u64 = 0;
    loop {
        match read_bulk(stream, buf)? {
            Body::Manifest(page) => {
                for e in page.entries {
                    if entries.len() >= MAX_ENTRIES {
                        return Err(TransferError::Limit(format!("超过 {MAX_ENTRIES} 个项目")));
                    }
                    path_bytes += e.path.len() + e.link_target.len();
                    if path_bytes > MAX_PATH_BYTES {
                        return Err(TransferError::Limit("路径总长度过大".into()));
                    }
                    let parts = validate_macos_path(&e.path)
                        .map_err(|err| bad(format!("路径无效（{err}）")))?;
                    if entries.is_empty() {
                        if parts.len() != 1 || parts[0] != name {
                            return Err(bad("第一项不是拖拽的项目本身".into()));
                        }
                    } else {
                        let parent = &e.path[..e
                            .path
                            .rfind('/')
                            .ok_or_else(|| bad("出现了第二个顶层项目".into()))?];
                        // Pre-order: a parent directory always comes first.
                        if !dirs.contains(parent) {
                            return Err(bad("项目的上级文件夹缺失或不是文件夹".into()));
                        }
                    }
                    if !seen.insert(e.path.clone()) {
                        return Err(bad("存在重复的路径".into()));
                    }
                    match EntryKind::try_from(e.kind) {
                        Ok(EntryKind::File) => {
                            total = total
                                .checked_add(e.size)
                                .ok_or_else(|| bad("总大小溢出".into()))?;
                        }
                        Ok(EntryKind::Directory) => {
                            dirs.insert(e.path.clone());
                        }
                        Ok(EntryKind::Symlink) => {
                            if e.link_target.is_empty() || e.link_target.contains('\0') {
                                return Err(bad("符号链接目标无效".into()));
                            }
                        }
                        _ => return Err(bad("未知的项目类型".into())),
                    }
                    entries.push(e);
                }
            }
            Body::ManifestEnd(end) => {
                if entries.is_empty()
                    || end.entry_count != entries.len() as u64
                    || end.total_bytes != total
                {
                    return Err(bad("清单条目数或总大小不一致".into()));
                }
                return Ok((entries, total));
            }
            Body::Failed(f) => return Err(TransferError::from_wire(f)),
            _ => return Err(TransferError::Protocol("清单之前收到了意外的数据".into())),
        }
    }
}

/// Creates the tree under `staging` and fills in every file.
fn build(
    stream: &mut impl Read,
    staging: &OwnedFd,
    entries: &[ManifestEntry],
    progress: &Progress,
    buf: &mut Vec<u8>,
) -> Result<(), TransferError> {
    let mut parents = ParentCache::default();
    for e in entries {
        let (parent, leaf) = split(&e.path);
        let dir = parents.get(staging, parent)?;
        let collision = |err: std::io::Error| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                // Two names the volume considers equal (case or Unicode
                // normalization); refuse rather than merge them.
                TransferError::InvalidManifest(format!("“{}”与另一项目同名", e.path))
            } else {
                TransferError::Io(err)
            }
        };
        match EntryKind::try_from(e.kind) {
            Ok(EntryKind::Directory) => fsx::mkdir_at(dir, leaf, 0o700).map_err(collision)?,
            Ok(EntryKind::Symlink) => {
                fsx::symlink_at(dir, leaf, &e.link_target).map_err(collision)?
            }
            _ => {
                progress.set_phase(Phase::Transferring);
                let file = fsx::create_file_at(dir, leaf).map_err(collision)?;
                receive_file(stream, file, e, progress, buf)?;
                progress.file_done();
            }
        }
    }
    match read_bulk(stream, buf)? {
        Body::Done(_) => {}
        Body::Failed(f) => return Err(TransferError::from_wire(f)),
        _ => return Err(TransferError::Protocol("传输结束标记缺失".into())),
    }
    // Deepest first, so setting a parent's mtime is not undone by a child.
    // The root is handled after commit.
    for e in entries[1..].iter().rev() {
        if e.kind == EntryKind::Directory as i32 {
            let (parent, leaf) = split(&e.path);
            let fd = fsx::open_dir_at(parents.get(staging, parent)?, leaf)?;
            fsx::set_meta(&fd, e.mode | 0o700, e.mtime_sec, e.mtime_nsec);
        }
    }
    Ok(())
}

fn receive_file(
    stream: &mut impl Read,
    mut file: std::fs::File,
    entry: &ManifestEntry,
    progress: &Progress,
    buf: &mut Vec<u8>,
) -> Result<(), TransferError> {
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    loop {
        if progress.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        match read_bulk(stream, buf)? {
            Body::Chunk(c) => {
                written += c.data.len() as u64;
                if written > entry.size {
                    return Err(TransferError::Protocol(format!(
                        "“{}”的数据超过声明的大小",
                        entry.path
                    )));
                }
                hasher.update(&c.data);
                file.write_all(&c.data)?;
                progress.add_bytes(c.data.len() as u64);
            }
            Body::FileEnd(end) => {
                if written != entry.size {
                    return Err(TransferError::Protocol(format!(
                        "“{}”的数据不完整",
                        entry.path
                    )));
                }
                if hasher.finalize().as_slice() != end.sha256.as_slice() {
                    return Err(TransferError::ChecksumMismatch(entry.path.clone()));
                }
                fsx::set_meta(&file, entry.mode, entry.mtime_sec, entry.mtime_nsec);
                return Ok(());
            }
            Body::Failed(f) => return Err(TransferError::from_wire(f)),
            _ => return Err(TransferError::Protocol("文件数据中出现了意外的帧".into())),
        }
    }
}

/// Moves the staged root into `dest`, picking "name 2", "name 3", … when the
/// name is taken. Returns the name used.
fn commit(
    staging: &OwnedFd,
    name: &str,
    dest: &OwnedFd,
    dest_name: &str,
) -> Result<String, TransferError> {
    for n in 1..=MAX_RENAME_ATTEMPTS {
        let candidate = if n == 1 {
            dest_name.to_owned()
        } else {
            numbered(dest_name, n)
        };
        match fsx::rename_excl(staging, name, dest, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(TransferError::NameConflict(dest_name.to_owned()))
}

/// Finder's duplicate naming: "photo.jpg" -> "photo 2.jpg", "Folder" ->
/// "Folder 2", ".hidden" -> ".hidden 2".
pub(crate) fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => {
            format!("{} {n}{}", &name[..dot], &name[dot..])
        }
        _ => format!("{name} {n}"),
    }
}

fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// Holds the most recently used parent directory open. Siblings arrive
/// together in pre-order, so this avoids re-walking the path for most files
/// without holding one fd per directory.
#[derive(Default)]
struct ParentCache {
    path: Option<String>,
    fd: Option<OwnedFd>,
}

impl ParentCache {
    fn get<'a>(&'a mut self, staging: &'a OwnedFd, parent: &str) -> std::io::Result<&'a OwnedFd> {
        if parent.is_empty() {
            return Ok(staging);
        }
        if self.path.as_deref() != Some(parent) {
            let mut parts = parent.split('/');
            let mut fd = fsx::open_dir_at(staging, parts.next().expect("non-empty"))?;
            for p in parts {
                fd = fsx::open_dir_at(&fd, p)?;
            }
            self.fd = Some(fd);
            self.path = Some(parent.to_owned());
        }
        Ok(self.fd.as_ref().expect("set above"))
    }
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    // SAFETY: arc4random_buf fills exactly the buffer given.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    u64::from_ne_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_like_finder() {
        assert_eq!(numbered("photo.jpg", 2), "photo 2.jpg");
        assert_eq!(numbered("Folder", 3), "Folder 3");
        assert_eq!(numbered(".hidden", 2), ".hidden 2");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar 2.gz");
        assert_eq!(numbered("trailing.", 2), "trailing. 2");
    }

    #[test]
    fn splits_paths() {
        assert_eq!(split("a"), ("", "a"));
        assert_eq!(split("a/b/c"), ("a/b", "c"));
    }
}
