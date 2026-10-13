//! macOS platform adapter.
//!
//! The input hot path (event tap callback, suppression decision, injection)
//! lives here in Rust rather than in Swift, so the tap never crosses the FFI
//! boundary or the Swift concurrency checker. AppKit-bound work (pasteboard,
//! drag sessions, overlays) stays in the Swift app.
//!
//! On other targets every probe reports "not granted" so the workspace still
//! builds and tests there.

pub mod permissions;

pub use permissions::{PermissionSnapshot, probe, request_input_monitoring, request_post_events};
