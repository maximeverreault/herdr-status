//! Record types returned by `session.snapshot` and `agent.read`.
//! Every optional field defaults so herdr additions and omissions never break parsing.

use serde::Deserialize;

use crate::protocol::AgentStatus;

/// One agent-bearing pane, as listed in `snapshot.agents` and `agent.list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentInfo {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    /// Agent kind label such as `claude` or `codex`.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    /// Live agent name set with `herdr agent start <name>` or `rename`.
    #[serde(default)]
    pub name: Option<String>,
    /// herdr's presentation title for the pane.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Any pane (with or without an agent), as listed in `snapshot.panes` and
/// carried by `pane_created` / `pane_updated` events.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
}

/// A herdr workspace: an id and its display label.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
}

/// The `snapshot` object of a `session_snapshot` result.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct SessionSnapshot {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub protocol: u32,
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
}

impl SessionSnapshot {
    /// Label of a workspace, if it is in the snapshot.
    pub fn workspace_label(&self, workspace_id: &str) -> Option<&str> {
        self.workspaces
            .iter()
            .find(|w| w.workspace_id == workspace_id)
            .map(|w| w.label.as_str())
    }

    /// Every pane id in the snapshot, in snapshot order. Used to build the
    /// per-pane subscription set.
    pub fn pane_ids(&self) -> Vec<&str> {
        self.panes.iter().map(|p| p.pane_id.as_str()).collect()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{parse_line, AgentStatus, ResponseResult, ServerLine};

    const FIXTURE: &str = include_str!("../tests/fixtures/snapshot.json");

    fn fixture_snapshot() -> SessionSnapshot {
        let compact: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        match parse_line(&compact.to_string()).unwrap() {
            ServerLine::Response {
                result: ResponseResult::SessionSnapshot { snapshot },
                ..
            } => snapshot,
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_real_snapshot_fixture() {
        let snapshot = fixture_snapshot();
        assert_eq!(snapshot.protocol, 22);
        assert_eq!(snapshot.version, "0.9.3");
        assert_eq!(snapshot.agents.len(), 2);
        assert_eq!(snapshot.panes.len(), 3);
        let w2 = snapshot
            .agents
            .iter()
            .find(|a| a.pane_id == "w2:p1")
            .unwrap();
        assert_eq!(w2.workspace_id, "w2");
        assert_eq!(w2.tab_id, "w2:t1");
        assert_eq!(w2.agent.as_deref(), Some("claude"));
        assert_eq!(w2.agent_status, AgentStatus::Working);
        assert_eq!(
            w2.terminal_title_stripped.as_deref(),
            Some("Herdr agent status bar")
        );
        assert_eq!(w2.name, None);
        assert!(w2.focused);
        assert_eq!(snapshot.workspace_label("w2"), Some("herdr-status"));
        assert_eq!(snapshot.workspace_label("w1"), Some("swat-db-tools"));
        assert_eq!(snapshot.workspace_label("nope"), None);
        assert_eq!(snapshot.pane_ids(), vec!["w1:p1", "w1:p2", "w2:p1"]);
    }

    #[test]
    fn tolerates_missing_optional_fields_and_unknown_keys() {
        let json = r#"{"version":"9.9.9","protocol":99,
            "agents":[{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"blocked","future_field":1}],
            "panes":[{"pane_id":"w1:p1","workspace_id":"w1"}],
            "workspaces":[{"workspace_id":"w1"}],"extra":true}"#;
        let s: SessionSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(s.agents[0].agent_status, AgentStatus::Blocked);
        assert_eq!(s.agents[0].tab_id, "");
        assert!(s.agents[0].agent.is_none());
        assert_eq!(s.panes[0].agent_status, AgentStatus::Unknown);
        assert_eq!(s.workspaces[0].label, "");
    }
}
