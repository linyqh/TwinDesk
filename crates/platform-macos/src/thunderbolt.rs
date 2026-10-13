//! Thunderbolt cable and Thunderbolt Bridge detection.
//!
//! Two independent signals:
//! - per-port hardware state from `system_profiler SPThunderboltDataType`
//!   (receptacle status, link speed, the device on the other end and whether
//!   it offers the IP service). There is no public framework API for this;
//!   the JSON output is stable and takes ~50 ms.
//! - the `bridge0` network interface that macOS builds over the Thunderbolt
//!   ports ("Thunderbolt Bridge"), which is what we actually talk over.

use std::net::Ipv4Addr;
use std::process::Command;

use serde::Deserialize;

/// Thunderbolt Bridge interface name. macOS names it `bridge0` by default.
pub const BRIDGE_INTERFACE: &str = "bridge0";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThunderboltPort {
    /// Receptacle number as printed by System Information.
    pub receptacle: String,
    pub connected: bool,
    /// e.g. "40 Gb/s" when linked, "Up to 40 Gb/s" when idle.
    pub speed: String,
    pub peer: Option<ThunderboltPeer>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThunderboltPeer {
    /// Marketing name, e.g. "Mac Mini".
    pub name: String,
    /// Model identifier, e.g. "Mac18,5".
    pub model: String,
    pub vendor: String,
    /// The peer offers Thunderbolt IP networking.
    pub ip_service: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeStatus {
    pub present: bool,
    /// Interface is up and running.
    pub up: bool,
    pub ipv4: Option<Ipv4Addr>,
    pub ipv4_netmask: Option<Ipv4Addr>,
    pub broadcast: Option<Ipv4Addr>,
    pub index: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThunderboltStatus {
    pub ports: Vec<ThunderboltPort>,
    pub bridge: BridgeStatus,
    /// Set when `system_profiler` could not be run or parsed.
    pub hardware_error: Option<String>,
}

impl ThunderboltStatus {
    /// First port with another computer offering IP networking attached.
    pub fn connected_peer(&self) -> Option<(&ThunderboltPort, &ThunderboltPeer)> {
        self.ports.iter().filter(|p| p.connected).find_map(|p| {
            p.peer
                .as_ref()
                .filter(|peer| peer.ip_service)
                .map(|peer| (p, peer))
        })
    }

    /// The cable is plugged into another computer and the bridge has an
    /// address, so networking over it should work.
    pub fn link_ready(&self) -> bool {
        self.connected_peer().is_some() && self.bridge.up && self.bridge.ipv4.is_some()
    }
}

pub fn probe() -> ThunderboltStatus {
    let (ports, hardware_error) = match read_system_profiler() {
        Ok(ports) => (ports, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    ThunderboltStatus {
        ports,
        bridge: bridge_status(BRIDGE_INTERFACE),
        hardware_error,
    }
}

fn read_system_profiler() -> Result<Vec<ThunderboltPort>, String> {
    let out = Command::new("/usr/sbin/system_profiler")
        .args(["SPThunderboltDataType", "-json", "-detailLevel", "mini"])
        .output()
        .map_err(|e| format!("无法运行 system_profiler：{e}"))?;
    if !out.status.success() {
        return Err(format!("system_profiler 退出码 {}", out.status));
    }
    parse_system_profiler(&out.stdout)
}

#[derive(Deserialize)]
struct Report {
    #[serde(rename = "SPThunderboltDataType", default)]
    buses: Vec<Bus>,
}

#[derive(Deserialize)]
struct Bus {
    #[serde(rename = "receptacle_1_tag")]
    receptacle: Option<Receptacle>,
    #[serde(rename = "_items", default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Receptacle {
    #[serde(default)]
    receptacle_id_key: String,
    #[serde(default)]
    receptacle_status_key: String,
    #[serde(default)]
    current_speed_key: String,
}

#[derive(Deserialize)]
struct Item {
    #[serde(rename = "_name", default)]
    name: String,
    #[serde(default)]
    device_name_key: String,
    #[serde(default)]
    vendor_name_key: String,
    #[serde(rename = "services_title", default)]
    services: Vec<Service>,
}

#[derive(Deserialize)]
struct Service {
    #[serde(rename = "_name", default)]
    name: String,
}

pub fn parse_system_profiler(json: &[u8]) -> Result<Vec<ThunderboltPort>, String> {
    let report: Report =
        serde_json::from_slice(json).map_err(|e| format!("无法解析雷雳信息：{e}"))?;
    let mut ports: Vec<ThunderboltPort> = report
        .buses
        .into_iter()
        .filter_map(|bus| {
            let r = bus.receptacle?;
            let peer = bus.items.into_iter().next().map(|item| ThunderboltPeer {
                name: item.name,
                model: item.device_name_key,
                vendor: item.vendor_name_key,
                ip_service: item.services.iter().any(|s| s.name == "service_ip"),
            });
            Some(ThunderboltPort {
                receptacle: r.receptacle_id_key,
                connected: r.receptacle_status_key == "receptacle_connected",
                speed: r.current_speed_key,
                peer,
            })
        })
        .collect();
    ports.sort_by(|a, b| a.receptacle.cmp(&b.receptacle));
    Ok(ports)
}

fn bridge_status(name: &str) -> BridgeStatus {
    let mut status = BridgeStatus::default();
    let Ok(cname) = std::ffi::CString::new(name) else {
        return status;
    };
    // SAFETY: plain libc call with a valid C string.
    status.index = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    status.present = status.index != 0;
    if !status.present {
        return status;
    }

    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs allocates a list we free below with freeifaddrs.
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return status;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        // SAFETY: cur walks the list getifaddrs returned and is non-null.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        // SAFETY: ifa_name is a valid NUL-terminated string for every entry.
        let ifname = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) };
        if ifname.to_bytes() != name.as_bytes() {
            continue;
        }
        let flags = ifa.ifa_flags as libc::c_int;
        status.up = flags & libc::IFF_UP != 0 && flags & libc::IFF_RUNNING != 0;
        if let Some(addr) = sockaddr_v4(ifa.ifa_addr) {
            status.ipv4 = Some(addr);
            status.ipv4_netmask = sockaddr_v4(ifa.ifa_netmask);
            // On BSD the broadcast address lives in ifa_dstaddr.
            status.broadcast = sockaddr_v4(ifa.ifa_dstaddr);
        }
    }
    // SAFETY: ifap came from a successful getifaddrs call.
    unsafe { libc::freeifaddrs(ifap) };
    status
}

fn sockaddr_v4(sa: *const libc::sockaddr) -> Option<Ipv4Addr> {
    if sa.is_null() {
        return None;
    }
    // SAFETY: non-null sockaddr from getifaddrs; we only reinterpret it after
    // checking the family.
    unsafe {
        if (*sa).sa_family as libc::c_int != libc::AF_INET {
            return None;
        }
        let sin = &*(sa as *const libc::sockaddr_in);
        Some(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONNECTED: &[u8] = include_bytes!("../testdata/sp_thunderbolt_connected.json");

    #[test]
    fn parses_real_output_with_mac_mini_attached() {
        let ports = parse_system_profiler(CONNECTED).unwrap();
        assert_eq!(ports.len(), 3);
        let connected: Vec<_> = ports.iter().filter(|p| p.connected).collect();
        assert_eq!(connected.len(), 1);
        let port = connected[0];
        assert_eq!(port.receptacle, "2");
        assert_eq!(port.speed, "40 Gb/s");
        let peer = port.peer.as_ref().unwrap();
        assert_eq!(peer.name, "Mac Mini");
        assert_eq!(peer.model, "Mac18,5");
        assert!(peer.ip_service);
    }

    #[test]
    fn idle_ports_have_no_peer() {
        let ports = parse_system_profiler(CONNECTED).unwrap();
        for p in ports.iter().filter(|p| !p.connected) {
            assert!(p.peer.is_none());
            assert!(p.speed.starts_with("Up to"));
        }
    }

    #[test]
    fn empty_report_means_no_ports() {
        assert!(
            parse_system_profiler(br#"{"SPThunderboltDataType": []}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_system_profiler(b"{}").unwrap().is_empty());
        assert!(parse_system_profiler(b"not json").is_err());
    }

    #[test]
    fn link_ready_needs_peer_and_address() {
        let ports = parse_system_profiler(CONNECTED).unwrap();
        let mut s = ThunderboltStatus {
            ports,
            ..Default::default()
        };
        assert!(s.connected_peer().is_some());
        assert!(!s.link_ready());
        s.bridge = BridgeStatus {
            present: true,
            up: true,
            ipv4: Some(Ipv4Addr::new(169, 254, 1, 2)),
            ..Default::default()
        };
        assert!(s.link_ready());
    }

    #[test]
    fn probe_runs_on_this_machine() {
        // Must not panic whatever hardware the test machine has.
        let s = probe();
        if let Some(err) = &s.hardware_error {
            eprintln!("system_profiler unavailable: {err}");
        }
    }
}
