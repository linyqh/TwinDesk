//! Link thread: polls Thunderbolt status, and while the Thunderbolt Bridge
//! has an address, announces this device and accepts connections on it.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use osm_platform_macos::{BridgeStatus, thunderbolt};
use osm_transport::{DEFAULT_PORT, DiscoveredPeer, Discovery, listen};

use crate::engine::Msg;

pub(crate) const LINK_POLL: Duration = Duration::from_secs(2);
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(1);
/// Used when the bridge reports no broadcast address.
const LINK_LOCAL_BROADCAST: Ipv4Addr = Ipv4Addr::new(169, 254, 255, 255);

struct NetSetup {
    /// Bridge identity this setup was built for; rebuilt when it changes.
    key: (u32, Ipv4Addr),
    discovery: Discovery,
    listener: TcpListener,
}

pub(crate) fn spawn(
    tx: Sender<Msg>,
    running: Arc<AtomicBool>,
    device_id: u64,
    name: String,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("osm-link".into())
        .spawn(move || run(tx, running, device_id, name))
        .expect("spawn link thread")
}

fn run(tx: Sender<Msg>, running: Arc<AtomicBool>, device_id: u64, name: String) {
    let mut last_probe: Option<Instant> = None;
    let mut last_announce: Option<Instant> = None;
    let mut net: Option<NetSetup> = None;
    let mut net_error: Option<String> = None;

    while running.load(Ordering::Acquire) {
        if last_probe.is_none_or(|t| t.elapsed() >= LINK_POLL) {
            last_probe = Some(Instant::now());
            let status = thunderbolt::probe();
            reconfigure(&mut net, &mut net_error, &status.bridge, device_id, &name);
            if tx
                .send(Msg::Link {
                    status,
                    net_error: net_error.clone(),
                })
                .is_err()
            {
                return;
            }
        }

        let Some(n) = &net else {
            thread::sleep(Duration::from_millis(250));
            continue;
        };

        if last_announce.is_none_or(|t| t.elapsed() >= ANNOUNCE_INTERVAL) {
            last_announce = Some(Instant::now());
            if let Err(e) = n.discovery.announce() {
                tracing::debug!("announce failed: {e}");
            }
        }

        loop {
            match n.listener.accept() {
                Ok((stream, addr)) => {
                    let _ = stream.set_nonblocking(false);
                    let std::net::SocketAddr::V4(addr) = addr else {
                        continue;
                    };
                    if tx
                        .send(Msg::Stream {
                            stream,
                            addr,
                            dialed: false,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    tracing::debug!("accept failed: {e}");
                    break;
                }
            }
        }

        // Blocks for at most the socket read timeout (500 ms).
        match n.discovery.receive() {
            Ok(Some(peer)) => {
                if tx.send(Msg::PeerFound(peer)).is_err() {
                    return;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("discovery receive failed: {e}"),
        }
    }
}

fn reconfigure(
    net: &mut Option<NetSetup>,
    net_error: &mut Option<String>,
    bridge: &BridgeStatus,
    device_id: u64,
    name: &str,
) {
    let key = match (bridge.up, bridge.ipv4) {
        (true, Some(ip)) if bridge.index != 0 => (bridge.index, ip),
        _ => {
            *net = None;
            *net_error = None;
            return;
        }
    };
    if net.as_ref().is_some_and(|n| n.key == key) {
        return;
    }
    *net = None;
    let broadcast = bridge.broadcast.unwrap_or(LINK_LOCAL_BROADCAST);
    let built = listen(key.0, DEFAULT_PORT).and_then(|listener| {
        Discovery::new(key.0, broadcast, device_id, name, DEFAULT_PORT).map(|discovery| NetSetup {
            key,
            discovery,
            listener,
        })
    });
    match built {
        Ok(n) => {
            tracing::info!("listening on bridge if={} ip={}", key.0, key.1);
            *net = Some(n);
            *net_error = None;
        }
        Err(e) => {
            tracing::warn!("cannot open bridge sockets: {e}");
            *net_error = Some(format!("无法在雷雳网桥上建立网络连接：{e}"));
        }
    }
}

/// Dials the peer off the engine thread; the connect can take seconds.
pub(crate) fn dial(tx: Sender<Msg>, if_index: u32, peer: DiscoveredPeer) {
    thread::Builder::new()
        .name("osm-dial".into())
        .spawn(move || match osm_transport::connect(if_index, peer.addr) {
            Ok(stream) => {
                let _ = tx.send(Msg::Stream {
                    stream,
                    addr: peer.addr,
                    dialed: true,
                });
            }
            Err(e) => {
                let _ = tx.send(Msg::DialFailed(format!("连接 {} 失败：{e}", peer.addr)));
            }
        })
        .expect("spawn dial thread");
}
