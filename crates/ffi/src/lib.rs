//! Control-plane API exported to Swift through UniFFI.
//!
//! Contract (report §15.2): strings are UTF-8, UniFFI owns buffer lifetimes,
//! and Rust panics are caught at the boundary and surfaced as errors. Nothing
//! on the input hot path goes through here.

use std::sync::Mutex;

use osm_core::SessionCore;
use osm_model::CapabilityState;

uniffi::setup_scaffolding!();

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PermissionState {
    Granted,
    Missing,
    TemporarilyBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PermissionItem {
    pub capability: String,
    pub state: PermissionState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PermissionReport {
    pub input_monitoring: bool,
    pub post_events: bool,
    pub accessibility: bool,
    pub secure_input_active: bool,
    pub items: Vec<PermissionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BuildInfo {
    pub version: String,
    pub protocol_major: u32,
    pub protocol_minor: u32,
    pub default_port: u16,
    pub service_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ControlStatus {
    Local,
    Paused,
    Switching,
    Controlling,
    Controlled,
    Recovering,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum OsmError {
    #[error("{message}")]
    Internal { message: String },
}

#[uniffi::export]
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_major: osm_protocol::PROTOCOL_MAJOR,
        protocol_minor: osm_protocol::PROTOCOL_MINOR,
        default_port: osm_transport::DEFAULT_PORT,
        service_type: osm_transport::SERVICE_TYPE.to_string(),
    }
}

#[uniffi::export]
pub fn probe_permissions() -> PermissionReport {
    let snap = osm_platform_macos::probe();
    let items = snap
        .capabilities()
        .into_iter()
        .map(|c| PermissionItem {
            capability: c.name.0,
            state: match c.state {
                CapabilityState::Available => PermissionState::Granted,
                CapabilityState::TemporarilyUnavailable => PermissionState::TemporarilyBlocked,
                _ => PermissionState::Missing,
            },
            reason: c.reason,
        })
        .collect();
    PermissionReport {
        input_monitoring: snap.input_monitoring,
        post_events: snap.post_events,
        accessibility: snap.accessibility,
        secure_input_active: snap.secure_input_active,
        items,
    }
}

/// Triggers the system prompts for the grants that are still missing.
#[uniffi::export]
pub fn request_permissions() {
    let snap = osm_platform_macos::probe();
    if !snap.input_monitoring {
        osm_platform_macos::request_input_monitoring();
    }
    if !snap.post_events {
        osm_platform_macos::request_post_events();
    }
}

/// The agent owning one OS session. Grows networking and the input pipeline
/// in P1; for now it only exposes the control state for the UI.
#[derive(Debug, uniffi::Object)]
pub struct Agent {
    session: Mutex<SessionCore>,
}

#[uniffi::export]
impl Agent {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            session: Mutex::new(SessionCore::new()),
        }
    }

    pub fn status(&self) -> Result<ControlStatus, OsmError> {
        use osm_core::ControlState as S;
        let session = self.session.lock().map_err(|_| OsmError::Internal {
            message: "session lock poisoned".into(),
        })?;
        Ok(match session.state() {
            S::Local => ControlStatus::Local,
            S::Paused => ControlStatus::Paused,
            S::Preparing { .. } => ControlStatus::Switching,
            S::Sending(_) => ControlStatus::Controlling,
            S::Receiving(_) => ControlStatus::Controlled,
            S::Recovering => ControlStatus::Recovering,
        })
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_agent_starts_local() {
        assert_eq!(Agent::new().status().unwrap(), ControlStatus::Local);
    }

    #[test]
    fn build_info_matches_crates() {
        let info = build_info();
        assert_eq!(info.protocol_major, osm_protocol::PROTOCOL_MAJOR);
        assert_eq!(info.service_type, "_opensharemouse._udp");
    }
}
