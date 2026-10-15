//! Target-local input-source selection. TIS calls run on the app's main queue.
//! The engine waits for completion before injecting the next character; the
//! event tap never waits. A deadline prevents UI shutdown from deadlocking it.

use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

use crate::sys::CFRelease;
use osm_core::caps_lock::{InputSource, SourceHistory};

type Ref = *const c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> Ref;
    fn TISCreateInputSourceList(properties: Ref, all_installed: bool) -> Ref;
    fn TISGetInputSourceProperty(source: Ref, property: Ref) -> Ref;
    fn TISSelectInputSource(source: Ref) -> i32;
    static kTISPropertyInputSourceID: Ref;
    static kTISPropertyInputSourceCategory: Ref;
    static kTISCategoryKeyboardInputSource: Ref;
    static kTISPropertyInputSourceIsSelectCapable: Ref;
    static kTISPropertyInputSourceIsEnabled: Ref;
    static kTISPropertyInputSourceIsASCIICapable: Ref;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFBooleanGetValue(value: Ref) -> bool;
    fn CFEqual(a: Ref, b: Ref) -> u8;
    fn CFStringGetLength(string: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(string: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFStringCreateWithCString(allocator: Ref, string: *const c_char, encoding: u32) -> Ref;
    fn CFPreferencesGetAppBooleanValue(key: Ref, application: Ref, valid: *mut bool) -> bool;
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

const UTF8: u32 = 0x0800_0100;
struct Owned(Ref);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: each Owned wraps a Create/Copy result exactly once.
            unsafe { CFRelease(self.0) };
        }
    }
}

fn string(value: &str) -> Owned {
    let text = CString::new(value).expect("constant without NUL");
    // SAFETY: valid NUL-terminated UTF-8, default allocator.
    Owned(unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) })
}

unsafe fn text(value: Ref) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: caller passes a CFString property held by a live TIS source.
    unsafe {
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8) + 1;
        if size <= 0 {
            return None;
        }
        let mut bytes = vec![0u8; size as usize];
        if !CFStringGetCString(value, bytes.as_mut_ptr().cast(), size, UTF8) {
            return None;
        }
        let end = bytes.iter().position(|&c| c == 0)?;
        String::from_utf8(bytes[..end].to_vec()).ok()
    }
}

unsafe fn boolean(source: Ref, key: Ref) -> bool {
    // SAFETY: documented TIS boolean property on a live source.
    unsafe {
        let value = TISGetInputSourceProperty(source, key);
        !value.is_null() && CFBooleanGetValue(value)
    }
}

/// False means Caps Lock is configured as a lock, or no Latin/non-Latin pair
/// exists. In either case the caller uses normal Caps Lock behavior.
fn switch_on_main(cancelled: &AtomicBool) -> Result<bool, String> {
    let app = string("com.apple.HIToolbox");
    let key = string("AppleCapsLockPressAndHoldToggleOff");
    let mut valid = false;
    // SAFETY: valid CF strings and output pointer. An absent preference uses
    // the standard Caps Lock language-switch behavior when a pair exists.
    if unsafe { CFPreferencesGetAppBooleanValue(key.0, app.0, &mut valid) } && valid {
        return Ok(false);
    }
    // SAFETY: this function is only called by the main-queue callback.
    unsafe {
        let current = Owned(TISCopyCurrentKeyboardInputSource());
        let list = Owned(TISCreateInputSourceList(std::ptr::null(), false));
        if current.0.is_null() || list.0.is_null() {
            return Err("无法读取本机输入源".into());
        }
        let current_id = text(TISGetInputSourceProperty(
            current.0,
            kTISPropertyInputSourceID,
        ))
        .ok_or("无法读取当前输入源标识")?;
        let mut sources = Vec::new();
        let mut refs = Vec::new();
        for i in 0..CFArrayGetCount(list.0) {
            let source = CFArrayGetValueAtIndex(list.0, i);
            let category = TISGetInputSourceProperty(source, kTISPropertyInputSourceCategory);
            if category.is_null()
                || CFEqual(category, kTISCategoryKeyboardInputSource) == 0
                || !boolean(source, kTISPropertyInputSourceIsSelectCapable)
                || !boolean(source, kTISPropertyInputSourceIsEnabled)
            {
                continue;
            }
            if let Some(id) = text(TISGetInputSourceProperty(source, kTISPropertyInputSourceID)) {
                sources.push(InputSource {
                    id,
                    ascii: boolean(source, kTISPropertyInputSourceIsASCIICapable),
                });
                refs.push(source);
            }
        }
        static HISTORY: OnceLock<Mutex<SourceHistory>> = OnceLock::new();
        let target = HISTORY
            .get_or_init(|| Mutex::new(SourceHistory::default()))
            .lock()
            .map_err(|_| "输入源状态不可用")?
            .target(&current_id, &sources);
        let Some(target) = target else {
            return Ok(false);
        };
        let index = sources
            .iter()
            .position(|s| s.id == target)
            .ok_or("目标输入源已移除")?;
        if cancelled.load(Ordering::Acquire) {
            return Err("输入源切换已取消".into());
        }
        let status = TISSelectInputSource(refs[index]);
        if status != 0 {
            return Err(format!("切换输入源失败（{status}）"));
        }
        let selected = Owned(TISCopyCurrentKeyboardInputSource());
        if selected.0.is_null()
            || text(TISGetInputSourceProperty(
                selected.0,
                kTISPropertyInputSourceID,
            ))
            .as_deref()
                != Some(&target)
        {
            return Err("系统未切换到目标输入源".into());
        }
        tracing::debug!(from = %current_id, to = %target, "switched target input source");
        Ok(true)
    }
}

struct Request {
    cancelled: Arc<AtomicBool>,
    result: mpsc::Sender<Result<bool, String>>,
}

extern "C" fn perform(context: *mut c_void) {
    // SAFETY: switch_input_source transfers exactly one Box to dispatch.
    let request = unsafe { Box::from_raw(context.cast::<Request>()) };
    if request.cancelled.load(Ordering::Acquire) {
        return;
    }
    // Never unwind over a C ABI boundary, including if a platform call fails.
    let result = std::panic::catch_unwind(|| switch_on_main(&request.cancelled))
        .unwrap_or_else(|_| Err("切换输入源时发生内部错误".into()));
    let _ = request.result.send(result);
}

/// Call from the engine thread only, never the main thread or event tap.
/// Queued work that has not started by the deadline is cancelled.
pub fn switch_input_source() -> Result<bool, String> {
    let (tx, rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let request = Box::new(Request {
        cancelled: cancelled.clone(),
        result: tx,
    });
    // SAFETY: _dispatch_main_q is libdispatch's public main-queue object
    // (the dispatch_get_main_queue inline accessor returns its address).
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q),
            Box::into_raw(request).cast(),
            perform,
        )
    };
    match rx.recv_timeout(Duration::from_millis(750)) {
        Ok(result) => result,
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            Err("输入源切换超时，已停止远程控制".into())
        }
    }
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(port: u32, matching: *mut c_void) -> u32;
    fn IOServiceOpen(service: u32, task: u32, kind: u32, connection: *mut u32) -> i32;
    fn IOServiceClose(connection: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOHIDGetModifierLockState(connection: u32, selector: i32, state: *mut bool) -> i32;
    fn IOHIDSetModifierLockState(connection: u32, selector: i32, state: bool) -> i32;
}
unsafe extern "C" {
    static mach_task_self_: u32;
}

/// Change the target's real Caps Lock state, so it survives return to local
/// control and subsequent injected characters use the same state as hardware.
pub fn set_caps_lock(enabled: bool) -> Result<(), String> {
    with_hid_connection(|connection| {
        // SAFETY: live IOHID parameter connection and documented selector.
        let status = unsafe { IOHIDSetModifierLockState(connection, 1, enabled) };
        if status != 0 {
            return Err(format!("修改大写锁定失败（{status}）"));
        }
        Ok(())
    })
}

pub fn caps_lock_state() -> Result<bool, String> {
    with_hid_connection(|connection| {
        let mut state = false;
        // SAFETY: live connection, valid output pointer and Caps Lock selector.
        let status = unsafe { IOHIDGetModifierLockState(connection, 1, &mut state) };
        if status != 0 {
            return Err(format!("读取大写锁定失败（{status}）"));
        }
        Ok(state)
    })
}

fn with_hid_connection<T>(action: impl FnOnce(u32) -> Result<T, String>) -> Result<T, String> {
    // SAFETY: matching dictionary is consumed; service and connection are
    // released on all paths. Constants are from IOHIDShared/IOHIDParameter.h.
    unsafe {
        let matching = IOServiceMatching(c"IOHIDSystem".as_ptr());
        if matching.is_null() {
            return Err("无法查询大写锁定服务".into());
        }
        let service = IOServiceGetMatchingService(0, matching);
        if service == 0 {
            return Err("无法访问大写锁定服务".into());
        }
        let mut connection = 0;
        let status = IOServiceOpen(service, mach_task_self_, 1, &mut connection);
        IOObjectRelease(service);
        if status != 0 {
            return Err(format!("无法访问大写锁定状态（{status}）"));
        }
        let result = action(connection);
        IOServiceClose(connection);
        result
    }
}
