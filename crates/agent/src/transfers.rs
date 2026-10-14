//! Bulk file transfers for drag and drop, off the engine thread.
//!
//! - Serving: files this Mac offered in a `DragOffer` are served on a bulk
//!   listener bound to the Thunderbolt Bridge, only to the connected peer's
//!   address and only with the drag's random token.
//! - Fetching: when a drop target asks for a promised file, the UI thread
//!   calls [`Transfers::fetch`], which dials the peer and receives into the
//!   destination.
//!
//! Every transfer has a `Progress` the UI polls and can cancel.

use std::collections::VecDeque;
use std::io::{BufReader, BufWriter, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use osm_protocol::v1::{self, FetchRequest};
use osm_transfer::{Progress, ProgressSnapshot, TransferError, receive, send};

/// Offers kept per direction. Drop targets usually fetch right after the
/// drop, but some (Mail, browsers) fetch later; older offers are forgotten.
const MAX_OFFERS: usize = 32;
/// Running transfers per direction. The receiving UI fetches at most two
/// at a time (report §7.3, D); this only stops a misbehaving peer from
/// spawning unbounded sender threads.
const MAX_RUNNING: usize = 16;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// No bytes for this long means the peer is gone; generous because the
/// sender may spend a while scanning a large tree before the manifest.
const IO_TIMEOUT: Duration = Duration::from_secs(60);
const SOCKET_BUFFER: usize = 1024 * 1024;
/// Finished or failed transfers stay listed this long for the UI.
const KEEP_FINISHED: Duration = Duration::from_secs(4);
const KEEP_FAILED: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Clone, Debug)]
pub struct TransferInfo {
    pub id: u64,
    pub direction: Direction,
    pub name: String,
    pub progress: ProgressSnapshot,
    pub error: Option<String>,
    pub finished: bool,
    pub started: Instant,
    /// Where a received item was committed.
    pub destination: Option<PathBuf>,
}

struct Active {
    id: u64,
    direction: Direction,
    name: String,
    progress: Arc<Progress>,
    socket: Option<TcpStream>,
    error: Option<String>,
    finished_at: Option<Instant>,
    started: Instant,
    destination: Option<PathBuf>,
}

struct ServedOffer {
    drag_id: u64,
    token: Vec<u8>,
    paths: Vec<PathBuf>,
}

struct ReceivedOffer {
    drag_id: u64,
    token: Vec<u8>,
    port: u16,
    names: Vec<String>,
}

#[derive(Clone, Copy)]
struct PeerLink {
    ip: Ipv4Addr,
    if_index: u32,
}

#[derive(Default)]
struct Inner {
    peer: Option<PeerLink>,
    /// Survives reconnects, so offers stay valid across a brief drop.
    peer_device: Option<u64>,
    served: VecDeque<ServedOffer>,
    received: VecDeque<ReceivedOffer>,
    active: Vec<Active>,
    listener_port: Option<u16>,
}

#[derive(Clone, Default)]
pub struct Transfers {
    inner: Arc<Mutex<Inner>>,
    next_id: Arc<AtomicU64>,
    /// Bumped to retire the current listener thread.
    generation: Arc<AtomicU64>,
}

impl std::fmt::Debug for Transfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transfers").finish_non_exhaustive()
    }
}

impl Transfers {
    pub fn new() -> Self {
        Self::default()
    }

    /// A control connection to `ip` is up: start a bulk listener on the
    /// bridge and return its port.
    pub fn connected(&self, ip: Ipv4Addr, if_index: u32) -> Option<u16> {
        self.inner.lock().unwrap().peer = Some(PeerLink { ip, if_index });
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let listener = match osm_transport::listen(if_index, 0) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("cannot open bulk listener: {e}");
                self.inner.lock().unwrap().listener_port = None;
                return None;
            }
        };
        let port = listener.local_addr().ok()?.port();
        self.inner.lock().unwrap().listener_port = Some(port);
        let this = self.clone();
        thread::Builder::new()
            .name("osm-bulk-listen".into())
            .spawn(move || this.accept_loop(listener, generation))
            .ok()?;
        Some(port)
    }

    /// Records the peer's device id once its Hello arrives. Offers made to
    /// or by a different device are forgotten.
    pub fn peer_identified(&self, device_id: u64) {
        let mut g = self.inner.lock().unwrap();
        if g.peer_device != Some(device_id) {
            g.served.clear();
            g.received.clear();
            g.peer_device = Some(device_id);
        }
    }

    /// Stops accepting bulk connections. Running transfers continue; they
    /// fail on their own if the cable is gone.
    pub fn disconnected(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.inner.lock().unwrap().listener_port = None;
    }

    pub fn listener_port(&self) -> Option<u16> {
        self.inner.lock().unwrap().listener_port
    }

    /// Makes `paths` fetchable for `drag_id` with `token`.
    pub fn serve(&self, drag_id: u64, token: Vec<u8>, paths: Vec<PathBuf>) {
        let mut g = self.inner.lock().unwrap();
        g.served.push_back(ServedOffer {
            drag_id,
            token,
            paths,
        });
        while g.served.len() > MAX_OFFERS {
            g.served.pop_front();
        }
    }

    /// Remembers what the peer offered so a later drop can fetch it.
    pub fn offer_received(&self, offer: &v1::DragOffer) -> bool {
        let Ok(port) = u16::try_from(offer.bulk_port) else {
            return false;
        };
        if port == 0 || offer.items.is_empty() {
            return false;
        }
        let mut g = self.inner.lock().unwrap();
        g.received.retain(|o| o.drag_id != offer.drag_id);
        g.received.push_back(ReceivedOffer {
            drag_id: offer.drag_id,
            token: offer.token.clone(),
            port,
            names: offer.items.iter().map(|i| i.name.clone()).collect(),
        });
        while g.received.len() > MAX_OFFERS {
            g.received.pop_front();
        }
        true
    }

    /// Fetches item `index` of the peer's drag `drag_id` to `destination`
    /// (the path a drop target asked for). Blocks until the item is
    /// committed, fails or is cancelled, and returns where it landed.
    pub fn fetch(
        &self,
        drag_id: u64,
        index: u32,
        destination: &Path,
    ) -> Result<PathBuf, TransferError> {
        let (Some(dest_dir), Some(dest_name)) = (
            destination.parent(),
            destination.file_name().and_then(|n| n.to_str()),
        ) else {
            return Err(TransferError::InvalidManifest("目标位置无效".into()));
        };
        let (addr, if_index, token, name) = {
            let g = self.inner.lock().unwrap();
            let peer = g.peer.ok_or(TransferError::NotFound)?;
            let offer = g
                .received
                .iter()
                .find(|o| o.drag_id == drag_id)
                .ok_or(TransferError::NotFound)?;
            let name = offer
                .names
                .get(index as usize)
                .ok_or(TransferError::NotFound)?
                .clone();
            (
                SocketAddrV4::new(peer.ip, offer.port),
                peer.if_index,
                offer.token.clone(),
                name,
            )
        };
        let (id, progress) = self.register(Direction::Receive, &name, None)?;
        let result = (|| {
            let stream = connect(if_index, addr)?;
            self.attach_socket(id, &stream);
            if progress.is_cancelled() {
                return Err(TransferError::Cancelled);
            }
            let mut w = BufWriter::new(stream.try_clone()?);
            receive::send_request(
                &mut w,
                FetchRequest {
                    drag_id,
                    token,
                    item_index: index,
                },
            )?;
            w.flush()?;
            let mut r = BufReader::with_capacity(SOCKET_BUFFER, stream);
            receive::receive(&mut r, dest_dir, &name, dest_name, &progress)
        })();
        let result = result.map_err(|e| cancelled_or(e, &progress));
        self.finish(id, &result);
        result
    }

    pub fn list(&self) -> Vec<TransferInfo> {
        let mut g = self.inner.lock().unwrap();
        g.active.retain(|a| match (a.finished_at, &a.error) {
            (Some(t), None) => t.elapsed() < KEEP_FINISHED,
            (Some(t), Some(_)) => t.elapsed() < KEEP_FAILED,
            (None, _) => true,
        });
        g.active
            .iter()
            .map(|a| TransferInfo {
                id: a.id,
                direction: a.direction,
                name: a.name.clone(),
                progress: a.progress.snapshot(),
                error: a.error.clone(),
                finished: a.finished_at.is_some(),
                started: a.started,
                destination: a.destination.clone(),
            })
            .collect()
    }

    pub fn cancel(&self, id: u64) {
        let g = self.inner.lock().unwrap();
        if let Some(a) = g
            .active
            .iter()
            .find(|a| a.id == id && a.finished_at.is_none())
        {
            a.progress.cancel();
            // Unblocks a read or write that is waiting on the network.
            if let Some(s) = &a.socket {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Removes a finished or failed transfer from the list.
    pub fn dismiss(&self, id: u64) {
        self.inner
            .lock()
            .unwrap()
            .active
            .retain(|a| a.id != id || a.finished_at.is_none());
    }

    /// Cancels everything; used at shutdown.
    pub fn cancel_all(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let ids: Vec<u64> = self
            .inner
            .lock()
            .unwrap()
            .active
            .iter()
            .map(|a| a.id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }

    fn register(
        &self,
        direction: Direction,
        name: &str,
        socket: Option<&TcpStream>,
    ) -> Result<(u64, Arc<Progress>), TransferError> {
        let mut g = self.inner.lock().unwrap();
        let running = g
            .active
            .iter()
            .filter(|a| a.direction == direction && a.finished_at.is_none())
            .count();
        if running >= MAX_RUNNING {
            // Far beyond what a drop produces; refuse instead of queueing
            // unbounded threads.
            return Err(TransferError::Limit("同时进行的传输过多".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let progress = Arc::new(Progress::new());
        g.active.push(Active {
            id,
            direction,
            name: name.to_owned(),
            progress: progress.clone(),
            socket: socket.and_then(|s| s.try_clone().ok()),
            error: None,
            finished_at: None,
            started: Instant::now(),
            destination: None,
        });
        Ok((id, progress))
    }

    fn attach_socket(&self, id: u64, stream: &TcpStream) {
        let mut g = self.inner.lock().unwrap();
        if let Some(a) = g.active.iter_mut().find(|a| a.id == id) {
            a.socket = stream.try_clone().ok();
        }
    }

    fn finish(&self, id: u64, result: &Result<PathBuf, TransferError>) {
        let mut g = self.inner.lock().unwrap();
        if let Some(a) = g.active.iter_mut().find(|a| a.id == id) {
            a.finished_at = Some(Instant::now());
            a.socket = None;
            match result {
                Ok(p) => a.destination = Some(p.clone()),
                // A cancel the user asked for is not an error worth showing.
                Err(TransferError::Cancelled) => {
                    a.finished_at = Some(Instant::now() - KEEP_FINISHED);
                }
                Err(e) => a.error = Some(e.to_string()),
            }
        }
    }

    fn accept_loop(self, listener: TcpListener, generation: u64) {
        while self.generation.load(Ordering::Acquire) == generation {
            match listener.accept() {
                Ok((stream, from)) => {
                    let allowed = self.inner.lock().unwrap().peer.map(|p| p.ip);
                    let std::net::SocketAddr::V4(from) = from else {
                        continue;
                    };
                    if allowed != Some(*from.ip()) {
                        tracing::warn!("rejected bulk connection from a non-peer address");
                        continue;
                    }
                    let this = self.clone();
                    let _ = thread::Builder::new()
                        .name("osm-bulk-send".into())
                        .spawn(move || this.serve_connection(stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    tracing::debug!("bulk accept failed: {e}");
                    thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn serve_connection(&self, stream: TcpStream) {
        if stream.set_nonblocking(false).is_err() || tune_bulk(&stream).is_err() {
            return;
        }
        let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut w = BufWriter::with_capacity(SOCKET_BUFFER, stream);
        let req = match send::read_request(&mut BufReader::new(read_half)) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("bad bulk request: {e}");
                return;
            }
        };
        let path = {
            let g = self.inner.lock().unwrap();
            g.served
                .iter()
                .find(|o| o.drag_id == req.drag_id && constant_time_eq(&o.token, &req.token))
                .and_then(|o| o.paths.get(req.item_index as usize).cloned())
        };
        let Some(path) = path else {
            send::refuse(&mut w, &TransferError::NotFound);
            let _ = w.flush();
            return;
        };
        let name = osm_transfer::manifest::item_display_name(&path);
        let Ok((id, progress)) = self.register(Direction::Send, &name, Some(w.get_ref())) else {
            send::refuse(&mut w, &TransferError::Limit("同时进行的传输过多".into()));
            let _ = w.flush();
            return;
        };
        let result = send::serve(&mut w, &path, &progress).and_then(|()| {
            w.flush()?;
            // Wait for the receiver to close first, so its last reads are
            // not cut off by our close.
            let _ = w.get_ref().shutdown(std::net::Shutdown::Write);
            Ok(path.clone())
        });
        let result = result.map_err(|e| cancelled_or(e, &progress));
        if let Err(e) = &result {
            tracing::info!("serving a dragged item failed: {e}");
        }
        self.finish(id, &result);
    }
}

fn cancelled_or(e: TransferError, progress: &Progress) -> TransferError {
    if progress.is_cancelled() {
        TransferError::Cancelled
    } else {
        e
    }
}

fn connect(if_index: u32, addr: SocketAddrV4) -> Result<TcpStream, TransferError> {
    let stream = osm_transport::connect_with_timeout(if_index, addr, CONNECT_TIMEOUT)?;
    tune_bulk(&stream)?;
    Ok(stream)
}

fn tune_bulk(stream: &TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    osm_transport::tune_bulk(stream)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn fetch_without_offer_is_not_found() {
        let t = Transfers::new();
        assert!(matches!(
            t.fetch(1, 0, Path::new("/tmp/x")),
            Err(TransferError::NotFound)
        ));
    }

    #[test]
    fn rejects_offer_without_port_or_items() {
        let t = Transfers::new();
        let mut offer = v1::DragOffer {
            drag_id: 1,
            epoch: 1,
            token: vec![1; 16],
            bulk_port: 0,
            items: vec![v1::DragItem {
                name: "a".into(),
                kind: v1::EntryKind::File as i32,
                size: 1,
            }],
        };
        assert!(!t.offer_received(&offer));
        offer.bulk_port = 70_000;
        assert!(!t.offer_received(&offer));
        offer.bulk_port = 5000;
        assert!(t.offer_received(&offer));
        offer.items.clear();
        assert!(!t.offer_received(&offer));
    }
}
