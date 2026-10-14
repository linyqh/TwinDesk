//! Read-only view of the system drag pasteboard.
//!
//! When the cursor leaves the screen with the left button held, the engine
//! asks whether a drag session started since the button went down and, if
//! so, which files it carries. The Carbon Pasteboard C API gives that answer
//! from any thread without AppKit: `PasteboardSynchronize` reports
//! `kPasteboardModified` when another process rewrote the pasteboard since
//! our last look, which is exactly "a new drag began".

use std::ffi::c_void;
use std::path::PathBuf;

use crate::sys::CFRelease;

type OSStatus = i32;
type PasteboardRef = *mut c_void;
type PasteboardItemID = *mut c_void;
type CFStringRef = *const c_void;
type CFDataRef = *const c_void;
type CFURLRef = *const c_void;

const K_PASTEBOARD_MODIFIED: u32 = 1 << 0;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const MAX_ITEMS: usize = 4096;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn PasteboardCreate(name: CFStringRef, out: *mut PasteboardRef) -> OSStatus;
    fn PasteboardSynchronize(pb: PasteboardRef) -> u32;
    fn PasteboardGetItemCount(pb: PasteboardRef, out: *mut usize) -> OSStatus;
    fn PasteboardGetItemIdentifier(
        pb: PasteboardRef,
        index: usize,
        out: *mut PasteboardItemID,
    ) -> OSStatus;
    fn PasteboardCopyItemFlavorData(
        pb: PasteboardRef,
        item: PasteboardItemID,
        flavor: CFStringRef,
        out: *mut CFDataRef,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const libc::c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFDataGetLength(d: CFDataRef) -> isize;
    fn CFDataGetBytePtr(d: CFDataRef) -> *const u8;
    fn CFURLCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        base: CFURLRef,
    ) -> CFURLRef;
    fn CFURLCreateFilePathURL(alloc: *const c_void, url: CFURLRef, err: *mut c_void) -> CFURLRef;
    fn CFURLGetFileSystemRepresentation(
        url: CFURLRef,
        resolve_against_base: bool,
        buf: *mut u8,
        len: isize,
    ) -> bool;
}

#[derive(Debug)]
pub struct DragPasteboard {
    pb: PasteboardRef,
    flavor: CFStringRef,
}

// SAFETY: the pasteboard ref is only used by the thread owning this value.
unsafe impl Send for DragPasteboard {}

impl DragPasteboard {
    pub fn open() -> Option<Self> {
        // SAFETY: plain CF constructors; both results are released in Drop.
        unsafe {
            let name = CFStringCreateWithCString(
                std::ptr::null(),
                c"Apple CFPasteboard drag".as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            let flavor = CFStringCreateWithCString(
                std::ptr::null(),
                c"public.file-url".as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            let mut pb = std::ptr::null_mut();
            let status = PasteboardCreate(name, &mut pb);
            CFRelease(name);
            if status != 0 || pb.is_null() {
                CFRelease(flavor);
                return None;
            }
            let this = Self { pb, flavor };
            // Absorb whatever drag happened before we started watching.
            this.changed();
            Some(this)
        }
    }

    /// True if another process wrote the drag pasteboard since the last call.
    pub fn changed(&self) -> bool {
        // SAFETY: valid pasteboard.
        unsafe { PasteboardSynchronize(self.pb) & K_PASTEBOARD_MODIFIED != 0 }
    }

    /// Local file paths on the drag pasteboard, in drag order. Empty for a
    /// drag of text, images or file promises.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        // SAFETY: valid pasteboard; every CF object created here is released.
        unsafe {
            let mut count = 0usize;
            if PasteboardGetItemCount(self.pb, &mut count) != 0 {
                return out;
            }
            for i in 1..=count.min(MAX_ITEMS) {
                let mut item = std::ptr::null_mut();
                if PasteboardGetItemIdentifier(self.pb, i, &mut item) != 0 {
                    continue;
                }
                let mut data = std::ptr::null();
                if PasteboardCopyItemFlavorData(self.pb, item, self.flavor, &mut data) != 0
                    || data.is_null()
                {
                    continue;
                }
                if let Some(p) = path_from_url_data(data) {
                    out.push(p);
                }
                CFRelease(data);
            }
        }
        out
    }
}

/// Converts `file://` URL bytes (including Finder's `/.file/id=` reference
/// URLs) into a file system path.
///
/// # Safety
/// `data` must be a valid CFData.
unsafe fn path_from_url_data(data: CFDataRef) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: caller guarantees valid CFData; created URLs are released.
    unsafe {
        let url = CFURLCreateWithBytes(
            std::ptr::null(),
            CFDataGetBytePtr(data),
            CFDataGetLength(data),
            K_CF_STRING_ENCODING_UTF8,
            std::ptr::null(),
        );
        if url.is_null() {
            return None;
        }
        // Resolves file reference URLs to path URLs.
        let path_url = CFURLCreateFilePathURL(std::ptr::null(), url, std::ptr::null_mut());
        CFRelease(url);
        if path_url.is_null() {
            return None;
        }
        let mut buf = vec![0u8; libc::PATH_MAX as usize * 4];
        let ok =
            CFURLGetFileSystemRepresentation(path_url, true, buf.as_mut_ptr(), buf.len() as isize);
        CFRelease(path_url);
        if !ok {
            return None;
        }
        let len = buf.iter().position(|b| *b == 0)?;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
    }
}

impl Drop for DragPasteboard {
    fn drop(&mut self) {
        // SAFETY: both were created in open().
        unsafe {
            CFRelease(self.pb);
            CFRelease(self.flavor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_reads_without_crashing() {
        let pb = DragPasteboard::open().expect("drag pasteboard");
        // Just opened: nothing changed since.
        assert!(!pb.changed());
        for p in pb.file_paths() {
            assert!(p.is_absolute());
        }
    }

    #[test]
    fn resolves_plain_file_url() {
        let url = b"file:///tmp/a%20b.txt";
        // SAFETY: CFData over a static buffer, released below.
        unsafe {
            unsafe extern "C" {
                fn CFDataCreate(a: *const c_void, b: *const u8, n: isize) -> CFDataRef;
            }
            let d = CFDataCreate(std::ptr::null(), url.as_ptr(), url.len() as isize);
            assert_eq!(
                path_from_url_data(d).unwrap(),
                PathBuf::from("/tmp/a b.txt")
            );
            CFRelease(d);
        }
    }
}
