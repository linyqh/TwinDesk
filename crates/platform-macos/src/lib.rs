//! macOS platform adapter.
//!
//! The input hot path (event tap callback, suppression decision, injection)
//! lives here in Rust rather than in Swift, so the tap never crosses the FFI
//! boundary. See docs/adr/0001-input-hot-path-in-rust.md.

#![cfg(target_os = "macos")]

pub mod input;
pub mod permissions;
mod sys;
pub mod thunderbolt;

pub use input::{
    Capture, CaptureError, CaptureMode, CursorLock, Injector, InputEvent, display_rects,
};
pub use permissions::{PermissionSnapshot, probe, request_input_monitoring, request_post_events};
pub use thunderbolt::{BridgeStatus, ThunderboltPeer, ThunderboltPort, ThunderboltStatus};
