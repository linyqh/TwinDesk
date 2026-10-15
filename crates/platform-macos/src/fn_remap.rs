//! Keeps the local Fn and Caps Lock keys away from local apps while forwarding.
//!
//! Some apps (WeChat's WeType input method is the motivating case) watch the
//! Fn/Globe key with their own event tap, install it at the head of the HID
//! tap chain, swallow the event, and re-create the tap within seconds if
//! another tap gets in front. Tap order is therefore not something we can
//! win, and while this Mac controls the peer, pressing Fn would trigger the
//! local app instead of being forwarded.
//!
//! The fix acts below the tap chain: while forwarding, the HID event system
//! remaps physical Fn to F19 (`UserKeyMapping`, what `hidutil` sets). No tap
//! ever sees an Fn press; our tap sees F19 and forwards it as Fn. Measured on
//! macOS 27 with WeType 3.0.0: injected F19 does not trigger WeType, injected
//! Fn does, and a remapped physical Fn arrives at the tail of the chain as
//! F19 (keycode 80).
//!
//! With a minor-3 peer, also remap Caps Lock to F18. Its genuine down/up
//! edges become an epoch-scoped CapsLock message; input-source selection
//! and the latched lock state are handled exclusively on the target.
//!
//! Only keyboard services without a user mapping of their own are touched,
//! and only those are cleared again, so an existing remap (hidutil,
//! Karabiner-style tools) is never overwritten.

use std::ffi::c_void;

use crate::sys::{CFRelease, CFStringRef};

/// macOS virtual keycode of F19 as delivered after the remap.
pub const REMAPPED_FN_KEYCODE: u16 = 80;
/// Reserved while controlling a peer supporting physical Caps Lock (minor 3).
pub const REMAPPED_CAPS_LOCK_KEYCODE: u16 = 79; // F18

/// HID usages, encoded as `page << 32 | usage`.
const HID_FN_APPLE_VENDOR: i64 = 0xFF_0000_0003; // AppleVendor top case, keyboard Fn
const HID_FN_APPLE_VENDOR_KEYBOARD: i64 = 0xFF01_0000_0003; // AppleVendor keyboard page, Fn
const HID_F19: i64 = 0x7_0000_006E;
const HID_CAPS_LOCK: i64 = 0x7_0000_0039;
const HID_F18: i64 = 0x7_0000_006D;

const USAGE_PAGE_GENERIC_DESKTOP: u32 = 1;
const USAGE_KEYBOARD: u32 = 6;

type IOHIDEventSystemClientRef = *mut c_void;
type IOHIDServiceClientRef = *mut c_void;
type CFArrayRef = *const c_void;
type CFTypeRef = *const c_void;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreateSimpleClient(
        allocator: *const c_void,
    ) -> IOHIDEventSystemClientRef;
    fn IOHIDEventSystemClientCopyServices(client: IOHIDEventSystemClientRef) -> CFArrayRef;
    fn IOHIDServiceClientConformsTo(service: IOHIDServiceClientRef, page: u32, usage: u32) -> i32;
    fn IOHIDServiceClientSetProperty(
        service: IOHIDServiceClientRef,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> u8;
    fn IOHIDServiceClientCopyProperty(
        service: IOHIDServiceClientRef,
        key: CFStringRef,
    ) -> CFTypeRef;
    fn IOHIDServiceClientGetRegistryID(service: IOHIDServiceClientRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFArrayGetCount(array: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: isize) -> *const c_void;
    fn CFArrayCreate(
        alloc: *const c_void,
        values: *const *const c_void,
        n: isize,
        cb: *const c_void,
    ) -> CFArrayRef;
    fn CFDictionaryCreate(
        alloc: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        n: isize,
        key_cb: *const c_void,
        value_cb: *const c_void,
    ) -> CFTypeRef;
    fn CFNumberCreate(alloc: *const c_void, number_type: isize, value: *const c_void) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, number_type: isize, value: *mut c_void) -> u8;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const libc::c_char,
        enc: u32,
    ) -> CFStringRef;
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
}

const K_CF_NUMBER_SINT64: isize = 4;
const UTF8: u32 = 0x0800_0100;

/// Owned CF object, released on drop.
struct Cf(*const c_void);

impl Cf {
    fn string(s: &std::ffi::CStr) -> Self {
        // SAFETY: valid C string; result released on drop.
        Self(unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) })
    }
    fn number(v: i64) -> Self {
        // SAFETY: valid pointer to an i64; result released on drop.
        Self(unsafe {
            CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_SINT64,
                (&v as *const i64).cast(),
            )
        })
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own this reference.
            unsafe { CFRelease(self.0) };
        }
    }
}

struct Keys {
    user_mapping: Cf,
    src: Cf,
    dst: Cf,
}

impl Keys {
    fn new() -> Self {
        Self {
            user_mapping: Cf::string(c"UserKeyMapping"),
            src: Cf::string(c"HIDKeyboardModifierMappingSrc"),
            dst: Cf::string(c"HIDKeyboardModifierMappingDst"),
        }
    }

    fn entry(&self, src: i64, dst: i64) -> Cf {
        let (s, d) = (Cf::number(src), Cf::number(dst));
        let keys = [self.src.0, self.dst.0];
        let values = [s.0, d.0];
        // SAFETY: two valid keys/values with CFType callbacks (retained).
        Cf(unsafe {
            CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        })
    }

    fn array(items: &[&Cf]) -> Cf {
        let raw: Vec<*const c_void> = items.iter().map(|c| c.0).collect();
        // SAFETY: valid CF objects with CFType callbacks (retained).
        Cf(unsafe {
            CFArrayCreate(
                std::ptr::null(),
                raw.as_ptr(),
                raw.len() as isize,
                &kCFTypeArrayCallBacks,
            )
        })
    }

    /// Both Fn encodings to F19; optionally Caps Lock to F18.
    fn ours(&self, caps_lock: bool) -> Cf {
        let a = self.entry(HID_FN_APPLE_VENDOR, HID_F19);
        let b = self.entry(HID_FN_APPLE_VENDOR_KEYBOARD, HID_F19);
        let caps = self.entry(HID_CAPS_LOCK, HID_F18);
        if caps_lock {
            Self::array(&[&a, &b, &caps])
        } else {
            Self::array(&[&a, &b])
        }
    }
}

/// Keyboard services currently known to the HID event system.
struct Services {
    /// Held only to keep the service clients in `list` valid.
    _client: Cf,
    list: Cf,
}

impl Services {
    fn keyboards() -> Option<Self> {
        // SAFETY: plain constructor; released by Cf.
        let client = Cf(unsafe { IOHIDEventSystemClientCreateSimpleClient(std::ptr::null()) });
        if client.0.is_null() {
            return None;
        }
        // SAFETY: valid client; array released by Cf.
        let list = Cf(unsafe { IOHIDEventSystemClientCopyServices(client.0 as _) });
        if list.0.is_null() {
            return None;
        }
        Some(Self {
            _client: client,
            list,
        })
    }

    fn iter(&self) -> impl Iterator<Item = IOHIDServiceClientRef> + '_ {
        // SAFETY: list is a valid CFArray of service clients.
        let n = unsafe { CFArrayGetCount(self.list.0) };
        (0..n).filter_map(move |i| {
            // SAFETY: index in bounds; the array keeps the element alive.
            let s = unsafe { CFArrayGetValueAtIndex(self.list.0, i) } as IOHIDServiceClientRef;
            // SAFETY: valid service client.
            let is_kb = unsafe {
                IOHIDServiceClientConformsTo(s, USAGE_PAGE_GENERIC_DESKTOP, USAGE_KEYBOARD)
            } != 0;
            is_kb.then_some(s)
        })
    }
}

fn registry_id(service: IOHIDServiceClientRef) -> Option<u64> {
    // SAFETY: valid service; the returned number is not owned (Get rule).
    let n = unsafe { IOHIDServiceClientGetRegistryID(service) };
    if n.is_null() {
        return None;
    }
    let mut v: i64 = 0;
    // SAFETY: n is a CFNumber; v is a valid out pointer.
    (unsafe { CFNumberGetValue(n, K_CF_NUMBER_SINT64, (&mut v as *mut i64).cast()) } != 0)
        .then_some(v as u64)
}

/// Current mapping on a service, or `None` when empty or unset.
fn current_mapping(keys: &Keys, service: IOHIDServiceClientRef) -> Option<Cf> {
    // SAFETY: valid service and key; result owned (Copy rule).
    let v = Cf(unsafe { IOHIDServiceClientCopyProperty(service, keys.user_mapping.0) });
    // SAFETY: only treated as an array when non-null.
    (!v.0.is_null() && unsafe { CFArrayGetCount(v.0) } > 0).then_some(v)
}

fn is_ours(keys: &Keys, mapping: &Cf) -> bool {
    // SAFETY: both are valid CF objects.
    unsafe {
        CFEqual(mapping.0, keys.ours(false).0) != 0 || CFEqual(mapping.0, keys.ours(true).0) != 0
    }
}

/// Remaps Fn and optionally Caps Lock while alive (between `engage` and `release`).
#[derive(Debug, Default)]
pub struct FnRemap {
    /// Registry ids of the services we set, so we only ever clear those.
    engaged: Vec<u64>,
}

impl FnRemap {
    pub fn is_engaged(&self) -> bool {
        !self.engaged.is_empty()
    }

    /// Remaps Fn to F19 on every keyboard without a user mapping. Returns
    /// how many keyboards were remapped. Repeated calls discover newly attached
    /// keyboards without touching existing user mappings.
    pub fn engage(&mut self, caps_lock: bool) -> usize {
        let Some(services) = Services::keyboards() else {
            return 0;
        };
        let keys = Keys::new();
        let ours = keys.ours(caps_lock);
        for s in services.iter() {
            let Some(id) = registry_id(s) else { continue };
            if self.engaged.contains(&id) || current_mapping(&keys, s).is_some() {
                continue;
            }
            // Get the id BEFORE setting a mapping: every change must be recoverable.
            // SAFETY: valid service, key and CFArray value.
            if unsafe { IOHIDServiceClientSetProperty(s, keys.user_mapping.0, ours.0) } != 0 {
                self.engaged.push(id);
            }
        }
        tracing::debug!(keyboards = self.engaged.len(), "keyboard remap engaged");
        self.engaged.len()
    }

    /// Clears the mapping on the keyboards we remapped.
    pub fn release(&mut self) {
        if self.engaged.is_empty() {
            return;
        }
        let ids = std::mem::take(&mut self.engaged);
        let Some(services) = Services::keyboards() else {
            return;
        };
        let keys = Keys::new();
        let empty = Keys::array(&[]);
        for s in services.iter() {
            let Some(id) = registry_id(s) else { continue };
            if !ids.contains(&id) {
                continue;
            }
            // Leave it alone if someone replaced our mapping meanwhile.
            if current_mapping(&keys, s).is_some_and(|m| is_ours(&keys, &m)) {
                // SAFETY: valid service, key and empty CFArray.
                unsafe { IOHIDServiceClientSetProperty(s, keys.user_mapping.0, empty.0) };
            }
        }
        tracing::debug!("keyboard remap released");
    }

    /// Removes a mapping identical to ours left behind by a crash. Mappings
    /// that differ in any way belong to someone else and stay.
    pub fn clear_stale() -> usize {
        let Some(services) = Services::keyboards() else {
            return 0;
        };
        let keys = Keys::new();
        let empty = Keys::array(&[]);
        let mut cleared = 0;
        for s in services.iter() {
            if current_mapping(&keys, s).is_some_and(|m| is_ours(&keys, &m)) {
                // SAFETY: valid service, key and empty CFArray.
                unsafe { IOHIDServiceClientSetProperty(s, keys.user_mapping.0, empty.0) };
                cleared += 1;
            }
        }
        cleared
    }
}

impl Drop for FnRemap {
    fn drop(&mut self) {
        self.release();
    }
}

/// Mapping state of every keyboard, for tests and diagnostics: whether each
/// one carries exactly our mapping.
pub fn keyboards_with_our_mapping() -> Vec<bool> {
    let Some(services) = Services::keyboards() else {
        return Vec::new();
    };
    let keys = Keys::new();
    services
        .iter()
        .map(|s| current_mapping(&keys, s).is_some_and(|m| is_ours(&keys, &m)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_mapping_is_recognised_as_ours() {
        let keys = Keys::new();
        assert!(is_ours(&keys, &keys.ours(true)));
        // Upgrading must also recover the previous Fn-only crash mapping.
        assert!(is_ours(&keys, &keys.ours(false)));
        let foreign_caps = Keys::array(&[&keys.entry(HID_CAPS_LOCK, HID_F18)]);
        assert!(!is_ours(&keys, &foreign_caps));
        let other = Keys::array(&[&keys.entry(HID_FN_APPLE_VENDOR, 0x7_0000_0004)]);
        assert!(!is_ours(&keys, &other));
    }

    /// Touches the real HID event system: engages, checks, releases. Runs
    /// without root; on machines without a keyboard it is a no-op.
    #[test]
    fn engage_and_release_round_trip() {
        let before = keyboards_with_our_mapping();
        assert!(
            before.iter().all(|m| !m),
            "stale mapping present before test"
        );
        let mut r = FnRemap::default();
        let n = r.engage(true);
        let during = keyboards_with_our_mapping();
        assert_eq!(during.iter().filter(|m| **m).count(), n);
        r.release();
        assert!(keyboards_with_our_mapping().iter().all(|m| !m));
    }

    #[test]
    fn drop_releases() {
        {
            let mut r = FnRemap::default();
            r.engage(true);
        }
        assert!(keyboards_with_our_mapping().iter().all(|m| !m));
    }
}
