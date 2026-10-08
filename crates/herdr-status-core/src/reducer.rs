//! Pure transitions of one server's state. The watcher calls these; nothing
//! here does I/O.

use std::time::SystemTime;

use herdr_client::{AgentInfo, PaneAgentStatusChanged, SessionSnapshot};

use crate::model::{AgentRow, ConnectionState, ServerState};

/// Row name: live agent name, else herdr's presentation title, else the
/// terminal title, else `"<kind> · <workspace>"`.
pub fn display_name(agent: &AgentInfo, workspace_label: &str) -> String {
    for candidate in [&agent.name, &agent.title, &agent.terminal_title_stripped] {
        if let Some(text) = candidate
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return text.to_string();
        }
    }
    format!(
        "{} · {}",
        agent.agent.as_deref().unwrap_or("agent"),
        workspace_label
    )
}

/// Rebuild rows from an authoritative snapshot. Rows whose status is unchanged
/// keep `since` and `flash_seq`; a changed status counts as observed now.
pub fn apply_snapshot(state: &mut ServerState, snapshot: &SessionSnapshot, now: SystemTime) {
    let mut rows = Vec::with_capacity(snapshot.agents.len());
    for agent in &snapshot.agents {
        let workspace_label = snapshot
            .workspace_label(&agent.workspace_id)
            .filter(|l| !l.is_empty())
            .unwrap_or(&agent.workspace_id)
            .to_string();
        let name = display_name(agent, &workspace_label);
        let previous = state.rows.iter().find(|r| r.pane_id == agent.pane_id);
        let (since, flash_seq) = match previous {
            Some(p) if p.status == agent.agent_status => (p.since, p.flash_seq),
            Some(p) => (Some(now), p.flash_seq + 1),
            None => (None, 0),
        };
        rows.push(AgentRow {
            server_id: state.id.clone(),
            server_label: state.label.clone(),
            pane_id: agent.pane_id.clone(),
            workspace_label,
            agent_kind: agent.agent.clone(),
            name,
            status: agent.agent_status,
            since,
            flash_seq,
        });
    }
    state.rows = rows;
}

/// Apply a per-pane status event. Returns `false` when the pane has no row yet;
/// the caller should then take a snapshot instead of inventing a row.
pub fn apply_status(
    state: &mut ServerState,
    event: &PaneAgentStatusChanged,
    now: SystemTime,
) -> bool {
    let Some(row) = state.rows.iter_mut().find(|r| r.pane_id == event.pane_id) else {
        return false;
    };
    if row.status != event.agent_status {
        row.status = event.agent_status;
        row.since = Some(now);
        row.flash_seq += 1;
    }
    true
}

pub fn remove_pane(state: &mut ServerState, pane_id: &str) {
    state.rows.retain(|r| r.pane_id != pane_id);
}

/// Rows are kept so the UI can show them dimmed.
pub fn set_offline(state: &mut ServerState, reason: String, retry_at: SystemTime) {
    state.connection = ConnectionState::Offline { reason, retry_at };
}

pub fn set_online(state: &mut ServerState, transport: &str) {
    state.connection = ConnectionState::Online;
    state.transport = transport.to_string();
}

pub fn set_connecting(state: &mut ServerState) {
    state.connection = ConnectionState::Connecting;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentStatus, ServerKind};
    use herdr_client::protocol::{parse_line, ResponseResult, ServerLine};
    use herdr_client::{AgentInfo, PaneAgentStatusChanged, SessionSnapshot};
    use std::time::Duration;

    const FIXTURE: &str = include_str!("../../herdr-client/tests/fixtures/snapshot.json");

    fn fixture() -> SessionSnapshot {
        let compact: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        match parse_line(&compact.to_string()).unwrap() {
            ServerLine::Response {
                result: ResponseResult::SessionSnapshot { snapshot },
                ..
            } => snapshot,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn status_event(pane_id: &str, status: AgentStatus) -> PaneAgentStatusChanged {
        PaneAgentStatusChanged {
            pane_id: pane_id.into(),
            workspace_id: "w2".into(),
            agent_status: status,
            agent: Some("claude".into()),
            title: None,
            display_agent: None,
        }
    }

    fn server() -> ServerState {
        ServerState::new("local", "local", ServerKind::Local)
    }

    #[test]
    fn first_snapshot_builds_rows_without_since_or_flash() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        assert_eq!(s.rows.len(), 2);
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.name, "Herdr agent status bar");
        assert_eq!(w2.workspace_label, "herdr-status");
        assert_eq!(w2.agent_kind.as_deref(), Some("claude"));
        assert_eq!(w2.status, AgentStatus::Working);
        assert_eq!(w2.since, None);
        assert_eq!(w2.flash_seq, 0);
        assert_eq!(w2.server_label, "local");
    }

    #[test]
    fn status_event_sets_since_and_bumps_flash_only_on_change() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        assert!(apply_status(
            &mut s,
            &status_event("w2:p1", AgentStatus::Idle),
            t(2)
        ));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.status, AgentStatus::Idle);
        assert_eq!(w2.since, Some(t(2)));
        assert_eq!(w2.flash_seq, 1);
        assert!(apply_status(
            &mut s,
            &status_event("w2:p1", AgentStatus::Idle),
            t(3)
        ));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.since, Some(t(2)), "unchanged status keeps since");
        assert_eq!(w2.flash_seq, 1);
    }

    #[test]
    fn status_event_for_unknown_pane_returns_false() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        assert!(!apply_status(
            &mut s,
            &status_event("w9:p9", AgentStatus::Blocked),
            t(2)
        ));
    }

    #[test]
    fn later_snapshot_keeps_since_for_same_status_and_resets_on_change() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        apply_status(&mut s, &status_event("w2:p1", AgentStatus::Idle), t(2));
        let mut snap = fixture();
        snap.agents
            .iter_mut()
            .find(|a| a.pane_id == "w2:p1")
            .unwrap()
            .agent_status = AgentStatus::Idle;
        apply_snapshot(&mut s, &snap, t(5));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.since, Some(t(2)));
        assert_eq!(w2.flash_seq, 1);
        snap.agents
            .iter_mut()
            .find(|a| a.pane_id == "w2:p1")
            .unwrap()
            .agent_status = AgentStatus::Blocked;
        apply_snapshot(&mut s, &snap, t(9));
        let w2 = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.status, AgentStatus::Blocked);
        assert_eq!(w2.since, Some(t(9)));
        assert_eq!(w2.flash_seq, 2);
    }

    #[test]
    fn snapshot_drops_rows_for_panes_that_are_gone_and_remove_pane_works() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        let mut snap = fixture();
        snap.agents.retain(|a| a.pane_id != "w1:p1");
        apply_snapshot(&mut s, &snap, t(2));
        assert_eq!(s.rows.len(), 1);
        remove_pane(&mut s, "w2:p1");
        assert!(s.rows.is_empty());
    }

    #[test]
    fn display_name_priority() {
        let base = AgentInfo {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            tab_id: String::new(),
            agent: Some("codex".into()),
            agent_status: AgentStatus::Idle,
            name: None,
            title: None,
            display_agent: None,
            terminal_title_stripped: None,
            terminal_title: None,
            focused: false,
            cwd: None,
        };
        assert_eq!(display_name(&base, "swat"), "codex · swat");
        let with_terminal = AgentInfo {
            terminal_title_stripped: Some(" PR review ".into()),
            ..base.clone()
        };
        assert_eq!(display_name(&with_terminal, "swat"), "PR review");
        let with_title = AgentInfo {
            title: Some("Reviewer pane".into()),
            ..with_terminal.clone()
        };
        assert_eq!(display_name(&with_title, "swat"), "Reviewer pane");
        let with_name = AgentInfo {
            name: Some("reviewer".into()),
            ..with_title
        };
        assert_eq!(display_name(&with_name, "swat"), "reviewer");
        let no_kind = AgentInfo {
            agent: None,
            ..base
        };
        assert_eq!(display_name(&no_kind, "swat"), "agent · swat");
    }

    #[test]
    fn offline_keeps_rows_and_online_restores_state() {
        let mut s = server();
        apply_snapshot(&mut s, &fixture(), t(1));
        set_offline(&mut s, "Unreachable".into(), t(30));
        assert_eq!(
            s.connection,
            ConnectionState::Offline {
                reason: "Unreachable".into(),
                retry_at: t(30)
            }
        );
        assert_eq!(s.rows.len(), 2);
        set_online(&mut s, "unix socket /x");
        assert!(s.is_online());
        assert_eq!(s.transport, "unix socket /x");
    }
}
