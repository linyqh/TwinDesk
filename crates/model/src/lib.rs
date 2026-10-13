//! Platform-independent data model shared by every OpenShareMouse crate.
//!
//! Nothing in here touches the OS, the network or the clock. Types follow the
//! report's §5.1 table; invariants that the type system cannot express are
//! documented on the type and enforced by the owning module in `osm-core`.

pub mod capability;
pub mod geometry;
pub mod ids;
pub mod profile;

pub use capability::{Capability, CapabilityName, CapabilityState};
pub use geometry::{LogicalPoint, LogicalRect, NormalizedPoint};
pub use ids::{BootId, DeviceId, LeaseEpoch, SessionId, TopologyRevision};
