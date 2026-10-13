//! TCC permission probes (report §13.3, §17.2.1).
//!
//! An active (filtering) event tap needs Accessibility; a listen-only tap
//! needs Input Monitoring; posting events has its own grant that shows up
//! under Accessibility. Preflight results can lag a revocation, so callers
//! must still treat a failed tap creation or a disabled tap as authoritative.

use osm_model::{Capability, CapabilityName, CapabilityState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PermissionSnapshot {
    /// `kTCCServiceListenEvent`: listen-only event taps.
    pub input_monitoring: bool,
    /// `kTCCServicePostEvent`: `CGEventPost` injection.
    pub post_events: bool,
    /// Full Accessibility trust, required for a filtering tap that can
    /// suppress local input while forwarding.
    pub accessibility: bool,
    /// Another process holds Secure Event Input (password field, Terminal
    /// "Secure Keyboard Entry"); keyboard capture is unreliable meanwhile.
    pub secure_input_active: bool,
}

impl PermissionSnapshot {
    pub fn capabilities(&self) -> Vec<Capability> {
        let cap = |name: &str, ok: bool, reason: &str| Capability {
            name: CapabilityName(name.to_string()),
            version: 1,
            state: if ok {
                CapabilityState::Available
            } else {
                CapabilityState::PermissionRequired
            },
            reason: (!ok).then(|| reason.to_string()),
        };
        let capture = if !self.accessibility {
            cap(
                CapabilityName::INPUT_CAPTURE,
                false,
                "Accessibility permission is required to capture and suppress input",
            )
        } else if self.secure_input_active {
            // Not a permission problem: it clears when the other app drops it.
            Capability {
                state: CapabilityState::TemporarilyUnavailable,
                reason: Some("Secure Event Input is active in another app".to_string()),
                ..cap(CapabilityName::INPUT_CAPTURE, true, "")
            }
        } else {
            cap(CapabilityName::INPUT_CAPTURE, true, "")
        };
        vec![
            capture,
            cap(
                CapabilityName::INPUT_KEYBOARD_PHYSICAL,
                self.post_events,
                "Event posting permission is required to inject keyboard input",
            ),
            cap(
                CapabilityName::INPUT_POINTER_ABSOLUTE,
                self.post_events,
                "Event posting permission is required to inject pointer input",
            ),
        ]
    }
}

pub fn probe() -> PermissionSnapshot {
    #[cfg(target_os = "macos")]
    {
        PermissionSnapshot {
            input_monitoring: sys::CGPreflightListenEventAccess(),
            post_events: sys::CGPreflightPostEventAccess(),
            accessibility: sys::AXIsProcessTrusted() != 0,
            secure_input_active: sys::IsSecureEventInputEnabled() != 0,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionSnapshot::default()
    }
}

/// Shows the system prompt the first time; afterwards the user has to flip
/// the switch in System Settings. A grant usually needs an app relaunch to
/// take effect for event taps.
pub fn request_input_monitoring() -> bool {
    #[cfg(target_os = "macos")]
    {
        sys::CGRequestListenEventAccess()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn request_post_events() -> bool {
    #[cfg(target_os = "macos")]
    {
        sys::CGRequestPostEventAccess()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
#[allow(non_snake_case)]
mod sys {
    // Darwin `Boolean` is an unsigned char; the CG preflight calls return C `bool`.
    type Boolean = u8;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub safe fn CGPreflightListenEventAccess() -> bool;
        pub safe fn CGRequestListenEventAccess() -> bool;
        pub safe fn CGPreflightPostEventAccess() -> bool;
        pub safe fn CGRequestPostEventAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        pub safe fn AXIsProcessTrusted() -> Boolean;
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        pub safe fn IsSecureEventInputEnabled() -> Boolean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_does_not_crash() {
        // The test binary is unsigned and never granted anything; we only
        // check that the symbols resolve and the calls return.
        let _ = probe();
    }

    #[test]
    fn secure_input_blocks_capture() {
        let snap = PermissionSnapshot {
            accessibility: true,
            post_events: true,
            input_monitoring: true,
            secure_input_active: true,
        };
        let capture = snap
            .capabilities()
            .into_iter()
            .find(|c| c.name.0 == CapabilityName::INPUT_CAPTURE)
            .unwrap();
        assert_eq!(capture.state, CapabilityState::TemporarilyUnavailable);
        assert!(capture.reason.unwrap().contains("Secure Event Input"));
    }
}
