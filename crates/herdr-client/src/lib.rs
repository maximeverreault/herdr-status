//! Client for the herdr socket API: newline-delimited JSON over a Unix socket,
//! a Windows named pipe, or a child-process bridge (`herdr remote-api-bridge`).

pub mod client;
pub mod protocol;
pub mod snapshot;
pub mod transport;

#[cfg(all(unix, feature = "test-support"))]
pub mod test_support;
