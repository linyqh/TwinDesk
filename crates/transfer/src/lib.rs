//! Data exchange: on-demand clipboard and file transfer.
//!
//! Only manifest path validation exists so far; offers, staging and resume
//! land with CLIP-01 and FILE-01.

pub mod path;

pub use path::{PathError, validate_relative_path};
