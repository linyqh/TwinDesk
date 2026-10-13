//! Capability negotiation (report §5.1, §17.1).
//!
//! Capabilities come from adapter probes at runtime, never from a lookup on
//! the platform name. What a peer may actually do is the intersection of what
//! is available and what the user allowed for that peer.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    PermissionRequired,
    Unsupported,
    TemporarilyUnavailable,
    Experimental,
}

impl CapabilityState {
    /// Whether the capability may be exercised right now. `Experimental`
    /// requires an explicit opt-in that lives outside this type.
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Dotted capability name, e.g. `input.keyboard.physical`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CapabilityName(pub String);

impl CapabilityName {
    pub const INPUT_KEYBOARD_PHYSICAL: &str = "input.keyboard.physical";
    pub const INPUT_POINTER_ABSOLUTE: &str = "input.pointer.absolute";
    pub const INPUT_POINTER_RELATIVE: &str = "input.pointer.relative";
    pub const INPUT_SCROLL_PRECISE: &str = "input.scroll.precise";
    pub const INPUT_CAPTURE: &str = "input.capture";
    pub const CLIPBOARD_TEXT: &str = "clipboard.text";
    pub const DRAG_FILES_TARGET: &str = "drag.files.target";
    pub const DISPLAY_INPUT_DDC: &str = "display.input.ddc";
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub name: CapabilityName,
    pub version: u32,
    pub state: CapabilityState,
    /// Human-readable reason when the state is not `Available`.
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_available_is_usable() {
        assert!(CapabilityState::Available.is_usable());
        assert!(!CapabilityState::Experimental.is_usable());
        assert!(!CapabilityState::PermissionRequired.is_usable());
    }

    #[test]
    fn state_serializes_snake_case() {
        let json = serde_json::to_string(&CapabilityState::PermissionRequired).unwrap();
        assert_eq!(json, "\"permission_required\"");
    }
}
