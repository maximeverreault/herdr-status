//! Snapshot record types (`session.snapshot`, `agent.read`).
use serde::Deserialize;

/// One pane as reported by herdr. Fields the bar does not use are ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
}

/// Result of `agent.read` / `pane.read`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReadResult {
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub revision: u64,
}

/// Placeholder until Task 4; keeps `ResponseResult::SessionSnapshot` compiling.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionSnapshot {}
