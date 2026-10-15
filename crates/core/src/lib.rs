//! Portable decision logic. No OS types, no sockets, no clocks, so it runs
//! deterministically under tests.

pub mod caps_lock;
pub mod desktop;
pub mod keys;

pub use desktop::{Desktop, EDGE_INSET, Side};
