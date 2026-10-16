//! macOS platform adapter.
//!
//! The input hot path (event tap callback, suppression decision, injection)
//! lives here in Rust rather than in Swift, so the tap never crosses the FFI
//! boundary. See docs/adr/0001-input-hot-path-in-rust.md.

#![cfg(target_os = "macos")]

pub mod clipboard;
pub mod drag_pasteboard;
pub mod fn_remap;
pub mod input;
pub mod input_source;
pub mod permissions;
mod sys;
pub mod thunderbolt;

pub use clipboard::Pasteboard;
pub use drag_pasteboard::DragPasteboard;
pub use fn_remap::{FnRemap, REMAPPED_CAPS_LOCK_KEYCODE, REMAPPED_FN_KEYCODE};
pub use input::{
    Capture, CaptureError, CaptureMode, CursorLock, Injector, InputEvent, UntrackedLeft,
    cursor_position, display_rects, physical_left_down,
};
pub use permissions::{PermissionSnapshot, probe, request_input_monitoring, request_post_events};
pub use thunderbolt::{BridgeStatus, ThunderboltPeer, ThunderboltPort, ThunderboltStatus};
