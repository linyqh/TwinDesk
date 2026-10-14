//! Engine thread: owns the session and every OS resource that acts on it.
//!
//! Data flow:
//!   tap thread  -> pump thread -+
//!   reader thread (peer frames) -+-> engine thread -> Router -> effects
//!   link thread (cable, beacons) -+        |
//!   UI commands -----------------+         +-> Status snapshot <- UI polls
//!
//! The engine is the only writer to the peer socket, the injector and the
//! cursor lock, so no locks sit on the input path.

use std::net::{SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use osm_core::{Desktop, Side};
use osm_platform_macos::{
    Capture, CaptureMode, CursorLock, DragPasteboard, FnRemap, Injector, InputEvent,
    ThunderboltStatus, UntrackedLeft, cursor_position, display_rects, physical_left_down,
};
use osm_protocol::envelope;
use osm_protocol::v1::{self, envelope::Payload};
use osm_transport::{DiscoveredPeer, FrameReader, FrameWriter, ReadError, tune};

use crate::drag::{DragCoordinator, DragEffect, Driver, PreparedOffer, Probe};
use crate::link;
use crate::router::{Effect, Role, Router};
use crate::transfers::Transfers;

const PING_INTERVAL: Duration = Duration::from_millis(500);
/// No traffic from the peer for this long ends the session. This bounds how
/// long injected keys can stay held if the cable is pulled mid-keystroke.
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(100);
/// How long the UI gets to put up a shield or pickup panel before the drag
/// is cancelled instead (D: panels appear in well under 100 ms).
const OVERLAY_TIMEOUT: Duration = Duration::from_millis(1500);
/// Lets the window server deliver a release to the shield before the
/// shield disappears from under it (D).
const SHIELD_LINGER: Duration = Duration::from_millis(250);
/// Items in one drag offer; more would not fit in a control frame (D).
const MAX_DRAG_ITEMS: usize = 256;

/// Panels the engine asks the UI to show. Implemented in Swift; every call
/// comes from the engine thread and must return without blocking on it.
pub trait DragUi: Send + Sync {
    /// Show an invisible panel at `at` (Quartz points) that refuses drops.
    /// Report back with [`Command::OverlayReady`].
    fn show_shield(&self, request: u64, x: f64, y: f64);
    fn hide_shield(&self, request: u64);
    /// Show an invisible panel at `at`; the next left press on it starts a
    /// native drag of file promises for `items`. Report readiness with
    /// [`Command::OverlayReady`] and the end of the drag with
    /// [`Command::ProxyEnded`].
    fn begin_proxy(&self, request: u64, drag_id: u64, items: Vec<DragItemInfo>, x: f64, y: f64);
    fn end_proxy(&self, drag_id: u64);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragItemInfo {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// No Thunderbolt cable to another computer.
    NoCable,
    /// Cable present, but the Thunderbolt Bridge has no address yet.
    CableNoNetwork,
    /// Bridge up, waiting for OpenShareMouse on the other end.
    Searching,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub name: String,
    pub model: String,
    pub os_version: String,
    pub address: String,
}

/// Everything the UI shows, refreshed by the engine.
#[derive(Clone, Debug)]
pub struct Status {
    pub link: LinkState,
    pub thunderbolt: ThunderboltStatus,
    pub peer: Option<PeerInfo>,
    pub role: Role,
    pub peer_side: Side,
    pub sharing_enabled: bool,
    pub capture_error: Option<String>,
    pub last_error: Option<String>,
    pub round_trip: Option<Duration>,
}

#[derive(Clone, Debug)]
pub enum Command {
    SetPeerSide(Side),
    SetSharingEnabled(bool),
    /// Ends any session and gives this Mac back its own keyboard and mouse.
    ReturnToLocal,
    /// Try to create the event tap again, e.g. after granting permission.
    RetryCapture,
    /// The UI put up the panel for an overlay request.
    OverlayReady(u64),
    /// The proxy drag for `drag_id` ended; `dropped` if a target took it.
    ProxyEnded {
        drag_id: u64,
        dropped: bool,
    },
}

pub(crate) enum Msg {
    Input(InputEvent),
    Link {
        status: ThunderboltStatus,
        net_error: Option<String>,
    },
    PeerFound(DiscoveredPeer),
    Stream {
        stream: TcpStream,
        addr: SocketAddrV4,
        dialed: bool,
    },
    DialFailed(String),
    Frame {
        conn: u64,
        env: v1::Envelope,
    },
    ConnClosed {
        conn: u64,
        reason: String,
    },
    Command(Command),
    Shutdown,
}

/// Handle owned by the UI. Dropping it stops every thread and restores
/// local control.
#[derive(Debug)]
pub struct Engine {
    tx: Sender<Msg>,
    status: Arc<Mutex<Status>>,
    running: Arc<AtomicBool>,
    threads: Vec<thread::JoinHandle<()>>,
    transfers: Transfers,
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Msg")
    }
}

impl Engine {
    pub fn start(device_name: String, ui: Option<Arc<dyn DragUi>>) -> Self {
        let device_id = random_u64();
        let (tx, rx) = channel();
        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(Status {
            link: LinkState::NoCable,
            thunderbolt: ThunderboltStatus::default(),
            peer: None,
            role: Role::Local,
            peer_side: Side::Right,
            sharing_enabled: true,
            capture_error: None,
            last_error: None,
            round_trip: None,
        }));

        let mut threads = vec![link::spawn(
            tx.clone(),
            running.clone(),
            device_id,
            device_name.clone(),
        )];
        let engine_tx = tx.clone();
        let engine_status = status.clone();
        let transfers = Transfers::new();
        let engine_transfers = transfers.clone();
        threads.push(
            thread::Builder::new()
                .name("osm-engine".into())
                .spawn(move || {
                    State::new(
                        engine_tx,
                        engine_status,
                        device_id,
                        device_name,
                        engine_transfers,
                        ui,
                    )
                    .run(rx)
                })
                .expect("spawn engine thread"),
        );
        Self {
            tx,
            status,
            running,
            threads,
            transfers,
        }
    }

    /// File transfers started by drag and drop. Safe to use from any thread.
    pub fn transfers(&self) -> &Transfers {
        &self.transfers
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Msg::Command(cmd));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.transfers.cancel_all();
        let _ = self.tx.send(Msg::Shutdown);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

struct Connection {
    id: u64,
    writer: FrameWriter,
    addr: SocketAddrV4,
    last_seen: Instant,
    last_ping: Instant,
    ping_sent: Option<(u64, Instant)>,
    /// Bulk listener port announced to the peer for this connection.
    bulk_port: Option<u16>,
    /// Peer understands drag-and-drop messages (protocol minor >= 1).
    drag_capable: bool,
}

struct State {
    tx: Sender<Msg>,
    status: Arc<Mutex<Status>>,
    device_id: u64,
    device_name: String,

    capture: Option<Capture>,
    injector: Injector,
    cursor: CursorLock,
    fn_remap: FnRemap,
    router: Router,

    thunderbolt: ThunderboltStatus,
    net_error: Option<String>,
    conn: Option<Connection>,
    next_conn: u64,
    dialing: bool,
    peer: Option<PeerInfo>,
    capture_error: Option<String>,
    last_error: Option<String>,
    round_trip: Option<Duration>,
    sharing_enabled: bool,

    transfers: Transfers,
    ui: Option<Arc<dyn DragUi>>,
    drag: DragCoordinator,
    drag_pb: Option<DragPasteboard>,
    /// Physical left button state as seen by the tap in Local mode.
    left_down: bool,
    /// Drag pasteboard was rewritten since the current press began.
    drag_started: bool,
    overlay_deadlines: Vec<(u64, Instant)>,
    shield_hides: Vec<(u64, Instant)>,
}

impl State {
    fn new(
        tx: Sender<Msg>,
        status: Arc<Mutex<Status>>,
        device_id: u64,
        device_name: String,
        transfers: Transfers,
        ui: Option<Arc<dyn DragUi>>,
    ) -> Self {
        let desktop = current_desktop();
        let mut s = Self {
            tx,
            status,
            device_id,
            device_name,
            capture: None,
            injector: Injector::new(),
            cursor: CursorLock::default(),
            fn_remap: FnRemap::default(),
            router: Router::new(desktop, Side::Right),
            thunderbolt: ThunderboltStatus::default(),
            net_error: None,
            conn: None,
            next_conn: 1,
            dialing: false,
            peer: None,
            capture_error: None,
            last_error: None,
            round_trip: None,
            sharing_enabled: true,
            transfers,
            drag_pb: if ui.is_some() {
                DragPasteboard::open()
            } else {
                None
            },
            ui,
            drag: DragCoordinator::new(),
            left_down: false,
            drag_started: false,
            overlay_deadlines: Vec::new(),
            shield_hides: Vec::new(),
        };
        // A crash while forwarding would leave Fn remapped; undo that first.
        let stale = FnRemap::clear_stale();
        if stale > 0 {
            tracing::info!(keyboards = stale, "cleared Fn remap left by a previous run");
        }
        s.start_capture();
        s
    }

    fn start_capture(&mut self) {
        if self.capture.is_some() {
            return;
        }
        match Capture::start() {
            Ok((capture, events)) => {
                let tx = self.tx.clone();
                // Pump: the tap's bounded channel into the engine's queue.
                thread::Builder::new()
                    .name("osm-input-pump".into())
                    .spawn(move || {
                        for ev in events {
                            if tx.send(Msg::Input(ev)).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("spawn pump thread");
                self.capture = Some(capture);
                self.capture_error = None;
            }
            Err(e) => self.capture_error = Some(e.to_string()),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        self.publish();
        let mut last_desktop_check = Instant::now();
        loop {
            match rx.recv_timeout(TICK) {
                Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
            if last_desktop_check.elapsed() >= Duration::from_secs(1) {
                last_desktop_check = Instant::now();
                let fx = self.router.set_desktop(current_desktop());
                self.apply(fx);
            }
            self.publish();
        }
        // Leave both machines usable.
        let fx = self.router.abort();
        self.apply(fx);
        let fx = self.drag.reset(cursor_position());
        self.apply_drag(fx);
        self.transfers.disconnected();
        self.injector.release_all();
        self.fn_remap.release();
        self.cursor.unlock(None);
        if let Some(c) = self.conn.take() {
            c.writer.shutdown();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Input(ev) => self.on_input(ev),
            Msg::Link { status, net_error } => {
                let had_peer = self.thunderbolt.connected_peer().is_some();
                self.thunderbolt = status;
                self.net_error = net_error;
                if had_peer && self.thunderbolt.connected_peer().is_none() {
                    // Cable pulled: don't wait for the TCP timeout.
                    self.drop_connection("雷雳数据线已断开".into());
                }
            }
            Msg::PeerFound(peer) => self.maybe_dial(peer),
            Msg::Stream {
                stream,
                addr,
                dialed,
            } => {
                if dialed {
                    self.dialing = false;
                }
                self.adopt(stream, addr);
            }
            Msg::DialFailed(e) => {
                self.dialing = false;
                self.last_error = Some(e);
            }
            Msg::Frame { conn, env } => {
                if self.conn.as_ref().is_some_and(|c| c.id == conn) {
                    self.on_frame(env);
                }
            }
            Msg::ConnClosed { conn, reason } => {
                if self.conn.as_ref().is_some_and(|c| c.id == conn) {
                    self.drop_connection(reason);
                }
            }
            Msg::Command(cmd) => self.on_command(cmd),
            Msg::Shutdown => {}
        }
    }

    fn on_command(&mut self, cmd: Command) {
        let fx = match cmd {
            Command::SetPeerSide(side) => self.router.set_peer_side(side),
            Command::SetSharingEnabled(on) => {
                self.sharing_enabled = on;
                self.router.set_sharing_enabled(on)
            }
            Command::ReturnToLocal => self.router.abort(),
            Command::RetryCapture => {
                self.start_capture();
                Vec::new()
            }
            Command::OverlayReady(request) => {
                self.overlay_deadlines.retain(|(r, _)| *r != request);
                let held = physical_left_down();
                let fx = self.drag.overlay_ready(request, held);
                self.apply_drag(fx);
                Vec::new()
            }
            Command::ProxyEnded { drag_id, dropped } => {
                let fx = self.drag.proxy_ended(drag_id, dropped);
                self.apply_drag(fx);
                Vec::new()
            }
        };
        self.apply(fx);
    }

    fn on_input(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::LocalLeftButton { down } => {
                self.left_down = down;
                if down {
                    // Absorb older drags, so only one begun by this press
                    // counts at the crossing.
                    if let Some(pb) = &self.drag_pb {
                        pb.changed();
                    }
                    self.drag_started = false;
                } else {
                    let fx = self.drag.physical_left_up_local();
                    self.apply_drag(fx);
                }
                return;
            }
            InputEvent::LocalMove { left_down, .. } => {
                if left_down
                    && !self.drag_started
                    && let Some(pb) = &self.drag_pb
                    && pb.changed()
                {
                    self.drag_started = true;
                }
                if !left_down && self.left_down {
                    // Missed the release (e.g. while forwarding).
                    self.left_down = false;
                }
            }
            InputEvent::Button {
                button: 0,
                down: false,
                ..
            } if matches!(self.router.role(), Role::Controlling { .. }) => {
                let fx = self.drag.physical_left_up_forwarding();
                self.apply_drag(fx);
            }
            _ => {}
        }
        let before = self.router.role();
        let fx = self.router.on_local(ev);
        self.apply(fx);
        if before == Role::Local
            && matches!(self.router.role(), Role::Controlling { .. })
            && self.left_down
        {
            self.crossed_out(Driver::Physical);
        }
    }

    /// The cursor just left this Mac with the left button held by `driver`.
    fn crossed_out(&mut self, driver: Driver) {
        let at = match driver {
            Driver::Physical => self.router.park(),
            Driver::Injected => self.injector.position(),
        };
        // Carrying our own proxy back: it is cancelled, not offered.
        let probe = if self.drag.has_proxy() {
            Probe::NoDrag
        } else {
            self.probe_drag(driver)
        };
        let fx = self.drag.crossing_out(driver, probe, at);
        self.apply_drag(fx);
    }

    fn probe_drag(&mut self, driver: Driver) -> Probe {
        if self.ui.is_none() || !self.conn.as_ref().is_some_and(|c| c.drag_capable) {
            return Probe::NoDrag;
        }
        let Some(pb) = &self.drag_pb else {
            return Probe::NoDrag;
        };
        let started = match driver {
            Driver::Physical => std::mem::take(&mut self.drag_started) || pb.changed(),
            // The injected press happened on this Mac too, so any rewrite
            // of the drag pasteboard since then is this drag.
            Driver::Injected => pb.changed(),
        };
        if !started {
            return Probe::NoDrag;
        }
        let paths = pb.file_paths();
        if paths.is_empty() || paths.len() > MAX_DRAG_ITEMS {
            return Probe::OtherDrag;
        }
        let items: Result<Vec<_>, _> = paths
            .iter()
            .map(|p| osm_transfer::describe_item(p))
            .collect();
        match items {
            Ok(items) => Probe::Files(PreparedOffer {
                drag_id: random_u64(),
                token: random_bytes(),
                items,
                paths,
            }),
            Err(e) => {
                tracing::info!("dragged files cannot be offered: {e}");
                Probe::OtherDrag
            }
        }
    }

    fn apply_drag(&mut self, effects: Vec<DragEffect>) {
        for fx in effects {
            match fx {
                DragEffect::ShowShield { request, at } => {
                    self.overlay_deadlines
                        .push((request, Instant::now() + OVERLAY_TIMEOUT));
                    if let Some(ui) = &self.ui {
                        ui.show_shield(request, at.x, at.y);
                    }
                }
                DragEffect::HideShield { request } => {
                    self.overlay_deadlines.retain(|(r, _)| *r != request);
                    if let Some(ui) = &self.ui {
                        ui.hide_shield(request);
                    }
                }
                DragEffect::HideShieldSoon { request } => {
                    self.shield_hides
                        .push((request, Instant::now() + SHIELD_LINGER));
                }
                DragEffect::ReleaseLeft(Driver::Injected) => {
                    // Zero-length drag first, so the session re-targets the
                    // shield that just appeared under the cursor.
                    self.injector.nudge();
                    self.injector.button(0, false, 1);
                }
                // The OS saw the press but its release was forwarded.
                DragEffect::ReleaseLeft(Driver::Physical) => {
                    self.injector.post_untracked_left(UntrackedLeft::Drag);
                    self.injector.post_untracked_left(UntrackedLeft::Up);
                }
                DragEffect::PressLeft(Driver::Injected) => {
                    self.injector.button(0, true, 1);
                    self.injector.nudge();
                }
                // The press was forwarded, so the OS never saw it; the
                // user's real release later ends this drag normally.
                DragEffect::PressLeft(Driver::Physical) => {
                    self.injector.post_untracked_left(UntrackedLeft::Down);
                    self.injector.post_untracked_left(UntrackedLeft::Drag);
                }
                DragEffect::Escape(_) => self.injector.tap_escape(),
                DragEffect::BeginProxy {
                    request,
                    drag_id,
                    items,
                    at,
                } => {
                    self.overlay_deadlines
                        .push((request, Instant::now() + OVERLAY_TIMEOUT));
                    if let Some(ui) = &self.ui {
                        let items = items
                            .into_iter()
                            .map(|i| DragItemInfo {
                                is_directory: i.kind == v1::EntryKind::Directory as i32,
                                name: i.name,
                                size: i.size,
                            })
                            .collect();
                        ui.begin_proxy(request, drag_id, items, at.x, at.y);
                    }
                }
                DragEffect::EndProxy { drag_id } => {
                    if let Some(ui) = &self.ui {
                        ui.end_proxy(drag_id);
                    }
                }
                DragEffect::Offer(offer) => self.send_offer(offer),
                DragEffect::Send(p) => self.send(p),
            }
        }
    }

    fn send_offer(&mut self, offer: PreparedOffer) {
        let Some(port) = self.conn.as_ref().and_then(|c| c.bulk_port) else {
            return;
        };
        self.transfers
            .serve(offer.drag_id, offer.token.clone(), offer.paths);
        self.send(Payload::DragOffer(v1::DragOffer {
            drag_id: offer.drag_id,
            epoch: self.router.last_epoch(),
            token: offer.token,
            bulk_port: port.into(),
            items: offer.items,
        }));
    }

    fn on_drag_offer(&mut self, offer: v1::DragOffer) {
        if offer.epoch != self.router.last_epoch() || !self.transfers.offer_received(&offer) {
            return;
        }
        let (driver, at) = match self.router.role() {
            // The button is held on the peer's mouse; if it is released
            // before our pickup is ready, the forwarded release aborts.
            Role::Controlled { .. } => (Driver::Injected, self.injector.position()),
            // The peer handed the cursor back with a drag in our user's hand.
            Role::Local if physical_left_down() => (Driver::Physical, cursor_position()),
            _ => {
                self.send(Payload::DragEnd(v1::DragEnd {
                    drag_id: offer.drag_id,
                    dropped: false,
                }));
                return;
            }
        };
        let items = offer.items.into_iter().take(MAX_DRAG_ITEMS).collect();
        let fx = self.drag.offer_received(offer.drag_id, items, driver, at);
        self.apply_drag(fx);
    }

    /// Lower device id dials, so exactly one TCP connection forms.
    fn maybe_dial(&mut self, peer: DiscoveredPeer) {
        if self.conn.is_some() || self.dialing || self.device_id > peer.device_id {
            return;
        }
        if self.device_id == peer.device_id {
            return;
        }
        self.dialing = true;
        link::dial(self.tx.clone(), self.thunderbolt.bridge.index, peer);
    }

    fn adopt(&mut self, stream: TcpStream, addr: SocketAddrV4) {
        if self.conn.is_some() {
            // Already paired with someone; v1 is strictly two machines.
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        if let Err(e) = tune(&stream) {
            self.last_error = Some(format!("连接设置失败：{e}"));
            return;
        }
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let id = self.next_conn;
        self.next_conn += 1;

        let tx = self.tx.clone();
        thread::Builder::new()
            .name("osm-net-reader".into())
            .spawn(move || {
                let mut reader = FrameReader::new(read_half);
                loop {
                    match reader.read_frame() {
                        Ok(env) => {
                            if tx.send(Msg::Frame { conn: id, env }).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let reason = match e {
                                ReadError::Closed => "对方已断开连接".to_string(),
                                ReadError::Io(e) => format!("网络错误：{e}"),
                                ReadError::Frame(e) => format!("协议错误：{e}"),
                            };
                            let _ = tx.send(Msg::ConnClosed { conn: id, reason });
                            return;
                        }
                    }
                }
            })
            .expect("spawn reader thread");

        let now = Instant::now();
        let bulk_port = self
            .transfers
            .connected(*addr.ip(), self.thunderbolt.bridge.index);
        self.conn = Some(Connection {
            id,
            writer: FrameWriter::new(stream),
            addr,
            last_seen: now,
            last_ping: now,
            ping_sent: None,
            bulk_port,
            drag_capable: false,
        });
        self.last_error = None;
        self.send(Payload::Hello(v1::Hello {
            device_id: self.device_id,
            name: self.device_name.clone(),
            model: sysctl_string("hw.model").unwrap_or_default(),
            os_version: os_version(),
        }));
        tracing::info!("connected to {addr}");
    }

    fn drop_connection(&mut self, reason: String) {
        let Some(c) = self.conn.take() else { return };
        c.writer.shutdown();
        self.transfers.disconnected();
        tracing::info!("connection to {} closed: {reason}", c.addr);
        self.peer = None;
        self.round_trip = None;
        self.last_error = Some(reason);
        let fx = self.router.disconnected();
        self.apply(fx);
        let fx = self.drag.reset(cursor_position());
        self.apply_drag(fx);
    }

    fn on_frame(&mut self, env: v1::Envelope) {
        if let Some(c) = &mut self.conn {
            c.last_seen = Instant::now();
            c.drag_capable = env.protocol_minor >= 1;
        }
        let Some(payload) = env.payload else { return };
        match payload {
            Payload::Hello(h) => {
                self.transfers.peer_identified(h.device_id);
                let address = self
                    .conn
                    .as_ref()
                    .map(|c| c.addr.ip().to_string())
                    .unwrap_or_default();
                self.peer = Some(PeerInfo {
                    name: h.name,
                    model: h.model,
                    os_version: h.os_version,
                    address,
                });
            }
            Payload::Ping(p) => self.send(Payload::Pong(v1::Pong { nonce: p.nonce })),
            Payload::Pong(p) => {
                if let Some(c) = &mut self.conn
                    && let Some((nonce, at)) = c.ping_sent
                    && nonce == p.nonce
                {
                    self.round_trip = Some(at.elapsed());
                    c.ping_sent = None;
                }
            }
            Payload::DragOffer(o) => self.on_drag_offer(o),
            Payload::DragEnd(e) => {
                let fx = self.drag.drag_end_received(e.drag_id);
                self.apply_drag(fx);
            }
            other => {
                let forwarded_left_up = matches!(
                    other,
                    Payload::MouseButton(v1::MouseButton {
                        button: 0,
                        down: false,
                        ..
                    })
                ) && matches!(self.router.role(), Role::Controlled { .. });
                let before = self.router.role();
                let fx = self.router.on_peer(other);
                self.apply(fx);
                if forwarded_left_up {
                    let fx = self.drag.forwarded_left_up();
                    self.apply_drag(fx);
                }
                // The peer handed control back while our user's hand still
                // holds a drag that was suspended here.
                if matches!(before, Role::Controlling { .. }) && self.router.role() == Role::Local {
                    let fx = self.drag.crossing_in();
                    self.apply_drag(fx);
                }
            }
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .overlay_deadlines
            .iter()
            .filter(|(_, t)| *t <= now)
            .map(|(r, _)| *r)
            .collect();
        self.overlay_deadlines.retain(|(_, t)| *t > now);
        for request in expired {
            tracing::warn!("drag overlay did not appear in time");
            let fx = self.drag.overlay_timeout(request);
            self.apply_drag(fx);
        }
        let hides: Vec<u64> = self
            .shield_hides
            .iter()
            .filter(|(_, t)| *t <= now)
            .map(|(r, _)| *r)
            .collect();
        self.shield_hides.retain(|(_, t)| *t > now);
        for request in hides {
            if let Some(ui) = &self.ui {
                ui.hide_shield(request);
            }
        }

        let Some(c) = &mut self.conn else { return };
        if c.last_seen.elapsed() > PEER_TIMEOUT {
            self.drop_connection("对方无响应（超时）".into());
            return;
        }
        if c.last_ping.elapsed() >= PING_INTERVAL {
            c.last_ping = Instant::now();
            let nonce = random_u64();
            c.ping_sent = Some((nonce, Instant::now()));
            self.send(Payload::Ping(v1::Ping { nonce }));
        }
    }

    fn send(&mut self, payload: Payload) {
        let Some(c) = &mut self.conn else { return };
        if let Err(e) = c.writer.send(&envelope(payload)) {
            self.drop_connection(format!("发送失败：{e}"));
        }
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for fx in effects {
            match fx {
                Effect::Send(p) => self.send(p),
                Effect::BeginForwarding { park } => {
                    if self.conn.is_none() || self.capture.is_none() {
                        // Nobody to forward to: undo the switch right away.
                        let undo = self.router.disconnected();
                        self.apply(undo);
                        continue;
                    }
                    // Before forwarding starts, so no Fn press can slip through
                    // to a local app that taps ahead of us.
                    self.fn_remap.engage();
                    self.cursor.lock(park);
                    self.set_mode(CaptureMode::Forwarding);
                }
                Effect::EndForwarding { at } => {
                    self.set_mode(CaptureMode::Local);
                    self.fn_remap.release();
                    self.cursor.unlock(Some(at));
                    // The tap never saw the release if it happened while
                    // forwarding; trust the hardware state.
                    self.left_down = physical_left_down();
                    if !self.drag.has_proxy() {
                        let fx = self.drag.crossing_in();
                        self.apply_drag(fx);
                    }
                }
                Effect::BeginControlled { at } => {
                    self.injector.warp(at);
                    self.set_mode(CaptureMode::Controlled);
                    // A drag of ours carried to the peer comes back: it
                    // continues under the injected button.
                    let fx = self.drag.crossing_in();
                    self.apply_drag(fx);
                    // A press arriving from the peer starts a fresh drag.
                    if let Some(pb) = &self.drag_pb {
                        pb.changed();
                    }
                }
                Effect::EndControlled { handoff } => {
                    if handoff && self.injector.left_down() {
                        self.crossed_out(Driver::Injected);
                    } else if !handoff {
                        let fx = self.drag.reset(cursor_position());
                        self.apply_drag(fx);
                    }
                    let keep_left = self.drag.keeps_injected_left();
                    self.injector.release_all_but_left(keep_left);
                    self.set_mode(CaptureMode::Local);
                }
                Effect::InjectMove { to, dx, dy } => self.injector.move_to(to, dx, dy),
                Effect::InjectButton {
                    button,
                    down,
                    click_state,
                } => self.injector.button(button, down, click_state),
                Effect::InjectScroll {
                    continuous,
                    dy,
                    dx,
                    scroll_phase,
                    momentum_phase,
                } => self
                    .injector
                    .scroll(continuous, dy, dx, scroll_phase, momentum_phase),
                Effect::InjectKey {
                    keycode,
                    down,
                    repeat,
                    flags,
                } => self.injector.key(keycode, down, repeat, flags),
            }
        }
    }

    fn set_mode(&self, mode: CaptureMode) {
        if let Some(c) = &self.capture {
            c.set_mode(mode);
        }
    }

    fn link_state(&self) -> LinkState {
        if self.conn.is_some() {
            return LinkState::Connected;
        }
        if self.thunderbolt.connected_peer().is_none() {
            return LinkState::NoCable;
        }
        if !self.thunderbolt.link_ready() {
            return LinkState::CableNoNetwork;
        }
        if self.dialing {
            LinkState::Connecting
        } else {
            LinkState::Searching
        }
    }

    fn publish(&self) {
        let mut s = self.status.lock().unwrap();
        s.link = self.link_state();
        s.thunderbolt = self.thunderbolt.clone();
        s.peer = self.peer.clone();
        s.role = self.router.role();
        s.peer_side = self.router.peer_side();
        s.sharing_enabled = self.sharing_enabled;
        s.capture_error = self.capture_error.clone();
        s.last_error = self.net_error.clone().or_else(|| self.last_error.clone());
        s.round_trip = self.round_trip;
    }
}

fn current_desktop() -> Desktop {
    Desktop::new(display_rects()).unwrap_or_else(|| {
        // No display reported (e.g. lid closed, headless): use a nominal one
        // so geometry stays valid; switching is pointless without a screen.
        Desktop::new(vec![osm_model::LogicalRect {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
        }])
        .expect("nominal desktop")
    })
}

fn random_bytes() -> Vec<u8> {
    let mut b = vec![0u8; 16];
    // SAFETY: arc4random_buf fills exactly the buffer given.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    b
}

fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    // SAFETY: arc4random_buf fills exactly the buffer given.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    u64::from_ne_bytes(b)
}

fn sysctl_string(name: &str) -> Option<String> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    // SAFETY: first call queries the size, second fills a buffer of that size.
    unsafe {
        if libc::sysctlbyname(
            cname.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        while buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf).ok()
    }
}

fn os_version() -> String {
    sysctl_string("kern.osproductversion")
        .map(|v| format!("macOS {v}"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysctl_reads_model() {
        let model = sysctl_string("hw.model").unwrap();
        assert!(!model.is_empty());
        assert!(os_version().starts_with("macOS "));
    }

    #[test]
    fn random_ids_differ() {
        assert_ne!(random_u64(), random_u64());
    }
}
