//! What the UI renders. Pure data; serialised as the Tauri `agents` event.

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, Serializer};

pub use herdr_client::AgentStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    Local,
    Wsl,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ConnectionState {
    Connecting,
    Online,
    Offline {
        reason: String,
        #[serde(serialize_with = "ser_millis")]
        retry_at: SystemTime,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentRow {
    pub server_id: String,
    pub server_label: String,
    pub pane_id: String,
    pub workspace_label: String,
    pub agent_kind: Option<String>,
    pub name: String,
    pub status: AgentStatus,
    /// When the bar observed the current status; `None` until the first change.
    #[serde(serialize_with = "ser_opt_millis")]
    pub since: Option<SystemTime>,
    /// Bumped on every observed status change so the UI can replay the flash.
    pub flash_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerState {
    pub id: String,
    pub label: String,
    pub kind: ServerKind,
    pub connection: ConnectionState,
    pub rows: Vec<AgentRow>,
    /// `Transport::describe()`, for tooltips.
    pub transport: String,
}

impl ServerState {
    pub fn new(id: &str, label: &str, kind: ServerKind) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            kind,
            connection: ConnectionState::Connecting,
            rows: Vec::new(),
            transport: String::new(),
        }
    }

    pub fn is_online(&self) -> bool {
        self.connection == ConnectionState::Online
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerChip {
    pub id: String,
    pub label: String,
    pub kind: ServerKind,
    pub connection: ConnectionState,
    pub transport: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineGroup {
    pub server_id: String,
    pub label: String,
    pub reason: String,
    #[serde(serialize_with = "ser_millis")]
    pub retry_at: SystemTime,
    pub rows: Vec<AgentRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub blocked: usize,
    pub working: usize,
    pub done: usize,
    pub idle: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentList {
    pub servers: Vec<ServerChip>,
    /// Online servers only, urgency-sorted.
    pub rows: Vec<AgentRow>,
    pub offline: Vec<OfflineGroup>,
    pub counts: Counts,
}

/// Urgency rank, then newest change first (`None` last), then server, then name.
pub fn sort_rows(rows: &mut [AgentRow]) {
    rows.sort_by(|a, b| {
        a.status
            .cmp(&b.status)
            .then_with(|| cmp_since_desc(a.since, b.since))
            .then_with(|| a.server_label.cmp(&b.server_label))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn cmp_since_desc(a: Option<SystemTime>, b: Option<SystemTime>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub fn assemble(servers: &[ServerState]) -> AgentList {
    let mut list = AgentList::default();
    for server in servers {
        list.servers.push(ServerChip {
            id: server.id.clone(),
            label: server.label.clone(),
            kind: server.kind,
            connection: server.connection.clone(),
            transport: server.transport.clone(),
        });
        match &server.connection {
            ConnectionState::Online => list.rows.extend(server.rows.iter().cloned()),
            ConnectionState::Offline { reason, retry_at } => list.offline.push(OfflineGroup {
                server_id: server.id.clone(),
                label: server.label.clone(),
                reason: reason.clone(),
                retry_at: *retry_at,
                rows: server.rows.clone(),
            }),
            ConnectionState::Connecting => {}
        }
    }
    sort_rows(&mut list.rows);
    for row in &list.rows {
        match row.status {
            AgentStatus::Blocked => list.counts.blocked += 1,
            AgentStatus::Working => list.counts.working += 1,
            AgentStatus::Done => list.counts.done += 1,
            AgentStatus::Idle => list.counts.idle += 1,
            AgentStatus::Unknown => list.counts.unknown += 1,
        }
    }
    list
}

fn millis(t: &SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ser_millis<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(millis(t))
}

fn ser_opt_millis<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
    match t {
        Some(t) => s.serialize_some(&millis(t)),
        None => s.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn row(server: &str, name: &str, status: AgentStatus, since: Option<SystemTime>) -> AgentRow {
        AgentRow {
            server_id: server.into(),
            server_label: server.into(),
            pane_id: format!("{server}:{name}"),
            workspace_label: "ws".into(),
            agent_kind: Some("claude".into()),
            name: name.into(),
            status,
            since,
            flash_seq: 0,
        }
    }

    #[test]
    fn sorts_by_urgency_then_newest_change_then_server_then_name() {
        let mut rows = vec![
            row("local", "idle-old", AgentStatus::Idle, Some(t(10))),
            row("local", "working-new", AgentStatus::Working, Some(t(50))),
            row("winbox", "blocked", AgentStatus::Blocked, None),
            row("local", "working-old", AgentStatus::Working, Some(t(20))),
            row("local", "working-unknown-since", AgentStatus::Working, None),
            row("local", "done", AgentStatus::Done, Some(t(30))),
            row("box", "zeta", AgentStatus::Unknown, None),
            row("box", "alpha", AgentStatus::Unknown, None),
        ];
        sort_rows(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "blocked",
                "working-new",
                "working-old",
                "working-unknown-since",
                "done",
                "idle-old",
                "alpha",
                "zeta"
            ]
        );
    }

    #[test]
    fn assemble_splits_online_rows_from_offline_groups_and_counts_online_only() {
        let mut local = ServerState::new("local", "local", ServerKind::Local);
        local.connection = ConnectionState::Online;
        local.rows = vec![
            row("local", "a", AgentStatus::Working, None),
            row("local", "b", AgentStatus::Blocked, None),
        ];
        let mut remote = ServerState::new("winbox", "winbox", ServerKind::Remote);
        remote.connection = ConnectionState::Offline {
            reason: "Unreachable".into(),
            retry_at: t(100),
        };
        remote.rows = vec![row("winbox", "c", AgentStatus::Done, None)];
        let connecting = ServerState::new("wsl", "wsl", ServerKind::Wsl);

        let list = assemble(&[local, remote, connecting]);
        assert_eq!(list.servers.len(), 3);
        assert_eq!(
            list.servers[1].connection,
            ConnectionState::Offline {
                reason: "Unreachable".into(),
                retry_at: t(100)
            }
        );
        assert_eq!(
            list.rows
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert_eq!(list.offline.len(), 1);
        assert_eq!(list.offline[0].label, "winbox");
        assert_eq!(list.offline[0].reason, "Unreachable");
        assert_eq!(list.offline[0].rows[0].name, "c");
        assert_eq!(
            list.counts,
            Counts {
                blocked: 1,
                working: 1,
                done: 0,
                idle: 0,
                unknown: 0
            }
        );
    }

    #[test]
    fn serialises_times_as_unix_millis_and_statuses_lowercase() {
        let r = row("local", "a", AgentStatus::Blocked, Some(t(5)));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["since"], 5000);
        assert_eq!(v["status"], "blocked");
        let none = row("local", "a", AgentStatus::Idle, None);
        assert_eq!(
            serde_json::to_value(&none).unwrap()["since"],
            serde_json::Value::Null
        );
        let offline = ConnectionState::Offline {
            reason: "x".into(),
            retry_at: t(7),
        };
        let v = serde_json::to_value(&offline).unwrap();
        assert_eq!(v["state"], "offline");
        assert_eq!(v["retry_at"], 7000);
        assert_eq!(
            serde_json::to_value(ConnectionState::Online).unwrap()["state"],
            "online"
        );
    }
}
