//! Client for the herdr socket API: newline-delimited JSON over a Unix socket,
//! a Windows named pipe, or a child-process bridge (`herdr remote-api-bridge`).
//!
//! * [`protocol`]: wire types and line parsing.
//! * [`snapshot`]: `session.snapshot` records.
//! * [`transport`]: one-shot connections (herdr serves one request per
//!   connection; only subscriptions stay open).
//! * [`client`]: [`Client::request`] and [`Client::subscribe`].

pub mod client;
pub mod protocol;
pub mod snapshot;
pub mod transport;

#[cfg(all(unix, feature = "test-support"))]
pub mod test_support;

pub use client::{Client, ClientError, EventStream};
pub use protocol::{
    AgentStatus, ErrorBody, Event, Method, PaneAgentStatusChanged, ResponseResult, Subscription,
};
pub use snapshot::{AgentInfo, PaneInfo, ReadResult, SessionSnapshot, WorkspaceInfo};
pub use transport::{Connection, Transport, TransportError};
