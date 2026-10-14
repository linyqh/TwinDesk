//! Directory-handle relative file operations (report §10.2).
//!
//! The receiver opens its destination once and creates everything relative
//! to that handle with `O_NOFOLLOW`/`O_EXCL`, so a directory swapped for a
//! symlink mid-transfer cannot redirect writes, and two manifest entries
//! that the file system considers equal (case or Unicode normalization)
//! collide with `EEXIST` instead of overwriting each other.

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

fn cstr(name: &str) -> io::Result<CString> {
    CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))
}

fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

pub(crate) fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    // SAFETY: valid C string; the returned fd is owned below.
    let fd = check(unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    // SAFETY: fd was just returned by open and is not owned elsewhere.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(crate) fn open_dir_at(dir: &OwnedFd, name: &str) -> io::Result<OwnedFd> {
    let c = cstr(name)?;
    // SAFETY: valid dirfd and C string.
    let fd = check(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    // SAFETY: fresh fd.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(crate) fn mkdir_at(dir: &OwnedFd, name: &str, mode: u32) -> io::Result<()> {
    let c = cstr(name)?;
    // SAFETY: valid dirfd and C string.
    check(unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), mode as libc::mode_t) }).map(|_| ())
}

/// Creates a new file; fails if anything already has that name.
pub(crate) fn create_file_at(dir: &OwnedFd, name: &str) -> io::Result<File> {
    let c = cstr(name)?;
    // SAFETY: valid dirfd and C string; mode passed as the variadic arg.
    let fd = check(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    })?;
    // SAFETY: fresh fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Opens a source file for reading without following a final symlink.
pub(crate) fn open_source(path: &Path) -> io::Result<File> {
    let c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    // SAFETY: valid C string.
    let fd = check(unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    // SAFETY: fresh fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(crate) fn symlink_at(dir: &OwnedFd, name: &str, target: &str) -> io::Result<()> {
    let n = cstr(name)?;
    let t = cstr(target)?;
    // SAFETY: valid dirfd and C strings.
    check(unsafe { libc::symlinkat(t.as_ptr(), dir.as_raw_fd(), n.as_ptr()) }).map(|_| ())
}

/// Moves `from` (in `from_dir`) to `to` (in `to_dir`) atomically, failing
/// with `AlreadyExists` instead of replacing an existing item.
pub(crate) fn rename_excl(
    from_dir: &OwnedFd,
    from: &str,
    to_dir: &OwnedFd,
    to: &str,
) -> io::Result<()> {
    let f = cstr(from)?;
    let t = cstr(to)?;
    // SAFETY: valid dirfds and C strings.
    check(unsafe {
        libc::renameatx_np(
            from_dir.as_raw_fd(),
            f.as_ptr(),
            to_dir.as_raw_fd(),
            t.as_ptr(),
            libc::RENAME_EXCL,
        )
    })
    .map(|_| ())
}

/// Sets permission bits and modification time on an open file or directory.
pub(crate) fn set_meta(fd: &impl AsRawFd, mode: u32, mtime_sec: i64, mtime_nsec: u32) {
    let times = [
        // Access time: leave unchanged.
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: mtime_sec,
            tv_nsec: i64::from(mtime_nsec.min(999_999_999)),
        },
    ];
    // Best effort: metadata is not worth failing a verified copy over.
    // SAFETY: valid fd; `times` holds two timespecs as futimens expects.
    unsafe {
        libc::futimens(fd.as_raw_fd(), times.as_ptr());
        libc::fchmod(fd.as_raw_fd(), (mode & 0o777) as libc::mode_t);
    }
}

/// Bytes available to an unprivileged user on the volume holding `dir`.
pub(crate) fn available_bytes(dir: &OwnedFd) -> io::Result<u64> {
    // SAFETY: statfs is plain data; zeroed is a valid initial value.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid fd and out pointer.
    check(unsafe { libc::fstatfs(dir.as_raw_fd(), &mut st) })?;
    Ok(st.f_bavail.saturating_mul(u64::from(st.f_bsize)))
}

/// Removes a tree this process created, including directories it already
/// made read-only. Never follows symlinks.
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        for child in std::fs::read_dir(path)? {
            remove_tree(&child?.path())?;
        }
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}
