//! Explicit, interactive platform probe (not part of cargo test). Temporarily
//! switches input sources and toggles Caps Lock, then restores both. See P0-11.
use osm_platform_macos::FnRemap;
use osm_platform_macos::fn_remap::keyboards_with_our_mapping;
use osm_platform_macos::input_source::{caps_lock_state, set_caps_lock, switch_input_source};
use std::ffi::c_void;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: bool) -> i32;
    fn CFRelease(value: *const c_void);
}
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
    fn TISSelectInputSource(source: *const c_void) -> i32;
}

fn main() {
    // SAFETY: main-thread TIS query; held until worker completion.
    let original = unsafe { TISCopyCurrentKeyboardInputSource() };
    assert!(!original.is_null());
    let locked = caps_lock_state().expect("read Caps Lock");
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            // Do not disturb an existing forwarding session.
            if keyboards_with_our_mapping().iter().any(|&mapped| mapped) {
                return Err("Return control to this Mac before running the probe".into());
            }
            let mut remap = FnRemap::default();
            let count = remap.engage(true);
            let mapped = keyboards_with_our_mapping()
                .into_iter()
                .filter(|&m| m)
                .count();
            remap.release();
            let restored = keyboards_with_our_mapping().iter().all(|&m| !m);
            println!("keyboard mappings: engaged={count}, observed={mapped}, restored={restored}");
            if count == 0 || mapped != count || !restored {
                return Err("No eligible keyboard, or mapping round trip failed".into());
            }
            let start = Instant::now();
            let first = switch_input_source()?;
            let second = switch_input_source()?;
            println!(
                "input-source switches: {first}, {second}; elapsed: {:?}",
                start.elapsed()
            );
            if !first || !second {
                return Err(
                    "No enabled language-switch pair, or Caps Lock switching is disabled".into(),
                );
            }
            set_caps_lock(!locked)?;
            std::thread::sleep(Duration::from_millis(100));
            let actual = caps_lock_state().expect("read Caps Lock");
            println!("Caps Lock: original={locked}, toggled={actual}");
            if actual == locked {
                return Err("HID lock state did not change".into());
            }
            Ok(())
        })();
        let restore = set_caps_lock(locked);
        let _ = tx.send(result.and(restore));
    });
    let result = loop {
        if let Ok(result) = rx.try_recv() {
            break result;
        }
        // SAFETY: main thread services libdispatch for the TIS requests.
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, false);
        }
    };
    worker.join().expect("probe worker");
    // SAFETY: restore the retained original source before releasing it.
    let restore = unsafe {
        let status = TISSelectInputSource(original);
        CFRelease(original);
        status
    };
    println!(
        "restore input source: {restore}; Caps Lock restored: {}",
        (caps_lock_state().expect("read Caps Lock")) == locked
    );
    if let Err(error) = result {
        eprintln!("FAIL: {error}");
        std::process::exit(1);
    }
    assert_eq!(restore, 0);
    println!("PASS: target platform operations (not a two-Mac acceptance test)");
}
