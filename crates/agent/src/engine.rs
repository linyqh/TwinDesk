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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use osm_core::caps_lock::{CapsAction, CapsGesture};
use osm_core::{Desktop, Side};
use osm_platform_macos::input_source::{caps_lock_state, set_caps_lock, switch_input_source};
use osm_platform_macos::{
    Capture, CaptureMode, CursorLock, FnRemap, Injector, InputEvent, Pasteboard, ThunderboltStatus,
    display_rects,
};
use osm_protocol::envelope;
use osm_protocol::v1::{self, envelope::Payload};
use osm_transport::{DiscoveredPeer, FrameReader, FrameWriter, ReadError, tune};

use crate::clipboard::Clipboard;
use crate::link;
use crate::router::{Effect, Role, Router};
use crate::transfers::Transfers;

const PING_INTERVAL: Duration = Duration::from_millis(500);
/// No traffic from the peer for this long ends the session. This bounds how
/// long injected keys can stay held if the cable is pulled mid-keystroke.
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(100);

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
    pub portal_available: bool,
}

#[derive(Clone, Debug)]
pub enum Command {
    SetPeerSide(Side),
    SetSharingEnabled(bool),
    /// Ends any session and gives this Mac back its own keyboard and mouse.
    ReturnToLocal,
    /// Try to create the event tap again, e.g. after granting permission.
    RetryCapture,
    SendFiles {
        paths: Vec<PathBuf>,
        items: Vec<v1::DragItem>,
        reply: Sender<Result<(), String>>,
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
    ClipboardOffer {
        conn: u64,
        offer: v1::ClipboardOffer,
    },
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
    pub fn start(device_name: String, downloads: PathBuf) -> Self {
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
            portal_available: false,
        }));

        let mut threads = vec![link::spawn(
            tx.clone(),
            running.clone(),
            device_id,
            device_name.clone(),
        )];
        let engine_tx = tx.clone();
        let engine_status = status.clone();
        let transfers = Transfers::with_clipboard(Arc::new(Clipboard::new(Pasteboard::general())));
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
                        downloads,
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

    /// Metadata inspection runs on the caller's worker thread, never the
    /// input engine or AppKit main thread. Wait only for offer submission.
    pub fn send_files(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        if paths.is_empty() || paths.len() > crate::transfers::MAX_RUNNING {
            return Err("每次可投放 1–16 个文件或文件夹，更多项目请先放入一个文件夹".into());
        }
        let items = paths
            .iter()
            .map(|p| osm_transfer::describe_item(p))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let (reply, rx) = channel();
        self.tx
            .send(Msg::Command(Command::SendFiles {
                paths,
                items,
                reply,
            }))
            .map_err(|_| "应用正在退出".to_string())?;
        rx.recv().map_err(|_| "应用正在退出".to_string())?
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
    /// Peer supports explicit file portals (protocol minor >= 2).
    portal_capable: bool,
    caps_capable: bool,
    clipboard_capable: bool,
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
    caps_gesture: CapsGesture,
    clock: Instant,
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
    downloads: PathBuf,
}

impl State {
    fn new(
        tx: Sender<Msg>,
        status: Arc<Mutex<Status>>,
        device_id: u64,
        device_name: String,
        transfers: Transfers,
        downloads: PathBuf,
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
            caps_gesture: CapsGesture::default(),
            clock: Instant::now(),
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
            downloads,
        };
        // Recover both the old Fn-only and the new Fn/Caps crash mappings.
        let stale = FnRemap::clear_stale();
        if stale > 0 {
            tracing::info!(
                keyboards = stale,
                "cleared keyboard remap left by a previous run"
            );
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
                if matches!(self.router.role(), Role::Controlling { .. }) {
                    self.fn_remap
                        .engage(self.conn.as_ref().is_some_and(|c| c.caps_capable));
                }
                let fx = self.router.set_desktop(current_desktop());
                self.apply(fx);
            }
            self.publish();
        }
        // Leave both machines usable.
        let fx = self.router.abort();
        self.apply(fx);
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
            Msg::ClipboardOffer { conn, offer } => {
                if self.conn.as_ref().is_some_and(|c| c.id == conn) {
                    self.send(Payload::ClipboardOffer(offer));
                }
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
            Command::SendFiles {
                paths,
                items,
                reply,
            } => {
                let result = self.offer_files(paths, items);
                let _ = reply.send(result);
                Vec::new()
            }
        };
        self.apply(fx);
    }

    fn on_input(&mut self, ev: InputEvent) {
        let fx = self.router.on_local(ev);
        self.apply(fx);
    }

    fn offer_files(&mut self, paths: Vec<PathBuf>, items: Vec<v1::DragItem>) -> Result<(), String> {
        let c = self.conn.as_ref().ok_or("对方未连接，请连接后重新投放")?;
        if self.peer.is_none() || !c.portal_capable {
            return Err("请在两台电脑上都更新到支持传送通道的版本".into());
        }
        if !self.sharing_enabled {
            return Err("共享已暂停".into());
        }
        let port = c.bulk_port.ok_or("文件传输端口不可用，请重新连接")?;
        let offer = self
            .transfers
            .offer_files(random_u64(), random_bytes(), port, paths, items)
            .map_err(|e| e.to_string())?;
        self.send(Payload::PortalOffer(offer));
        if self.conn.is_none() {
            return Err("连接已断开，请重新投放".into());
        }
        Ok(())
    }

    /// The user is moving to the other Mac: let it paste what was copied here.
    fn offer_clipboard(&self) {
        let Some(c) = &self.conn else { return };
        let Some(port) = c.bulk_port else { return };
        if !c.clipboard_capable || !self.sharing_enabled || self.peer.is_none() {
            return;
        }
        let (tx, conn) = (self.tx.clone(), c.id);
        self.transfers
            .offer_clipboard(random_u64(), random_bytes(), port, move |offer| {
                let _ = tx.send(Msg::ClipboardOffer { conn, offer });
            });
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
            portal_capable: false,
            caps_capable: false,
            clipboard_capable: false,
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
    }

    fn on_frame(&mut self, env: v1::Envelope) {
        if let Some(c) = &mut self.conn {
            c.last_seen = Instant::now();
            c.portal_capable = env.protocol_minor >= 2;
            c.caps_capable = env.protocol_minor >= 3;
            c.clipboard_capable = env.protocol_minor >= 4;
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
            Payload::PortalOffer(o) => {
                let id = o.transfer_id;
                let result = if self.sharing_enabled && self.peer.is_some() {
                    self.transfers.receive_offer(o, self.downloads.clone())
                } else {
                    Err(osm_transfer::TransferError::Remote(
                        "对方共享已暂停或连接尚未就绪".into(),
                    ))
                };
                if let Err(e) = result {
                    self.send(Payload::PortalReject(v1::PortalReject {
                        transfer_id: id,
                        message: e.to_string(),
                    }));
                }
            }
            Payload::PortalReject(r) => self.transfers.reject_offer(r.transfer_id, &r.message),
            Payload::ClipboardOffer(o) => {
                if self.sharing_enabled && self.peer.is_some() {
                    self.transfers.receive_clipboard(o);
                }
            }
            // Explicitly cancel legacy proxy drags; do not synthesize a drop.
            Payload::DragOffer(o) => self.send(Payload::DragEnd(v1::DragEnd {
                drag_id: o.drag_id,
                dropped: false,
            })),
            Payload::DragEnd(_) => {}
            other => {
                let fx = self.router.on_peer(other);
                self.apply(fx);
            }
        }
    }

    fn tick(&mut self) {
        self.transfers.expire_pending();
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
        if matches!(self.router.role(), Role::Controlled { .. })
            && let Some(action) = self.caps_gesture.poll(self.clock.elapsed())
        {
            self.apply_caps_action(action);
        }
    }

    fn apply_caps_action(&mut self, action: CapsAction) {
        let result = (|| {
            let locked = caps_lock_state()?;
            match action {
                CapsAction::ToggleLock => set_caps_lock(!locked)?,
                CapsAction::ShortPress if locked => set_caps_lock(false)?,
                CapsAction::ShortPress => {
                    if !switch_input_source()? {
                        set_caps_lock(true)?;
                    }
                }
            }
            // IOHID lock changes do not immediately update Quartz flags. Keep
            // an authoritative snapshot for every subsequent injected event.
            self.injector.use_target_caps_lock(Some(caps_lock_state()?));
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "remote Caps Lock failed");
            self.last_error = Some(error);
            // Do not type subsequent characters in an unintended input source.
            let fx = self.router.abort();
            self.apply(fx);
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
                    let caps = self.conn.as_ref().is_some_and(|c| c.caps_capable);
                    self.router.set_caps_lock_forwarding(caps);
                    self.fn_remap.engage(caps);
                    self.cursor.lock(park);
                    self.set_mode(CaptureMode::Forwarding);
                    self.offer_clipboard();
                }
                Effect::EndForwarding { at } => {
                    self.router.set_caps_lock_forwarding(false);
                    self.set_mode(CaptureMode::Local);
                    self.fn_remap.release();
                    self.cursor.unlock(Some(at));
                }
                Effect::BeginControlled { at } => {
                    self.caps_gesture.cancel();
                    if self.conn.as_ref().is_some_and(|c| c.caps_capable) {
                        match caps_lock_state() {
                            Ok(locked) => self.injector.use_target_caps_lock(Some(locked)),
                            Err(error) => {
                                self.last_error = Some(error);
                                let fx = self.router.abort();
                                self.apply(fx);
                                continue;
                            }
                        }
                    }
                    self.injector.warp(at);
                    self.set_mode(CaptureMode::Controlled);
                }
                Effect::EndControlled { .. } => {
                    self.caps_gesture.cancel();
                    if self.conn.as_ref().is_some_and(|c| c.caps_capable)
                        && let Ok(locked) = caps_lock_state()
                    {
                        self.injector.use_target_caps_lock(Some(locked));
                    }
                    self.injector.release_all();
                    self.injector.use_target_caps_lock(None);
                    self.set_mode(CaptureMode::Local);
                    // Copies made while controlled follow the cursor back.
                    self.offer_clipboard();
                }
                Effect::CapsLock { down } => {
                    if let Some(action) = self.caps_gesture.edge(down, self.clock.elapsed()) {
                        self.apply_caps_action(action);
                    }
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
        s.portal_available = self.sharing_enabled
            && self.peer.is_some()
            && self
                .conn
                .as_ref()
                .is_some_and(|c| c.portal_capable && c.bulk_port.is_some());
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
