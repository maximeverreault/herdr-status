//! Core of herdr-status: configuration, per-server watchers and the sorted
//! agent list the UI renders. No UI code lives here.
//!
//! Entry points for the app: [`config::Config`], [`hub::Hub`],
//! [`hub::default_source_factory`], [`model::AgentList`], [`actions`],
//! [`prefill`], [`paths`].

pub mod actions;
pub mod config;
pub mod hub;
pub mod launchers;
pub mod model;
pub mod paths;
pub mod prefill;
pub mod reducer;
pub mod remote_connect;
pub mod remote_probe;
pub mod transport_source;
pub mod watcher;

pub use actions::{peek, send_key, send_message, ActionError, KEYPAD};
pub use config::{Config, ConfigError, LocalConfig, RemoteConfig, RemoteOs, UiConfig, WslConfig};
pub use hub::{default_source_factory, specs_from_config, Hub, ServerSpec, SourceFactory};
pub use model::{
    AgentList, AgentRow, AgentStatus, ConnectionState, Counts, OfflineGroup, ServerChip,
    ServerKind, ServerState,
};
pub use paths::{Env, Platform};
pub use prefill::{gather_inputs, prefill_config, render_prefilled, PrefillInputs};
pub use remote_connect::{ProcessRunner, TokioRunner};
pub use watcher::WatcherConfig;
