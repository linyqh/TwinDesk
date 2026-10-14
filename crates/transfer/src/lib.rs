//! Data exchange: file transfer for cross-machine drag and drop (report §10).
//!
//! One bulk connection carries one dropped item. The receiver sends a
//! `FetchRequest`; the sender streams a manifest and then every file's
//! contents with a SHA-256 per file. The receiver writes into a staging
//! directory on the destination volume and commits the verified item with
//! one exclusive rename. Clipboard offers (CLIP-01) are not implemented yet.

mod error;
mod fsx;
pub mod manifest;
pub mod path;
mod progress;
pub mod receive;
pub mod send;

pub use error::TransferError;
pub use manifest::{describe_item, scan};
pub use path::{PathError, validate_macos_path, validate_relative_path};
pub use progress::{Phase, Progress, ProgressSnapshot};
