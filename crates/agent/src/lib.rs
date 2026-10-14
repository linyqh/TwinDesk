//! Two-machine keyboard and mouse sharing over a Thunderbolt cable.
//!
//! `Engine::start` spawns the worker threads and returns a handle the UI
//! polls for a `Status` snapshot. All session state is owned by one engine
//! thread; other threads only feed it messages.

pub mod drag;
mod engine;
mod link;
pub mod router;
pub mod transfers;

pub use engine::{Command, DragItemInfo, DragUi, Engine, LinkState, PeerInfo, Status};
pub use router::{EMERGENCY_FLAGS, Role};
pub use transfers::{Direction, TransferInfo, Transfers};
