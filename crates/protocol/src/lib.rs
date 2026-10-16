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
/// 2: explicit edge portals and receiver commit receipts. Only offer files
/// to peers advertising minor >= 2; minor 1 used native drag proxies.
/// 3: physical Caps Lock edges and target-local input-source switching.
/// 4: clipboard hand-off when the cursor switches screens.
pub const PROTOCOL_MINOR: u32 = 4;

pub use beacon::{BEACON_MAGIC, decode_beacon, encode_beacon};
pub use bulk::{CHUNK_SIZE, MAX_BULK_FRAME, read_bulk, write_bulk};
pub use frame::{FrameError, MAX_CONTROL_FRAME, decode_frame, encode_frame, envelope};
