//! OSM/1 wire protocol. Not compatible with ShareMouse in any way (ADR-01).

pub mod frame;

/// Generated from `schema/osm/v1/osm.proto`.
#[allow(missing_debug_implementations, clippy::all)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/osm.v1.rs"));
}

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 0;

/// ALPN for the QUIC/TLS handshake. Bumped together with `PROTOCOL_MAJOR`.
pub const ALPN: &[u8] = b"osm/1";

pub use frame::{FrameError, MAX_CONTROL_FRAME, decode_frame, encode_frame};
