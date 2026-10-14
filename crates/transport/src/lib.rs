//! Peer discovery and connection over the Thunderbolt Bridge.
//!
//! v1 deliberately talks only over the Thunderbolt cable: beacons are
//! broadcast on the bridge interface and sockets are bound to it with
//! `IP_BOUND_IF`, so nothing leaks onto Wi-Fi.
//!
//! - UDP broadcast beacons on `DISCOVERY_PORT` once a second.
//! - One TCP connection carrying length-prefixed protobuf frames. The device
//!   with the lower id dials, the other accepts, so exactly one connection
//!   exists without any tie-breaking handshake.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::os::fd::AsRawFd;
use std::time::Duration;

use bytes::BytesMut;
use osm_protocol::v1::{Beacon, Envelope};
use osm_protocol::{FrameError, decode_beacon, decode_frame, encode_beacon, encode_frame};
use socket2::{Domain, Protocol, Socket, Type};

/// Default TCP port. A project default, not an IANA assignment.
pub const DEFAULT_PORT: u16 = 49001;
/// UDP port for discovery beacons.
pub const DISCOVERY_PORT: u16 = 49002;
/// Kept for the Info.plist and a future Bonjour implementation.
pub const SERVICE_TYPE: &str = "_opensharemouse._udp";

/// Restricts a socket to one interface (Darwin `IP_BOUND_IF`).
fn bind_to_interface(fd: i32, if_index: u32) -> io::Result<()> {
    let idx = if_index as libc::c_int;
    // SAFETY: setsockopt with a valid fd and an int-sized option value.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            libc::IP_BOUND_IF,
            &idx as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub device_id: u64,
    pub name: String,
    pub addr: SocketAddrV4,
}

/// Broadcast beacon sender and listener, bound to the bridge interface.
#[derive(Debug)]
pub struct Discovery {
    socket: UdpSocket,
    broadcast: SocketAddrV4,
    beacon: Vec<u8>,
    own_id: u64,
}

impl Discovery {
    pub fn new(
        if_index: u32,
        broadcast: Ipv4Addr,
        own_id: u64,
        name: &str,
        tcp_port: u16,
    ) -> io::Result<Self> {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;
        s.set_reuse_port(true)?;
        s.set_broadcast(true)?;
        bind_to_interface(s.as_raw_fd(), if_index)?;
        s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).into())?;
        let socket: UdpSocket = s.into();
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        Ok(Self {
            socket,
            broadcast: SocketAddrV4::new(broadcast, DISCOVERY_PORT),
            beacon: encode_beacon(own_id, name, tcp_port),
            own_id,
        })
    }

    pub fn announce(&self) -> io::Result<()> {
        self.socket
            .send_to(&self.beacon, self.broadcast)
            .map(|_| ())
    }

    /// Waits up to the read timeout for one beacon from another device.
    pub fn receive(&self) -> io::Result<Option<DiscoveredPeer>> {
        let mut buf = [0u8; 600];
        let (n, from) = match self.socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let SocketAddr::V4(from) = from else {
            return Ok(None);
        };
        Ok(decode_beacon(&buf[..n]).and_then(|b: Beacon| {
            (b.device_id != self.own_id).then(|| DiscoveredPeer {
                device_id: b.device_id,
                name: b.name,
                addr: SocketAddrV4::new(*from.ip(), b.tcp_port as u16),
            })
        }))
    }
}

/// TCP listener on the bridge interface.
pub fn listen(if_index: u32, port: u16) -> io::Result<TcpListener> {
    let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    s.set_reuse_address(true)?;
    bind_to_interface(s.as_raw_fd(), if_index)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    s.listen(4)?;
    let l: TcpListener = s.into();
    l.set_nonblocking(true)?;
    Ok(l)
}

pub fn connect(if_index: u32, addr: SocketAddrV4) -> io::Result<TcpStream> {
    connect_with_timeout(if_index, addr, Duration::from_secs(2))
}

pub fn connect_with_timeout(
    if_index: u32,
    addr: SocketAddrV4,
    timeout: Duration,
) -> io::Result<TcpStream> {
    let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    bind_to_interface(s.as_raw_fd(), if_index)?;
    s.connect_timeout(&SocketAddr::V4(addr).into(), timeout)?;
    Ok(s.into())
}

/// Input latency matters more than throughput: disable Nagle and keep the
/// connection alive at the TCP level as a backstop to our own pings.
pub fn tune(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    socket2::SockRef::from(stream).set_keepalive(true)?;
    Ok(())
}

/// Darwin `SO_NET_SERVICE_TYPE` / `NET_SERVICE_TYPE_BK` (sys/socket.h).
const SO_NET_SERVICE_TYPE: libc::c_int = 0x1116;
const NET_SERVICE_TYPE_BK: libc::c_int = 1;

/// Bulk connections carry file data. Marking them background lets the
/// interface queue send control frames (input) first when both compete,
/// and large buffers keep a fast link busy.
pub fn tune_bulk(stream: &TcpStream) -> io::Result<()> {
    let v = NET_SERVICE_TYPE_BK;
    // SAFETY: setsockopt with a valid fd and an int-sized option value.
    let rc = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_NET_SERVICE_TYPE,
            &v as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let s = socket2::SockRef::from(stream);
    s.set_send_buffer_size(4 * 1024 * 1024)?;
    s.set_recv_buffer_size(4 * 1024 * 1024)?;
    Ok(())
}

/// Reads frames off a blocking stream.
#[derive(Debug)]
pub struct FrameReader {
    stream: TcpStream,
    buf: BytesMut,
}

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    Frame(FrameError),
    Closed,
}

impl FrameReader {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            buf: BytesMut::with_capacity(8192),
        }
    }

    pub fn read_frame(&mut self) -> Result<Envelope, ReadError> {
        loop {
            if let Some(env) = decode_frame(&mut self.buf).map_err(ReadError::Frame)? {
                return Ok(env);
            }
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ReadError::Closed),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(ReadError::Io(e)),
            }
        }
    }
}

/// Writes frames to a blocking stream.
#[derive(Debug)]
pub struct FrameWriter {
    stream: TcpStream,
    buf: BytesMut,
}

impl FrameWriter {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            buf: BytesMut::with_capacity(256),
        }
    }

    pub fn send(&mut self, env: &Envelope) -> io::Result<()> {
        self.buf.clear();
        encode_frame(env, &mut self.buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.stream.write_all(&self.buf)
    }

    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osm_protocol::envelope;
    use osm_protocol::v1::{Ping, envelope::Payload};

    #[test]
    fn frames_round_trip_over_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = FrameReader::new(s);
            (0..3).map(|_| r.read_frame().unwrap()).collect::<Vec<_>>()
        });
        let s = TcpStream::connect(addr).unwrap();
        tune(&s).unwrap();
        let mut w = FrameWriter::new(s);
        for n in 0..3 {
            w.send(&envelope(Payload::Ping(Ping { nonce: n }))).unwrap();
        }
        let got = t.join().unwrap();
        let nonces: Vec<u64> = got
            .into_iter()
            .map(|e| match e.payload {
                Some(Payload::Ping(p)) => p.nonce,
                _ => panic!(),
            })
            .collect();
        assert_eq!(nonces, vec![0, 1, 2]);
    }

    #[test]
    fn reader_reports_clean_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let s = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        drop(s);
        assert!(matches!(
            FrameReader::new(server).read_frame(),
            Err(ReadError::Closed)
        ));
    }
}
