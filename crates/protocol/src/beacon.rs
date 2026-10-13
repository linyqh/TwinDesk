//! UDP discovery beacons.

use prost::Message;

use crate::PROTOCOL_MAJOR;
use crate::v1::Beacon;

/// "OSM1" in ASCII.
pub const BEACON_MAGIC: u32 = 0x4F53_4D31;

/// Beacons are tiny; anything bigger is not ours.
const MAX_BEACON: usize = 512;

pub fn encode_beacon(device_id: u64, name: &str, tcp_port: u16) -> Vec<u8> {
    Beacon {
        magic: BEACON_MAGIC,
        protocol_major: PROTOCOL_MAJOR,
        device_id,
        name: name.chars().take(64).collect(),
        tcp_port: tcp_port.into(),
    }
    .encode_to_vec()
}

/// Returns `None` for foreign, oversized or incompatible datagrams.
pub fn decode_beacon(datagram: &[u8]) -> Option<Beacon> {
    if datagram.len() > MAX_BEACON {
        return None;
    }
    let beacon = Beacon::decode(datagram).ok()?;
    let port_ok = (1..=u16::MAX as u32).contains(&beacon.tcp_port);
    (beacon.magic == BEACON_MAGIC && beacon.protocol_major == PROTOCOL_MAJOR && port_ok)
        .then_some(beacon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let b = decode_beacon(&encode_beacon(42, "Mac mini", 49001)).unwrap();
        assert_eq!(
            (b.device_id, b.name.as_str(), b.tcp_port),
            (42, "Mac mini", 49001)
        );
    }

    #[test]
    fn rejects_foreign_datagrams() {
        assert!(decode_beacon(b"hello world").is_none());
        assert!(decode_beacon(&[]).is_none());
        let mut wrong = Beacon::decode(&encode_beacon(1, "x", 1)[..]).unwrap();
        wrong.magic = 0;
        assert!(decode_beacon(&wrong.encode_to_vec()).is_none());
    }
}
