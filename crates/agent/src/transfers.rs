//! Explicit file portal transfers, independent of pointer ownership.
//! Offers are created only after a local drop. The receiver always saves
//! into its locally configured Downloads directory and acknowledges commit.

use std::collections::VecDeque;
use std::io::{BufReader, BufWriter, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use osm_protocol::v1::{self, FetchRequest, bulk_frame::Body};
use osm_protocol::{read_bulk, write_bulk};
use osm_transfer::{Progress, ProgressSnapshot, TransferError, receive, send};

/// Recent received IDs prevent duplicate offers from creating duplicate files.
const MAX_OFFERS: usize = 32;
/// Preserve the existing bound of 16 outstanding transfers per direction.
/// A folder counts as one transfer regardless of its number of descendants.
pub const MAX_RUNNING: usize = 16;
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
    items: Vec<Option<OfferedItem>>,
}

struct OfferedItem {
    path: PathBuf,
    id: u64,
    progress: Arc<Progress>,
}

struct ReceiveJob {
    peer: PeerLink,
    port: u16,
    request: FetchRequest,
    downloads: PathBuf,
    name: String,
    id: u64,
    progress: Arc<Progress>,
}

#[derive(Clone, Copy)]
struct PeerLink {
    ip: Ipv4Addr,
    if_index: u32,
}

#[derive(Default)]
struct Inner {
    peer: Option<PeerLink>,
    /// Identity of the current peer.
    peer_device: Option<u64>,
    served: VecDeque<ServedOffer>,
    received: VecDeque<u64>,
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

    /// Retire pending offers on disconnect. Already connected bulk streams
    /// may finish; unclaimed offers must not become accessible to a new peer.
    pub fn disconnected(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let mut g = self.inner.lock().unwrap();
        g.listener_port = None;
        g.peer = None;
        for offer in std::mem::take(&mut g.served) {
            for item in offer.items.into_iter().flatten() {
                finish_locked(
                    &mut g,
                    item.id,
                    &Err(TransferError::Remote("连接已断开，请重新投放".into())),
                );
            }
        }
    }

    /// Metadata has already been inspected off the input thread.
    pub fn offer_files(
        &self,
        transfer_id: u64,
        token: Vec<u8>,
        port: u16,
        paths: Vec<PathBuf>,
        items: Vec<v1::DragItem>,
    ) -> Result<v1::PortalOffer, TransferError> {
        let offer = v1::PortalOffer {
            transfer_id,
            token: token.clone(),
            bulk_port: port.into(),
            items,
        };
        validate_offer(&offer)?;
        if paths.len() != offer.items.len() {
            return Err(TransferError::NotFound);
        }
        let mut g = self.inner.lock().unwrap();
        ensure_capacity(&g, Direction::Send, paths.len())?;
        let mut pending = Vec::new();
        for (path, item) in paths.into_iter().zip(&offer.items) {
            let (id, progress) = self.register_locked(&mut g, Direction::Send, &item.name);
            pending.push(Some(OfferedItem { path, id, progress }));
        }
        g.served.push_back(ServedOffer {
            drag_id: transfer_id,
            token,
            items: pending,
        });
        Ok(offer)
    }

    /// Reserves all rows before starting any work: capacity failure cannot
    /// silently accept only part of a multi-file drop.
    pub fn receive_offer(
        &self,
        offer: v1::PortalOffer,
        downloads: PathBuf,
    ) -> Result<(), TransferError> {
        validate_offer(&offer)?;
        if !downloads.is_absolute() {
            return Err(TransferError::Remote("本机下载目录无效".into()));
        }
        let (peer, jobs) = {
            let mut g = self.inner.lock().unwrap();
            let peer = g.peer.ok_or(TransferError::NotFound)?;
            if g.received.contains(&offer.transfer_id) {
                // Idempotent replay: do not reject or cancel the original job.
                return Ok(());
            }
            ensure_capacity(&g, Direction::Receive, offer.items.len())?;
            let jobs: Vec<_> = offer
                .items
                .iter()
                .map(|i| self.register_locked(&mut g, Direction::Receive, &i.name))
                .collect();
            g.received.push_back(offer.transfer_id);
            while g.received.len() > MAX_OFFERS {
                g.received.pop_front();
            }
            (peer, jobs)
        };
        for (index, (item, (id, progress))) in offer.items.into_iter().zip(jobs).enumerate() {
            let this = self.clone();
            let dest = downloads.clone();
            let token = offer.token.clone();
            let transfer_id = offer.transfer_id;
            let port = offer.bulk_port as u16;
            if let Err(e) = thread::Builder::new()
                .name("osm-portal-receive".into())
                .spawn(move || {
                    let job = ReceiveJob {
                        peer,
                        port,
                        request: FetchRequest {
                            drag_id: transfer_id,
                            token,
                            item_index: index as u32,
                        },
                        downloads: dest,
                        name: item.name,
                        id,
                        progress,
                    };
                    let result = this
                        .fetch_item(&job)
                        .map_err(|e| cancelled_or(e, &job.progress));
                    this.finish(id, &result);
                })
            {
                self.finish(id, &Err(e.into()));
            }
        }
        Ok(())
    }

    fn fetch_item(&self, job: &ReceiveJob) -> Result<PathBuf, TransferError> {
        let ReceiveJob {
            peer,
            port,
            request,
            downloads,
            name,
            id,
            progress,
        } = job;
        if progress.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let stream = connect(peer.if_index, SocketAddrV4::new(peer.ip, *port))?;
        self.attach_socket(*id, &stream);
        if progress.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let mut w = BufWriter::new(stream.try_clone()?);
        receive::send_request(&mut w, request.clone())?;
        w.flush()?;
        let mut r = BufReader::with_capacity(SOCKET_BUFFER, stream);
        let result = (|| {
            std::fs::create_dir_all(downloads)?;
            receive::receive(&mut r, downloads, name, name, progress)
        })();
        // The sender reads this only after writing the payload. On a receive
        // failure the connection may already be closed; success on this Mac
        // still means a committed file, even if its receipt is lost.
        let receipt = v1::BulkFrame {
            body: Some(Body::Receipt(v1::TransferReceipt {
                error: result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            })),
        };
        let _ = write_bulk(&mut w, &receipt, &mut Default::default()).and_then(|()| w.flush());
        result
    }

    pub fn reject_offer(&self, transfer_id: u64, message: &str) {
        let mut g = self.inner.lock().unwrap();
        if let Some(pos) = g.served.iter().position(|o| o.drag_id == transfer_id) {
            let offer = g.served.remove(pos).unwrap();
            for item in offer.items.into_iter().flatten() {
                finish_locked(
                    &mut g,
                    item.id,
                    &Err(TransferError::Remote(message.chars().take(512).collect())),
                );
            }
        }
    }

    pub fn expire_pending(&self) {
        let mut g = self.inner.lock().unwrap();
        let expired: Vec<_> = g
            .active
            .iter()
            .filter(|a| {
                a.direction == Direction::Send
                    && a.finished_at.is_none()
                    && a.socket.is_none()
                    && (a.started.elapsed() >= IO_TIMEOUT || a.progress.is_cancelled())
            })
            .map(|a| (a.id, a.progress.is_cancelled()))
            .collect();
        for (id, cancelled) in expired {
            for offer in &mut g.served {
                for item in &mut offer.items {
                    if item.as_ref().is_some_and(|i| i.id == id) {
                        *item = None;
                    }
                }
            }
            let error = if cancelled {
                TransferError::Cancelled
            } else {
                TransferError::Remote("对方未开始接收，请检查连接后重新投放".into())
            };
            finish_locked(&mut g, id, &Err(error));
        }
        g.served.retain(|o| o.items.iter().any(Option::is_some));
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
                progress: {
                    let mut p = a.progress.snapshot();
                    if p.phase == osm_transfer::Phase::Finished && a.finished_at.is_none() {
                        p.phase = osm_transfer::Phase::Committing;
                    }
                    p
                },
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

    fn register_locked(
        &self,
        g: &mut Inner,
        direction: Direction,
        name: &str,
    ) -> (u64, Arc<Progress>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let progress = Arc::new(Progress::new());
        g.active.push(Active {
            id,
            direction,
            name: name.to_owned(),
            progress: progress.clone(),
            socket: None,
            error: None,
            finished_at: None,
            started: Instant::now(),
            destination: None,
        });
        (id, progress)
    }

    fn attach_socket(&self, id: u64, stream: &TcpStream) {
        let mut g = self.inner.lock().unwrap();
        if let Some(a) = g.active.iter_mut().find(|a| a.id == id) {
            a.socket = stream.try_clone().ok();
        }
    }

    fn finish(&self, id: u64, result: &Result<PathBuf, TransferError>) {
        finish_locked(&mut self.inner.lock().unwrap(), id, result);
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
        let mut r = BufReader::new(read_half);
        let req = match send::read_request(&mut r) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("bad bulk request: {e}");
                return;
            }
        };
        let Ok(cancel_socket) = w.get_ref().try_clone() else {
            return;
        };
        let item = {
            let mut g = self.inner.lock().unwrap();
            let item = g
                .served
                .iter_mut()
                .find(|o| o.drag_id == req.drag_id && constant_time_eq(&o.token, &req.token))
                .and_then(|o| o.items.get_mut(req.item_index as usize))
                .and_then(Option::take);
            // Claim and mark running under one lock: a pending-offer timeout
            // must not race the gap before a worker attaches its socket.
            if let Some(item) = &item
                && let Some(active) = g.active.iter_mut().find(|a| a.id == item.id)
            {
                active.socket = Some(cancel_socket);
            }
            item
        };
        let Some(item) = item else {
            send::refuse(&mut w, &TransferError::NotFound);
            let _ = w.flush();
            return;
        };
        let OfferedItem { path, id, progress } = item;
        let _ = w.get_ref().set_read_timeout(Some(IO_TIMEOUT));
        let result = (|| {
            if progress.is_cancelled() {
                return Err(TransferError::Cancelled);
            }
            send::serve(&mut w, &path, &progress)?;
            w.flush()?;
            match read_bulk(&mut r, &mut Vec::new())? {
                Body::Receipt(receipt) if receipt.error.is_empty() => Ok(path),
                Body::Receipt(receipt) => Err(TransferError::Remote(
                    receipt.error.chars().take(512).collect(),
                )),
                _ => Err(TransferError::Protocol("未收到对方保存文件的确认".into())),
            }
        })()
        .map_err(|e| cancelled_or(e, &progress));
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

fn validate_offer(offer: &v1::PortalOffer) -> Result<(), TransferError> {
    if offer.bulk_port == 0 || offer.bulk_port > u16::MAX as u32 || offer.token.len() != 16 {
        return Err(TransferError::Protocol("文件传送通道参数无效".into()));
    }
    if offer.items.is_empty() || offer.items.len() > MAX_RUNNING {
        return Err(TransferError::Limit("每次最多 16 个文件或文件夹".into()));
    }
    for i in &offer.items {
        if !osm_transfer::path::validate_macos_path(&i.name).is_ok_and(|p| p.len() == 1)
            || !matches!(
                v1::EntryKind::try_from(i.kind),
                Ok(v1::EntryKind::File | v1::EntryKind::Directory | v1::EntryKind::Symlink)
            )
        {
            return Err(TransferError::InvalidManifest("文件名或类型无效".into()));
        }
    }
    Ok(())
}

fn ensure_capacity(g: &Inner, direction: Direction, count: usize) -> Result<(), TransferError> {
    if g.active
        .iter()
        .filter(|a| a.direction == direction && a.finished_at.is_none())
        .count()
        + count
        > MAX_RUNNING
    {
        return Err(TransferError::Limit(
            "传送通道忙，请等待当前传输完成后重试".into(),
        ));
    }
    Ok(())
}

fn finish_locked(g: &mut Inner, id: u64, result: &Result<PathBuf, TransferError>) {
    if let Some(a) = g.active.iter_mut().find(|a| a.id == id) {
        a.finished_at = Some(Instant::now());
        a.socket = None;
        match result {
            Ok(p) if a.direction == Direction::Receive => a.destination = Some(p.clone()),
            Ok(_) => {}
            Err(e) => a.error = Some(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pair {
        a: Transfers,
        b: Transfers,
        port: u16,
    }
    impl Pair {
        fn new() -> Self {
            let a = Transfers::new();
            let b = Transfers::new();
            // Interface 0 leaves the loopback test sockets unbound.
            let port = a.connected(Ipv4Addr::LOCALHOST, 0).unwrap();
            b.connected(Ipv4Addr::LOCALHOST, 0).unwrap();
            a.peer_identified(2);
            b.peer_identified(1);
            Self { a, b, port }
        }
        fn offer(&self, paths: Vec<PathBuf>, id: u64) -> v1::PortalOffer {
            let items = paths
                .iter()
                .map(|p| osm_transfer::describe_item(p).unwrap())
                .collect();
            self.a
                .offer_files(id, vec![7; 16], self.port, paths, items)
                .unwrap()
        }
    }
    impl Drop for Pair {
        fn drop(&mut self) {
            self.a.cancel_all();
            self.b.cancel_all();
            self.a.disconnected();
            self.b.disconnected();
        }
    }
    fn wait_for(t: &Transfers, predicate: impl Fn(&[TransferInfo]) -> bool) -> Vec<TransferInfo> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let rows = t.list();
            if predicate(&rows) {
                return rows;
            }
            assert!(Instant::now() < deadline, "transfer stalled: {rows:?}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn finished(t: &Transfers, count: usize) -> Vec<TransferInfo> {
        wait_for(t, |r| r.len() == count && r.iter().all(|r| r.finished))
    }

    #[test]
    fn portal_copies_files_and_folders_to_downloads_without_overwrite_or_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let downloads = tmp.path().join("Downloads");
        std::fs::create_dir_all(source.join("文件夹/空目录")).unwrap();
        std::fs::create_dir(&downloads).unwrap();
        let payload = vec![42; osm_protocol::CHUNK_SIZE * 3 + 13];
        std::fs::write(source.join("文件夹/payload.bin"), &payload).unwrap();
        std::fs::write(source.join("zero.txt"), []).unwrap();
        std::fs::write(downloads.join("zero.txt"), b"existing").unwrap();
        let pair = Pair::new();
        let offer = pair.offer(vec![source.join("文件夹"), source.join("zero.txt")], 10);
        pair.b
            .receive_offer(offer.clone(), downloads.clone())
            .unwrap();
        pair.b.receive_offer(offer, downloads.clone()).unwrap();
        for rows in [finished(&pair.a, 2), finished(&pair.b, 2)] {
            assert!(rows.iter().all(|r| r.error.is_none()), "{rows:?}");
        }
        assert_eq!(
            std::fs::read(downloads.join("文件夹/payload.bin")).unwrap(),
            payload
        );
        assert!(downloads.join("文件夹/空目录").is_dir());
        assert_eq!(
            std::fs::read(downloads.join("zero.txt")).unwrap(),
            b"existing"
        );
        assert!(downloads.join("zero 2.txt").is_file());
        assert_eq!(std::fs::read_dir(&downloads).unwrap().count(), 3);
        assert_eq!(
            std::fs::read(source.join("文件夹/payload.bin")).unwrap(),
            payload
        );
        assert!(
            pair.b
                .list()
                .iter()
                .all(|r| r.destination.as_ref().unwrap().starts_with(&downloads))
        );
        assert!(pair.a.list().iter().all(|r| r.destination.is_none()));

        // Reverse direction uses exactly the same receiver-selected path.
        let reverse_downloads = tmp.path().join("OtherDownloads");
        let path = downloads.join("zero.txt");
        let port = pair.b.inner.lock().unwrap().listener_port.unwrap();
        let offer = pair
            .b
            .offer_files(
                11,
                vec![8; 16],
                port,
                vec![path.clone()],
                vec![osm_transfer::describe_item(&path).unwrap()],
            )
            .unwrap();
        pair.a
            .receive_offer(offer, reverse_downloads.clone())
            .unwrap();
        assert!(finished(&pair.a, 3).iter().all(|r| r.error.is_none()));
        assert!(finished(&pair.b, 3).iter().all(|r| r.error.is_none()));
        assert_eq!(
            std::fs::read(reverse_downloads.join("zero.txt")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn sender_needs_commit_receipt_and_reports_rejection_or_missing_receipt() {
        for receipt in [Some(""), Some("对方磁盘不可写"), None] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("file.txt");
            std::fs::write(&path, b"payload").unwrap();
            let dest = tmp.path().join("Downloads");
            std::fs::create_dir(&dest).unwrap();
            let pair = Pair::new();
            let offer = pair.offer(vec![path], 20);
            let stream = connect(0, SocketAddrV4::new(Ipv4Addr::LOCALHOST, pair.port)).unwrap();
            let mut writer = stream.try_clone().unwrap();
            receive::send_request(
                &mut writer,
                FetchRequest {
                    drag_id: offer.transfer_id,
                    token: offer.token,
                    item_index: 0,
                },
            )
            .unwrap();
            let mut reader = BufReader::new(stream);
            receive::receive(&mut reader, &dest, "file.txt", "file.txt", &Progress::new()).unwrap();
            let rows = wait_for(&pair.a, |r| {
                r[0].progress.phase == osm_transfer::Phase::Committing
            });
            assert!(!rows[0].finished);
            if let Some(error) = receipt {
                write_bulk(
                    &mut writer,
                    &v1::BulkFrame {
                        body: Some(Body::Receipt(v1::TransferReceipt {
                            error: error.into(),
                        })),
                    },
                    &mut Default::default(),
                )
                .unwrap();
            } else {
                writer.shutdown(std::net::Shutdown::Both).unwrap();
            }
            let rows = finished(&pair.a, 1);
            assert_eq!(rows[0].error.is_none(), receipt == Some(""));
            if receipt == Some("对方磁盘不可写") {
                assert_eq!(rows[0].error.as_deref(), receipt);
            }
        }
    }

    #[test]
    fn rejects_unsafe_names_invalid_tokens_and_excess_items_before_writing() {
        let valid = v1::PortalOffer {
            transfer_id: 1,
            token: vec![1; 16],
            bulk_port: 5000,
            items: vec![v1::DragItem {
                name: "ok".into(),
                kind: v1::EntryKind::File as i32,
                size: 0,
            }],
        };
        for name in ["/tmp/escape", "..", "../escape", "dir/file", "", "a\0b"] {
            let mut offer = valid.clone();
            offer.items[0].name = name.into();
            assert!(validate_offer(&offer).is_err(), "accepted {name:?}");
        }
        for port in [0, 70_000] {
            let mut offer = valid.clone();
            offer.bulk_port = port;
            assert!(validate_offer(&offer).is_err());
        }
        let mut offer = valid.clone();
        offer.token.clear();
        assert!(validate_offer(&offer).is_err());
        let mut offer = valid.clone();
        offer.items = vec![valid.items[0].clone(); 17];
        assert!(validate_offer(&offer).is_err());
        assert!(validate_offer(&valid).is_ok());
    }

    #[test]
    fn pending_cancel_disconnect_reject_and_timeout_are_visible_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file");
        std::fs::write(&path, b"test").unwrap();
        let pair = Pair::new();
        pair.offer(vec![path.clone()], 30);
        pair.a.cancel(pair.a.list()[0].id);
        pair.a.expire_pending();
        assert_eq!(pair.a.list()[0].error.as_deref(), Some("已取消"));
        pair.offer(vec![path.clone()], 31);
        pair.a.reject_offer(31, "对方共享已暂停");
        assert_eq!(pair.a.list()[1].error.as_deref(), Some("对方共享已暂停"));
        pair.offer(vec![path.clone()], 32);
        pair.a.inner.lock().unwrap().active[2].started = Instant::now() - IO_TIMEOUT;
        pair.a.expire_pending();
        assert!(pair.a.list()[2].error.is_some());
        pair.offer(vec![path], 33);
        pair.a.disconnected();
        assert!(
            pair.a
                .list()
                .iter()
                .all(|r| r.finished && r.error.is_some())
        );
        assert!(pair.a.inner.lock().unwrap().served.is_empty());
    }

    #[test]
    fn invalid_downloads_directory_fails_without_creating_a_partial_file() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::write(&source, b"test").unwrap();
        let not_dir = tmp.path().join("blocked");
        std::fs::write(&not_dir, b"keep").unwrap();
        let pair = Pair::new();
        let offer = pair.offer(vec![source], 40);
        pair.b
            .receive_offer(offer, not_dir.join("Downloads"))
            .unwrap();
        assert!(finished(&pair.b, 1)[0].error.is_some());
        assert!(finished(&pair.a, 1)[0].error.is_some());
        assert_eq!(std::fs::read(not_dir).unwrap(), b"keep");
    }

    #[test]
    fn capacity_rejection_does_not_partially_register_a_batch() {
        let pair = Pair::new();
        let mut g = pair.b.inner.lock().unwrap();
        for _ in 0..15 {
            pair.b.register_locked(&mut g, Direction::Receive, "busy");
        }
        drop(g);
        let item = v1::DragItem {
            name: "a".into(),
            kind: v1::EntryKind::File as i32,
            size: 0,
        };
        let offer = v1::PortalOffer {
            transfer_id: 50,
            token: vec![1; 16],
            bulk_port: 5000,
            items: vec![item.clone(), item],
        };
        assert!(
            pair.b
                .receive_offer(offer, PathBuf::from("/tmp/unused-downloads"))
                .is_err()
        );
        assert_eq!(pair.b.list().len(), 15);
        assert!(pair.b.inner.lock().unwrap().received.is_empty());
    }
}
