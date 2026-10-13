//! Transport layer. Placeholder until NET-01.
//!
//! Planned shape (report §7.3):
//! - one QUIC connection per peer pair, ALPN `osm/1`, mutual TLS with
//!   self-signed device certificates checked against the trust store;
//! - control stream, one input-state stream per lease, pointer samples as
//!   datagrams capped at `min(1200, max_datagram_size)`;
//! - bulk data on a separate, rate-limited connection;
//! - mDNS service `_opensharemouse._udp`, default port 49001.

/// Default listening port. A project default, not an IANA assignment.
pub const DEFAULT_PORT: u16 = 49001;

/// Bonjour service type. Must match `NSBonjourServices` in the app's
/// Info.plist, or macOS local network privacy silently blocks discovery.
pub const SERVICE_TYPE: &str = "_opensharemouse._udp";

pub const ALPN: &[u8] = osm_protocol::ALPN;

/// Datagram payload ceiling before the connection reports its real limit.
pub const MAX_DATAGRAM: usize = 1200;
