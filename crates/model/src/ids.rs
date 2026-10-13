//! Identifiers. Devices are identified by their public key, never by address.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable device identity, derived from the SHA-256 of the device's public key.
///
/// IP addresses and display names are routing hints only; two peers with the
/// same name but different keys are different devices.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    /// Short fingerprint for logs and UI. Never log anything more identifying.
    pub fn short(&self) -> String {
        self.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

/// One OS interactive session of a device. A logout or reboot creates a new
/// session, and control leases never carry over to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

/// Random value chosen at agent start, used to reject messages from a
/// previous run of the same session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BootId(pub u64);

/// Monotonic control lease epoch. A target only accepts input carrying its
/// current epoch, and the epoch never goes backwards.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct LeaseEpoch(pub u64);

impl LeaseEpoch {
    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("lease epoch overflow"))
    }
}

/// Bumped on every hot-plug, rotation, scale or arrangement change. Input
/// computed against an old revision is never applied to a new layout.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct TopologyRevision(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_fingerprint_is_first_four_bytes() {
        let mut raw = [0u8; 32];
        raw[..4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(DeviceId(raw).short(), "deadbeef");
    }

    #[test]
    fn epoch_is_monotonic() {
        let e = LeaseEpoch::default();
        assert!(e.next() > e);
    }
}
