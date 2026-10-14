//! Source-side tree scan into manifest entries (report §10.1).

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use osm_protocol::v1::{DragItem, EntryKind, ManifestEntry};

use crate::error::TransferError;
use crate::path::MAX_DEPTH;

/// Entries per dragged item. Bounds receiver memory for the manifest; a
/// starting value (D) sized well above a typical source tree (~50k files).
pub const MAX_ENTRIES: usize = 250_000;
/// Sum of all path lengths in one manifest (D).
pub const MAX_PATH_BYTES: usize = 64 * 1024 * 1024;

/// Describes a top-level dragged item for a `DragOffer` without walking it.
pub fn describe_item(path: &Path) -> Result<DragItem, TransferError> {
    let name = item_name(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| map_io(e, &name))?;
    let (kind, size) = kind_of(&meta).ok_or(TransferError::NotFound)?;
    Ok(DragItem {
        name,
        kind: kind as i32,
        size,
    })
}

/// Name for progress displays; never fails.
pub fn item_display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn item_name(path: &Path) -> Result<String, TransferError> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or(TransferError::NotFound)
}

/// Walks `root` without following symlinks and returns its entries in
/// depth-first pre-order, children sorted by name. Sockets, FIFOs and device
/// nodes are skipped, as Finder does.
pub fn scan(root: &Path) -> Result<Vec<ManifestEntry>, TransferError> {
    let root_name = item_name(root)?;
    let base = root.parent().ok_or(TransferError::NotFound)?;
    let mut out = Vec::new();
    let mut path_bytes = 0usize;
    // (relative path, depth); popped from the back, so children are pushed
    // in reverse to come out in name order.
    let mut stack = vec![(root_name, 1usize)];
    while let Some((rel, depth)) = stack.pop() {
        let full = base.join(&rel);
        let meta = fs::symlink_metadata(&full).map_err(|e| map_io(e, &rel))?;
        let Some((kind, size)) = kind_of(&meta) else {
            tracing::debug!("skipping special file in drag");
            continue;
        };
        if out.len() >= MAX_ENTRIES {
            return Err(TransferError::Limit(format!("超过 {MAX_ENTRIES} 个项目")));
        }
        path_bytes += rel.len();
        if path_bytes > MAX_PATH_BYTES {
            return Err(TransferError::Limit("路径总长度过大".into()));
        }
        let link_target = if kind == EntryKind::Symlink {
            fs::read_link(&full)
                .map_err(|e| map_io(e, &rel))?
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| TransferError::Limit(format!("“{rel}”的链接目标不是有效文本")))?
        } else {
            String::new()
        };
        if kind == EntryKind::Directory {
            if depth >= MAX_DEPTH {
                return Err(TransferError::Limit(format!("超过 {MAX_DEPTH} 层")));
            }
            let mut names = Vec::new();
            for child in fs::read_dir(&full).map_err(|e| map_io(e, &rel))? {
                let child = child.map_err(|e| map_io(e, &rel))?;
                let name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| TransferError::Limit(format!("“{rel}”中有无法识别的文件名")))?;
                names.push(name);
            }
            names.sort_unstable();
            for name in names.into_iter().rev() {
                stack.push((format!("{rel}/{name}"), depth + 1));
            }
        }
        out.push(ManifestEntry {
            path: rel,
            kind: kind as i32,
            size,
            mode: meta.permissions().mode() & 0o777,
            mtime_sec: meta.mtime(),
            mtime_nsec: meta.mtime_nsec().clamp(0, 999_999_999) as u32,
            link_target,
        });
    }
    if out.is_empty() {
        return Err(TransferError::NotFound);
    }
    Ok(out)
}

fn kind_of(meta: &fs::Metadata) -> Option<(EntryKind, u64)> {
    let t = meta.file_type();
    if t.is_symlink() {
        Some((EntryKind::Symlink, 0))
    } else if t.is_dir() {
        Some((EntryKind::Directory, 0))
    } else if t.is_file() {
        Some((EntryKind::File, meta.len()))
    } else {
        None
    }
}

pub(crate) fn map_io(e: std::io::Error, name: &str) -> TransferError {
    match e.kind() {
        std::io::ErrorKind::NotFound => TransferError::NotFound,
        std::io::ErrorKind::PermissionDenied => TransferError::PermissionDenied(name.to_owned()),
        _ => TransferError::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_pre_order_without_following_links() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Top");
        fs::create_dir_all(root.join("b/empty")).unwrap();
        fs::write(root.join("a.txt"), b"hello").unwrap();
        fs::write(root.join("b/c.bin"), vec![0u8; 3000]).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("link")).unwrap();

        let entries = scan(&root).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Top",
                "Top/a.txt",
                "Top/b",
                "Top/b/c.bin",
                "Top/b/empty",
                "Top/link"
            ]
        );
        assert_eq!(entries[1].size, 5);
        assert_eq!(entries[3].size, 3000);
        assert_eq!(entries[5].kind, EntryKind::Symlink as i32);
        assert_eq!(entries[5].link_target, "/etc");
    }

    #[test]
    fn describes_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("报告.pdf");
        fs::write(&f, b"1234").unwrap();
        let item = describe_item(&f).unwrap();
        assert_eq!(item.name, "报告.pdf");
        assert_eq!(item.kind, EntryKind::File as i32);
        assert_eq!(item.size, 4);
        assert!(matches!(
            describe_item(&tmp.path().join("missing")),
            Err(TransferError::NotFound)
        ));
    }
}
