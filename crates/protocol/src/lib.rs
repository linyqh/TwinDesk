//! OSM/1 wire protocol. Not compatible with ShareMouse in any way (ADR-01).

pub mod beacon;
pub mod bulk;
pub mod frame;

/// Generated from `schema/osm/v1/osm.proto`.
#[allow(missing_debug_implementations, clippy::all)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/osm.v1.rs"));
}

pub const PROTOCOL_MAJOR: u32 = 1;
/// 1: drag-and-drop (`DragOffer`, `DragEnd`) and bulk connections. Peers
/// below 1 drop the connection on unknown payloads, so drag offers are only
/// sent to peers that announced at least this minor.
pub const PROTOCOL_MINOR: u32 = 1;

pub use beacon::{BEACON_MAGIC, decode_beacon, encode_beacon};
pub use bulk::{CHUNK_SIZE, MAX_BULK_FRAME, read_bulk, write_bulk};
pub use frame::{FrameError, MAX_CONTROL_FRAME, decode_frame, encode_frame, envelope};
