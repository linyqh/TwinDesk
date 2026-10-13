//! Portable decision logic. No OS types, no sockets, no clocks: every input is
//! an explicit event and every side effect is a returned action, so the whole
//! crate runs deterministically under tests and replayed traces.

pub mod session;

pub use session::{Action, ControlState, Event, ForwardMode, SessionCore};
