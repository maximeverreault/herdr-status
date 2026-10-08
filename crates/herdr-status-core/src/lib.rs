//! Core of herdr-status: configuration, per-server watchers and the sorted
//! agent list the UI renders. No UI code lives here.
pub mod config;
pub mod launchers;
pub mod model;
pub mod paths;
pub mod reducer;
pub mod remote_connect;
pub mod remote_probe;
pub mod transport_source;
