//! Control-plane API exported to Swift through UniFFI.
//!
//! Strings are UTF-8, UniFFI owns buffer lifetimes, and nothing on the input
//! hot path goes through here: the UI only polls a status snapshot and sends
//! a handful of commands.

use osm_agent::{Command, Engine, LinkState as AgentLink, Role};
use osm_core::Side;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LinkState {
    NoCable,
    CableNoNetwork,
    Searching,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ControlRole {
    Local,
    Controlling,
    Controlled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenSide {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PortInfo {
    pub receptacle: String,
    pub connected: bool,
    pub speed: String,
    pub device_name: Option<String>,
    pub device_model: Option<String>,
    pub device_vendor: Option<String>,
    pub ip_service: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BridgeInfo {
    pub present: bool,
    pub up: bool,
    pub ipv4: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeerDetails {
    pub name: String,
    pub model: String,
    pub os_version: String,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AgentStatus {
    pub link: LinkState,
    pub ports: Vec<PortInfo>,
    pub bridge: BridgeInfo,
    pub hardware_error: Option<String>,
    pub peer: Option<PeerDetails>,
    pub role: ControlRole,
    pub peer_side: ScreenSide,
    pub sharing_enabled: bool,
    pub capture_error: Option<String>,
    pub last_error: Option<String>,
    /// Round-trip time to the peer in milliseconds.
    pub latency_ms: Option<f64>,
}

#[uniffi::export]
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_major: osm_protocol::PROTOCOL_MAJOR,
        protocol_minor: osm_protocol::PROTOCOL_MINOR,
        default_port: osm_transport::DEFAULT_PORT,
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

/// Owns the sharing engine for the lifetime of the app.
#[derive(Debug, uniffi::Object)]
pub struct Agent {
    engine: Engine,
}

#[uniffi::export]
impl Agent {
    #[uniffi::constructor]
    pub fn new(device_name: String) -> Self {
        Self {
            engine: Engine::start(device_name),
        }
    }

    pub fn status(&self) -> AgentStatus {
        let s = self.engine.status();
        AgentStatus {
            link: match s.link {
                AgentLink::NoCable => LinkState::NoCable,
                AgentLink::CableNoNetwork => LinkState::CableNoNetwork,
                AgentLink::Searching => LinkState::Searching,
                AgentLink::Connecting => LinkState::Connecting,
                AgentLink::Connected => LinkState::Connected,
            },
            ports: s
                .thunderbolt
                .ports
                .iter()
                .map(|p| PortInfo {
                    receptacle: p.receptacle.clone(),
                    connected: p.connected,
                    speed: p.speed.clone(),
                    device_name: p.peer.as_ref().map(|d| d.name.clone()),
                    device_model: p.peer.as_ref().map(|d| d.model.clone()),
                    device_vendor: p.peer.as_ref().map(|d| d.vendor.clone()),
                    ip_service: p.peer.as_ref().is_some_and(|d| d.ip_service),
                })
                .collect(),
            bridge: BridgeInfo {
                present: s.thunderbolt.bridge.present,
                up: s.thunderbolt.bridge.up,
                ipv4: s.thunderbolt.bridge.ipv4.map(|ip| ip.to_string()),
            },
            hardware_error: s.thunderbolt.hardware_error.clone(),
            peer: s.peer.map(|p| PeerDetails {
                name: p.name,
                model: p.model,
                os_version: p.os_version,
                address: p.address,
            }),
            role: match s.role {
                Role::Local => ControlRole::Local,
                Role::Controlling { .. } => ControlRole::Controlling,
                Role::Controlled { .. } => ControlRole::Controlled,
            },
            peer_side: match s.peer_side {
                Side::Left => ScreenSide::Left,
                Side::Right => ScreenSide::Right,
            },
            sharing_enabled: s.sharing_enabled,
            capture_error: s.capture_error,
            last_error: s.last_error,
            latency_ms: s.round_trip.map(|d| d.as_secs_f64() * 1000.0),
        }
    }

    pub fn set_peer_side(&self, side: ScreenSide) {
        self.engine.send(Command::SetPeerSide(match side {
            ScreenSide::Left => Side::Left,
            ScreenSide::Right => Side::Right,
        }));
    }

    pub fn set_sharing_enabled(&self, enabled: bool) {
        self.engine.send(Command::SetSharingEnabled(enabled));
    }

    pub fn return_to_local(&self) {
        self.engine.send(Command::ReturnToLocal);
    }

    pub fn retry_capture(&self) {
        self.engine.send(Command::RetryCapture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_matches_crates() {
        let info = build_info();
        assert_eq!(info.protocol_major, osm_protocol::PROTOCOL_MAJOR);
        assert_eq!(info.default_port, 49001);
    }

    #[test]
    fn agent_starts_and_reports_local() {
        let agent = Agent::new("test".into());
        let s = agent.status();
        assert_eq!(s.role, ControlRole::Local);
        assert!(s.sharing_enabled);
    }
}
