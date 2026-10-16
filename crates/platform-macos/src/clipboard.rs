//! NSPasteboard access through the Objective-C runtime, usable from any
//! thread. Every call runs inside its own autorelease pool so long-lived
//! threads (the engine) do not accumulate autoreleased objects.
//!
//! NSString / NSData / NSArray are toll-free bridged with their CF
//! counterparts, so values cross the boundary as CF types.

use std::ffi::{CStr, c_char, c_void};

use crate::sys::CFRelease;

type Id = *mut c_void;
type Sel = *const c_void;
type CFRef = *const c_void;

const UTF8: u32 = 0x0800_0100;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        alloc: CFRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: bool,
    ) -> CFRef;
    fn CFStringGetLength(s: CFRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(len: isize, encoding: u32) -> isize;
    fn CFStringGetCString(s: CFRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFDataCreate(alloc: CFRef, bytes: *const u8, len: isize) -> CFRef;
    fn CFDataGetLength(d: CFRef) -> isize;
    fn CFDataGetBytePtr(d: CFRef) -> *const u8;
    fn CFArrayGetCount(a: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFRef, i: isize) -> CFRef;
}

fn sel(name: &CStr) -> Sel {
    // SAFETY: NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's exact C signature.
macro_rules! send {
    ($recv:expr, $sel:literal $(, $arg:expr => $ty:ty)* ; -> $ret:ty) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $ret =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($recv, sel($sel) $(, $arg)*)
    }};
}

struct Pool(*mut c_void);
impl Pool {
    fn new() -> Self {
        // SAFETY: balanced by the pop in Drop on the same thread.
        Self(unsafe { objc_autoreleasePoolPush() })
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: token from the matching push.
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

struct Owned(CFRef);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: wraps exactly one Create result.
            unsafe { CFRelease(self.0) }
        }
    }
}

fn cf_string(s: &str) -> Owned {
    // SAFETY: valid UTF-8 bytes of the given length.
    Owned(unsafe {
        CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, false)
    })
}

/// # Safety
/// `s` must be a valid CFString / NSString.
unsafe fn rust_string(s: CFRef) -> Option<String> {
    // SAFETY: caller guarantees a valid string.
    unsafe {
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), UTF8) + 1;
        let mut buf = vec![0u8; usize::try_from(size).ok()?];
        if !CFStringGetCString(s, buf.as_mut_ptr().cast(), size, UTF8) {
            return None;
        }
        let len = buf.iter().position(|b| *b == 0)?;
        buf.truncate(len);
        String::from_utf8(buf).ok()
    }
}

/// A retained NSPasteboard.
#[derive(Debug)]
pub struct Pasteboard {
    pb: Id,
    unique: bool,
}

// SAFETY: NSPasteboard talks to the pasteboard server and is used from
// background threads by AppKit itself; this type only sends it messages.
unsafe impl Send for Pasteboard {}
unsafe impl Sync for Pasteboard {}

impl Pasteboard {
    /// The system clipboard (Cmd+C / Cmd+V).
    pub fn general() -> Self {
        Self::create(c"generalPasteboard", false)
    }

    /// A private pasteboard, released globally on drop. For tests.
    pub fn with_unique_name() -> Self {
        Self::create(c"pasteboardWithUniqueName", true)
    }

    fn create(constructor: &CStr, unique: bool) -> Self {
        let _pool = Pool::new();
        // SAFETY: class methods returning an NSPasteboard; retained here and
        // released in Drop.
        unsafe {
            let class = objc_getClass(c"NSPasteboard".as_ptr());
            let ctor: unsafe extern "C" fn(Id, Sel) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let pb = ctor(class, sel(constructor));
            assert!(!pb.is_null(), "NSPasteboard unavailable");
            send!(pb, c"retain"; -> Id);
            Self { pb, unique }
        }
    }

    /// Bumped by every writer, including us. Only comparable on this Mac.
    pub fn change_count(&self) -> i64 {
        let _pool = Pool::new();
        // SAFETY: valid pasteboard; returns NSInteger.
        unsafe { send!(self.pb, c"changeCount"; -> isize) as i64 }
    }

    /// Type identifiers of the first item, in the writer's preference order.
    pub fn types(&self) -> Vec<String> {
        let _pool = Pool::new();
        // SAFETY: valid pasteboard; the array is autoreleased and only read
        // inside the pool.
        unsafe {
            let array = send!(self.pb, c"types"; -> Id);
            if array.is_null() {
                return Vec::new();
            }
            (0..CFArrayGetCount(array))
                .filter_map(|i| rust_string(CFArrayGetValueAtIndex(array, i)))
                .collect()
        }
    }

    pub fn data(&self, ty: &str) -> Option<Vec<u8>> {
        let _pool = Pool::new();
        let ty = cf_string(ty);
        // SAFETY: valid pasteboard and NSString; the NSData is autoreleased
        // and copied out inside the pool.
        unsafe {
            let data = send!(self.pb, c"dataForType:", ty.0 => CFRef; -> Id);
            if data.is_null() {
                return None;
            }
            let len = usize::try_from(CFDataGetLength(data)).ok()?;
            if len == 0 {
                return Some(Vec::new());
            }
            Some(std::slice::from_raw_parts(CFDataGetBytePtr(data), len).to_vec())
        }
    }

    /// Replaces the contents with one item carrying every representation and
    /// returns the resulting change count.
    pub fn write(&self, items: &[(String, Vec<u8>)]) -> Result<i64, String> {
        let _pool = Pool::new();
        // SAFETY: valid pasteboard; every CF object created here is released.
        unsafe {
            send!(self.pb, c"clearContents"; -> isize);
            for (ty, bytes) in items {
                let ty = cf_string(ty);
                let data = Owned(CFDataCreate(
                    std::ptr::null(),
                    bytes.as_ptr(),
                    bytes.len() as isize,
                ));
                if data.0.is_null()
                    || !send!(self.pb, c"setData:forType:", data.0 => CFRef, ty.0 => CFRef; -> bool)
                {
                    return Err("无法写入剪贴板".into());
                }
            }
            Ok(send!(self.pb, c"changeCount"; -> isize) as i64)
        }
    }
}

impl Drop for Pasteboard {
    fn drop(&mut self) {
        let _pool = Pool::new();
        // SAFETY: balances the retain in create().
        unsafe {
            if self.unique {
                send!(self.pb, c"releaseGlobally"; -> ());
            }
            send!(self.pb, c"release"; -> ());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_pasteboard_round_trips_several_representations() {
        let pb = Pasteboard::with_unique_name();
        let before = pb.change_count();
        let items = vec![
            (
                "public.utf8-plain-text".to_string(),
                "你好 clipboard".as_bytes().to_vec(),
            ),
            ("public.html".to_string(), b"<b>hi</b>".to_vec()),
            ("public.png".to_string(), vec![0x89, b'P', b'N', b'G', 0, 1]),
        ];
        let after = pb.write(&items).unwrap();
        assert_ne!(after, before);
        assert_eq!(pb.change_count(), after);
        for (ty, bytes) in &items {
            assert!(pb.types().contains(ty), "{ty} missing");
            assert_eq!(pb.data(ty).as_ref(), Some(bytes));
        }
        assert_eq!(pb.data("public.rtf"), None);
    }

    #[test]
    fn general_pasteboard_is_readable_without_writing() {
        let pb = Pasteboard::general();
        let count = pb.change_count();
        let _ = pb.types();
        assert!(pb.change_count() >= count);
    }
}
