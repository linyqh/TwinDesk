//! Sender and receiver over a loopback socket, against real directories.

use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::thread;

use bytes::BytesMut;
use osm_protocol::v1::bulk_frame::Body;
use osm_protocol::v1::{
    BulkFrame, EntryKind, FetchRequest, FileChunk, FileEnd, ManifestEnd, ManifestEntry,
    ManifestPage, TransferDone,
};
use osm_protocol::write_bulk;
use osm_transfer::{Phase, Progress, TransferError, receive, send};

fn request() -> FetchRequest {
    FetchRequest {
        drag_id: 1,
        token: vec![9; 16],
        item_index: 0,
    }
}

/// Serves `root` on one end of a loopback socket and receives it into
/// `dest` on the other.
fn transfer(root: &Path, dest: &Path) -> (Result<PathBuf, TransferError>, Progress) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let served = root.to_path_buf();
    let server = thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let req = send::read_request(&mut BufReader::new(s.try_clone().unwrap())).unwrap();
        assert_eq!(req.token, vec![9; 16]);
        let mut w = BufWriter::new(s);
        let _ = send::serve(&mut w, &served, &Progress::new());
        let _ = w.flush();
    });
    let mut s = TcpStream::connect(addr).unwrap();
    receive::send_request(&mut s, request()).unwrap();
    let progress = Progress::new();
    let name = root.file_name().unwrap().to_str().unwrap();
    let res = receive::receive(&mut BufReader::new(s), dest, name, name, &progress);
    server.join().unwrap();
    (res, progress)
}

/// Feeds hand-made frames to the receiver, as a hostile peer would.
fn receive_frames(frames: Vec<Body>, dest: &Path, name: &str) -> Result<PathBuf, TransferError> {
    let mut wire = Vec::new();
    let mut scratch = BytesMut::new();
    for body in frames {
        write_bulk(&mut wire, &BulkFrame { body: Some(body) }, &mut scratch).unwrap();
    }
    receive::receive(&mut &wire[..], dest, name, name, &Progress::new())
}

fn entry(path: &str, kind: EntryKind, size: u64) -> ManifestEntry {
    ManifestEntry {
        path: path.into(),
        kind: kind as i32,
        size,
        mode: 0o644,
        mtime_sec: 1_700_000_000,
        mtime_nsec: 0,
        link_target: String::new(),
    }
}

fn visible(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

fn source_tree(base: &Path) -> PathBuf {
    let root = base.join("项目 Folder");
    fs::create_dir_all(root.join("sub/empty dir")).unwrap();
    fs::write(root.join("a.txt"), b"hello").unwrap();
    fs::write(root.join("empty.bin"), b"").unwrap();
    fs::write(root.join("Icon\r"), b"x").unwrap();
    let big: Vec<u8> = (0..(3 * 256 * 1024 + 17))
        .map(|i| (i % 251) as u8)
        .collect();
    fs::write(root.join("sub/big.dat"), &big).unwrap();
    std::os::unix::fs::symlink("a.txt", root.join("link")).unwrap();
    fs::set_permissions(root.join("a.txt"), fs::Permissions::from_mode(0o755)).unwrap();
    root
}

#[test]
fn copies_a_tree_exactly() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = source_tree(src.path());

    let (res, progress) = transfer(&root, dst.path());
    let out = res.unwrap();
    assert_eq!(out, dst.path().join("项目 Folder"));
    assert_eq!(
        visible(dst.path()),
        ["项目 Folder"],
        "no staging left behind"
    );

    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"hello");
    assert_eq!(fs::read(out.join("empty.bin")).unwrap(), b"");
    assert_eq!(fs::read(out.join("Icon\r")).unwrap(), b"x");
    assert_eq!(
        fs::read(out.join("sub/big.dat")).unwrap(),
        fs::read(root.join("sub/big.dat")).unwrap()
    );
    assert!(out.join("sub/empty dir").is_dir());
    assert_eq!(fs::read_link(out.join("link")).unwrap(), Path::new("a.txt"));
    let meta = fs::metadata(out.join("a.txt")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o755);
    let src_meta = fs::metadata(root.join("sub/big.dat")).unwrap();
    assert_eq!(
        fs::metadata(out.join("sub/big.dat")).unwrap().mtime(),
        src_meta.mtime()
    );

    let p = progress.snapshot();
    assert_eq!(p.phase, Phase::Finished);
    assert_eq!(p.total_files, 4);
    assert_eq!(p.done_files, 4);
    assert_eq!(p.done_bytes, p.total_bytes);
    assert_eq!(p.total_bytes, 5 + 1 + 3 * 256 * 1024 + 17);
}

#[test]
fn single_file_and_name_conflicts_keep_both() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = src.path().join("photo.jpg");
    fs::write(&f, b"new").unwrap();
    fs::write(dst.path().join("photo.jpg"), b"old").unwrap();

    let out = transfer(&f, dst.path()).0.unwrap();
    assert_eq!(out, dst.path().join("photo 2.jpg"));
    let out = transfer(&f, dst.path()).0.unwrap();
    assert_eq!(out, dst.path().join("photo 3.jpg"));
    assert_eq!(fs::read(dst.path().join("photo.jpg")).unwrap(), b"old");
    assert_eq!(fs::read(dst.path().join("photo 3.jpg")).unwrap(), b"new");
}

#[test]
fn commits_under_the_name_the_drop_target_asked_for() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = src.path().join("a.txt");
    fs::write(&f, b"x").unwrap();
    let frames_for = |dest_name: &str| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let served = f.clone();
        let server = thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            send::read_request(&mut BufReader::new(s.try_clone().unwrap())).unwrap();
            let _ = send::serve(&mut BufWriter::new(s), &served, &Progress::new());
        });
        let mut s = TcpStream::connect(addr).unwrap();
        receive::send_request(&mut s, request()).unwrap();
        let r = receive::receive(
            &mut BufReader::new(s),
            dst.path(),
            "a.txt",
            dest_name,
            &Progress::new(),
        );
        server.join().unwrap();
        r
    };
    assert_eq!(
        frames_for("a copy.txt").unwrap(),
        dst.path().join("a copy.txt")
    );
    assert!(matches!(
        frames_for("../evil"),
        Err(TransferError::InvalidManifest(_))
    ));
}

#[test]
fn read_only_directory_still_commits() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = src.path().join("ro");
    fs::create_dir_all(root.join("inner")).unwrap();
    fs::write(root.join("inner/f"), b"1").unwrap();
    fs::set_permissions(root.join("inner"), fs::Permissions::from_mode(0o555)).unwrap();

    let out = transfer(&root, dst.path()).0.unwrap();
    assert_eq!(fs::read(out.join("inner/f")).unwrap(), b"1");
    fs::set_permissions(root.join("inner"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn unreadable_file_fails_cleanly() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = src.path().join("d");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("secret"), b"s").unwrap();
    fs::set_permissions(root.join("secret"), fs::Permissions::from_mode(0o000)).unwrap();

    let err = transfer(&root, dst.path()).0.unwrap_err();
    assert!(
        matches!(err, TransferError::Remote(ref m) if m.contains("secret")),
        "{err}"
    );
    assert!(visible(dst.path()).is_empty());
}

#[test]
fn missing_source_reports_not_found() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let err = transfer(&src.path().join("gone"), dst.path())
        .0
        .unwrap_err();
    assert!(matches!(err, TransferError::NotFound), "{err}");
}

#[test]
fn rejects_traversal_in_manifest() {
    let dst = tempfile::tempdir().unwrap();
    for bad in ["x/../../escape", "/abs", "x//y"] {
        let frames = vec![
            Body::Manifest(ManifestPage {
                entries: vec![
                    entry("x", EntryKind::Directory, 0),
                    entry(bad, EntryKind::File, 1),
                ],
            }),
            Body::ManifestEnd(ManifestEnd {
                entry_count: 2,
                total_bytes: 1,
            }),
        ];
        let err = receive_frames(frames, dst.path(), "x").unwrap_err();
        assert!(
            matches!(err, TransferError::InvalidManifest(_)),
            "{bad}: {err}"
        );
    }
    assert!(visible(dst.path()).is_empty());
}

#[test]
fn rejects_wrong_root_orphans_and_duplicates() {
    let dst = tempfile::tempdir().unwrap();
    let cases: Vec<Vec<ManifestEntry>> = vec![
        // Root is not the offered item.
        vec![entry("other", EntryKind::File, 0)],
        // Child whose parent is a file.
        vec![
            entry("x", EntryKind::File, 0),
            entry("x/y", EntryKind::File, 0),
        ],
        // Child before parent.
        vec![
            entry("x", EntryKind::Directory, 0),
            entry("x/a/b", EntryKind::File, 0),
        ],
        // Duplicate.
        vec![
            entry("x", EntryKind::Directory, 0),
            entry("x/a", EntryKind::File, 0),
            entry("x/a", EntryKind::File, 0),
        ],
    ];
    for entries in cases {
        let n = entries.len() as u64;
        let frames = vec![
            Body::Manifest(ManifestPage { entries }),
            Body::ManifestEnd(ManifestEnd {
                entry_count: n,
                total_bytes: 0,
            }),
        ];
        let err = receive_frames(frames, dst.path(), "x").unwrap_err();
        assert!(matches!(err, TransferError::InvalidManifest(_)), "{err}");
    }
}

#[test]
fn case_insensitive_collision_is_refused_not_merged() {
    let dst = tempfile::tempdir().unwrap();
    let frames = vec![
        Body::Manifest(ManifestPage {
            entries: vec![
                entry("x", EntryKind::Directory, 0),
                entry("x/A", EntryKind::File, 0),
                entry("x/a", EntryKind::File, 0),
            ],
        }),
        Body::ManifestEnd(ManifestEnd {
            entry_count: 3,
            total_bytes: 0,
        }),
        Body::FileEnd(FileEnd {
            sha256: sha_of(b""),
        }),
        Body::FileEnd(FileEnd {
            sha256: sha_of(b""),
        }),
        Body::Done(TransferDone {}),
    ];
    match receive_frames(frames, dst.path(), "x") {
        // Case-sensitive volume: both names are distinct and legal.
        Ok(out) => assert!(out.join("A").exists() && out.join("a").exists()),
        Err(e) => assert!(matches!(e, TransferError::InvalidManifest(_)), "{e}"),
    }
    assert!(
        visible(dst.path())
            .iter()
            .all(|n| !n.starts_with(".osm-staging"))
    );
}

#[test]
fn corrupted_data_fails_checksum_and_leaves_nothing() {
    let dst = tempfile::tempdir().unwrap();
    let frames = vec![
        Body::Manifest(ManifestPage {
            entries: vec![entry("f.txt", EntryKind::File, 3)],
        }),
        Body::ManifestEnd(ManifestEnd {
            entry_count: 1,
            total_bytes: 3,
        }),
        Body::Chunk(FileChunk {
            data: b"abc".to_vec(),
        }),
        Body::FileEnd(FileEnd {
            sha256: sha_of(b"abd"),
        }),
        Body::Done(TransferDone {}),
    ];
    let err = receive_frames(frames, dst.path(), "f.txt").unwrap_err();
    assert!(matches!(err, TransferError::ChecksumMismatch(_)), "{err}");
    assert!(visible(dst.path()).is_empty());
}

#[test]
fn oversized_and_truncated_data_are_rejected() {
    let dst = tempfile::tempdir().unwrap();
    for (data, declared) in [(b"abcd".to_vec(), 3u64), (b"ab".to_vec(), 3)] {
        let frames = vec![
            Body::Manifest(ManifestPage {
                entries: vec![entry("f", EntryKind::File, declared)],
            }),
            Body::ManifestEnd(ManifestEnd {
                entry_count: 1,
                total_bytes: declared,
            }),
            Body::Chunk(FileChunk { data: data.clone() }),
            Body::FileEnd(FileEnd {
                sha256: sha_of(&data),
            }),
            Body::Done(TransferDone {}),
        ];
        let err = receive_frames(frames, dst.path(), "f").unwrap_err();
        assert!(matches!(err, TransferError::Protocol(_)), "{err}");
    }
    assert!(visible(dst.path()).is_empty());
}

#[test]
fn connection_lost_mid_file_leaves_nothing() {
    let dst = tempfile::tempdir().unwrap();
    let frames = vec![
        Body::Manifest(ManifestPage {
            entries: vec![entry("f", EntryKind::File, 10)],
        }),
        Body::ManifestEnd(ManifestEnd {
            entry_count: 1,
            total_bytes: 10,
        }),
        Body::Chunk(FileChunk { data: vec![1; 5] }),
    ];
    let err = receive_frames(frames, dst.path(), "f").unwrap_err();
    assert!(matches!(err, TransferError::Io(_)), "{err}");
    assert!(visible(dst.path()).is_empty());
}

#[test]
fn insufficient_space_is_reported_before_writing() {
    let dst = tempfile::tempdir().unwrap();
    let frames = vec![
        Body::Manifest(ManifestPage {
            entries: vec![entry("huge", EntryKind::File, u64::MAX / 2)],
        }),
        Body::ManifestEnd(ManifestEnd {
            entry_count: 1,
            total_bytes: u64::MAX / 2,
        }),
    ];
    let err = receive_frames(frames, dst.path(), "huge").unwrap_err();
    assert!(matches!(err, TransferError::DiskFull { .. }), "{err}");
    assert!(visible(dst.path()).is_empty());
}

#[test]
fn cancel_stops_the_receiver() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = source_tree(src.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        send::read_request(&mut BufReader::new(s.try_clone().unwrap())).unwrap();
        let _ = send::serve(&mut BufWriter::new(s), &root, &Progress::new());
    });
    let mut s = TcpStream::connect(addr).unwrap();
    receive::send_request(&mut s, request()).unwrap();
    let progress = Progress::new();
    progress.cancel();
    let name = "项目 Folder";
    let err =
        receive::receive(&mut BufReader::new(s), dst.path(), name, name, &progress).unwrap_err();
    assert!(matches!(err, TransferError::Cancelled), "{err}");
    assert!(visible(dst.path()).is_empty());
    server.join().unwrap();
}

#[test]
fn source_changed_during_send_is_detected() {
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("growing.log");
    fs::write(&f, vec![0u8; 1000]).unwrap();
    // Serving into a writer that appends to the file mid-stream simulates a
    // file that keeps growing while it is read.
    struct Grows<'a>(&'a Path, bool);
    impl Write for Grows<'_> {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if !self.1 {
                self.1 = true;
                let mut f = fs::OpenOptions::new().append(true).open(self.0)?;
                f.write_all(&[1u8; 10])?;
            }
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let err = send::serve(&mut Grows(&f, false), &f, &Progress::new()).unwrap_err();
    assert!(matches!(err, TransferError::SourceChanged(_)), "{err}");
}

fn sha_of(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(data).to_vec()
}
