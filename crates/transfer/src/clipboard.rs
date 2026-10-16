//! Clipboard hand-off over a bulk connection (report §9).
//!
//! The sender writes, per representation, a `ClipboardFormat` and then
//! `FileChunk`s totalling its size, and finally `TransferDone`. Only the
//! types below cross the cable; anything else stays on its own Mac.

use std::io::{Read, Write};

use bytes::BytesMut;
use osm_protocol::v1::bulk_frame::Body;
use osm_protocol::v1::{BulkFrame, ClipboardFetch, ClipboardFormat, FileChunk, TransferDone};
use osm_protocol::{CHUNK_SIZE, read_bulk, write_bulk};

use crate::error::TransferError;

pub const TEXT: &str = "public.utf8-plain-text";
pub const RTF: &str = "public.rtf";
pub const HTML: &str = "public.html";
pub const PNG: &str = "public.png";
pub const TIFF: &str = "public.tiff";

/// Report §9.3 starting values (D): text 16 MiB, compressed image 64 MiB.
const TEXT_LIMIT: u64 = 16 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;

/// nspasteboard.org markers set by password managers and by apps that
/// write short-lived data; such contents never leave this Mac.
const PRIVATE_MARKERS: [&str; 2] = [
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];
/// Finder file copies need the file transfer path, not raw bytes.
const FILE_URL: &str = "public.file-url";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Representation {
    pub ty: String,
    pub data: Vec<u8>,
}

fn limit(ty: &str) -> Option<u64> {
    match ty {
        TEXT | RTF | HTML => Some(TEXT_LIMIT),
        PNG | TIFF => Some(IMAGE_LIMIT),
        _ => None,
    }
}

/// Types worth sending from a pasteboard advertising `available`, in the
/// writer's preference order; empty when nothing should be shared.
pub fn shareable_types(available: &[String]) -> Vec<String> {
    let has = |ty: &str| available.iter().any(|t| t == ty);
    if PRIVATE_MARKERS.iter().any(|m| has(m)) || has(FILE_URL) {
        return Vec::new();
    }
    available
        .iter()
        .filter(|t| limit(t).is_some())
        // PNG is far smaller than the equivalent TIFF and every app reads it.
        .filter(|t| !(*t == TIFF && has(PNG)))
        .cloned()
        .collect()
}

/// Drops representations over their limit. If nothing is left, the copy is
/// too large to hand off.
pub fn within_limits(reps: Vec<Representation>) -> Result<Vec<Representation>, TransferError> {
    let kept: Vec<_> = reps
        .into_iter()
        .filter(|r| limit(&r.ty).is_some_and(|max| r.data.len() as u64 <= max))
        .collect();
    if kept.is_empty() {
        return Err(TransferError::Limit(
            "剪贴板内容过大，请改用文件传送".into(),
        ));
    }
    Ok(kept)
}

pub fn send_fetch(stream: &mut impl Write, fetch: ClipboardFetch) -> Result<(), TransferError> {
    write_bulk(
        stream,
        &frame(Body::ClipboardFetch(fetch)),
        &mut BytesMut::new(),
    )?;
    Ok(())
}

pub fn serve(stream: &mut impl Write, reps: &[Representation]) -> Result<(), TransferError> {
    let mut scratch = BytesMut::new();
    for rep in reps {
        write_bulk(
            stream,
            &frame(Body::ClipboardFormat(ClipboardFormat {
                r#type: rep.ty.clone(),
                size: rep.data.len() as u64,
            })),
            &mut scratch,
        )?;
        for chunk in rep.data.chunks(CHUNK_SIZE) {
            write_bulk(
                stream,
                &frame(Body::Chunk(FileChunk {
                    data: chunk.to_vec(),
                })),
                &mut scratch,
            )?;
        }
    }
    write_bulk(stream, &frame(Body::Done(TransferDone {})), &mut scratch)?;
    Ok(())
}

/// Reads one clipboard, enforcing the same per-type limits as the sender so
/// a hostile peer cannot make us buffer unbounded data.
pub fn receive(stream: &mut impl Read) -> Result<Vec<Representation>, TransferError> {
    let mut reps: Vec<Representation> = Vec::new();
    let mut remaining = 0u64;
    let mut buf = Vec::new();
    loop {
        match read_bulk(stream, &mut buf)? {
            Body::ClipboardFormat(f) if remaining == 0 => {
                let max = limit(&f.r#type)
                    .ok_or_else(|| TransferError::Protocol("不支持的剪贴板格式".into()))?;
                if f.size > max {
                    return Err(TransferError::Limit("剪贴板内容过大".into()));
                }
                if reps.iter().any(|r| r.ty == f.r#type) {
                    return Err(TransferError::Protocol("剪贴板格式重复".into()));
                }
                remaining = f.size;
                reps.push(Representation {
                    ty: f.r#type,
                    data: Vec::with_capacity(f.size as usize),
                });
            }
            Body::Chunk(c) if (c.data.len() as u64) <= remaining && !c.data.is_empty() => {
                remaining -= c.data.len() as u64;
                reps.last_mut()
                    .expect("remaining > 0 implies a format")
                    .data
                    .extend_from_slice(&c.data);
            }
            Body::Done(_) if remaining == 0 && !reps.is_empty() => return Ok(reps),
            Body::Failed(f) => return Err(TransferError::from_wire(f)),
            _ => return Err(TransferError::Protocol("剪贴板数据不完整".into())),
        }
    }
}

fn frame(body: Body) -> BulkFrame {
    BulkFrame { body: Some(body) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keeps_supported_types_in_order_and_prefers_png_over_tiff() {
        assert_eq!(
            shareable_types(&types(&[HTML, "com.apple.webarchive", TEXT, TIFF, PNG])),
            types(&[HTML, TEXT, PNG])
        );
        assert_eq!(shareable_types(&types(&[TIFF])), types(&[TIFF]));
        assert!(shareable_types(&types(&["com.example.private"])).is_empty());
    }

    #[test]
    fn never_shares_passwords_transient_data_or_finder_files() {
        for marker in [PRIVATE_MARKERS[0], PRIVATE_MARKERS[1], FILE_URL] {
            assert!(shareable_types(&types(&[TEXT, marker])).is_empty());
        }
    }

    #[test]
    fn drops_oversized_representations_and_fails_when_none_remain() {
        let big = Representation {
            ty: TEXT.into(),
            data: vec![b'a'; TEXT_LIMIT as usize + 1],
        };
        let png = Representation {
            ty: PNG.into(),
            data: vec![1, 2, 3],
        };
        assert_eq!(
            within_limits(vec![big.clone(), png.clone()]).unwrap(),
            vec![png]
        );
        assert!(matches!(
            within_limits(vec![big]),
            Err(TransferError::Limit(_))
        ));
    }

    #[test]
    fn round_trips_multi_chunk_and_empty_representations() {
        let reps = vec![
            Representation {
                ty: TEXT.into(),
                data: "跨电脑粘贴".as_bytes().to_vec(),
            },
            Representation {
                ty: HTML.into(),
                data: Vec::new(),
            },
            Representation {
                ty: PNG.into(),
                data: (0..CHUNK_SIZE * 2 + 7).map(|i| i as u8).collect(),
            },
        ];
        let mut wire = Vec::new();
        serve(&mut wire, &reps).unwrap();
        assert_eq!(receive(&mut &wire[..]).unwrap(), reps);
    }

    fn wire(frames: Vec<Body>) -> Vec<u8> {
        let mut out = Vec::new();
        for body in frames {
            write_bulk(&mut out, &frame(body), &mut BytesMut::new()).unwrap();
        }
        out
    }

    fn format(ty: &str, size: u64) -> Body {
        Body::ClipboardFormat(ClipboardFormat {
            r#type: ty.into(),
            size,
        })
    }

    fn chunk(data: &[u8]) -> Body {
        Body::Chunk(FileChunk {
            data: data.to_vec(),
        })
    }

    #[test]
    fn rejects_hostile_or_truncated_streams() {
        let done = || Body::Done(TransferDone {});
        let cases = [
            vec![format("com.example.secret", 1), chunk(b"x"), done()],
            vec![format(TEXT, TEXT_LIMIT + 1)],
            vec![format(TEXT, 2), chunk(b"abc"), done()],
            vec![format(TEXT, 3), chunk(b"ab"), done()],
            vec![
                format(TEXT, 1),
                chunk(b"a"),
                format(TEXT, 1),
                chunk(b"b"),
                done(),
            ],
            vec![done()],
        ];
        for frames in cases {
            assert!(
                receive(&mut &wire(frames.clone())[..]).is_err(),
                "{frames:?}"
            );
        }
        // A clean EOF before Done is also a failure, never an empty paste.
        assert!(receive(&mut &wire(vec![format(TEXT, 1), chunk(b"a")])[..]).is_err());
    }

    #[test]
    fn reports_sender_failure() {
        let failed = Body::Failed(TransferError::SourceChanged("剪贴板".into()).to_wire());
        assert!(matches!(
            receive(&mut &wire(vec![failed])[..]),
            Err(TransferError::Remote(_))
        ));
    }
}
