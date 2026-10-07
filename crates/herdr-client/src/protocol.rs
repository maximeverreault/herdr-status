//! Wire types for herdr's newline-delimited JSON API (protocol 22).
//!
//! Every line the server sends is one of: a success response (`id` + `result`),
//! an error response (`id` + `error`), or a pushed event (`event` + `data`).

use serde::{Deserialize, Serialize};

use crate::snapshot::{PaneInfo, ReadResult, SessionSnapshot};

/// Agent lifecycle state as herdr reports it. Declaration order is urgency
/// order: the most attention-worthy state sorts first.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Blocked,
    Working,
    Done,
    Idle,
    #[default]
    Unknown,
}

/// Body of an error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    #[serde(default)]
    pub message: String,
}

/// The `result` object of a success response, tagged by `type`.
/// Unknown types deserialise as [`ResponseResult::Other`]; extra fields are ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseResult {
    #[serde(rename = "pong")]
    Pong { version: String, protocol: u32 },
    #[serde(rename = "session_snapshot")]
    SessionSnapshot { snapshot: SessionSnapshot },
    #[serde(rename = "subscription_started")]
    SubscriptionStarted,
    #[serde(rename = "pane_read")]
    PaneRead { read: ReadResult },
    #[serde(rename = "agent_prompted")]
    AgentPrompted,
    #[serde(rename = "ok")]
    Ok,
    #[serde(other)]
    Other,
}

/// Payload of a per-pane `pane.agent_status_changed` subscription event.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneAgentStatusChanged {
    pub pane_id: String,
    pub workspace_id: String,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
}

/// A pushed event. Per-pane status events use the dotted name; lifecycle
/// events use snake_case names (herdr's raw envelope). Both are accepted.
/// Deserialization is hand-written so unknown events become [`Event::Other`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    PaneAgentStatusChanged(PaneAgentStatusChanged),
    PaneCreated {
        pane: PaneInfo,
    },
    PaneUpdated {
        pane: PaneInfo,
    },
    PaneClosed {
        pane_id: String,
        workspace_id: String,
    },
    PaneExited {
        pane_id: String,
        workspace_id: String,
    },
    PaneAgentDetected {
        pane_id: String,
        agent: Option<String>,
    },
    Other,
}

#[derive(Deserialize)]
struct RawEvent {
    event: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct PanePayload {
    pane: PaneInfo,
}

#[derive(Deserialize)]
struct PaneIdsPayload {
    pane_id: String,
    workspace_id: String,
}

#[derive(Deserialize)]
struct AgentDetectedPayload {
    pane_id: String,
    #[serde(default)]
    agent: Option<String>,
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = RawEvent::deserialize(deserializer)?;
        let data = raw.data;
        Ok(match raw.event.as_str() {
            "pane.agent_status_changed" | "pane_agent_status_changed" => {
                Event::PaneAgentStatusChanged(
                    serde_json::from_value(data).map_err(D::Error::custom)?,
                )
            }
            "pane_created" | "pane.created" => {
                let p: PanePayload = serde_json::from_value(data).map_err(D::Error::custom)?;
                Event::PaneCreated { pane: p.pane }
            }
            "pane_updated" | "pane.updated" => {
                let p: PanePayload = serde_json::from_value(data).map_err(D::Error::custom)?;
                Event::PaneUpdated { pane: p.pane }
            }
            "pane_closed" | "pane.closed" => {
                let p: PaneIdsPayload = serde_json::from_value(data).map_err(D::Error::custom)?;
                Event::PaneClosed {
                    pane_id: p.pane_id,
                    workspace_id: p.workspace_id,
                }
            }
            "pane_exited" | "pane.exited" => {
                let p: PaneIdsPayload = serde_json::from_value(data).map_err(D::Error::custom)?;
                Event::PaneExited {
                    pane_id: p.pane_id,
                    workspace_id: p.workspace_id,
                }
            }
            "pane_agent_detected" | "pane.agent_detected" => {
                let p: AgentDetectedPayload =
                    serde_json::from_value(data).map_err(D::Error::custom)?;
                Event::PaneAgentDetected {
                    pane_id: p.pane_id,
                    agent: p.agent,
                }
            }
            _ => Event::Other,
        })
    }
}

/// One decoded line from the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ServerLine {
    Response { id: String, result: ResponseResult },
    Error { id: String, error: ErrorBody },
    Event(Event),
}

/// Parse one line (without its trailing newline) from the server.
pub fn parse_line(line: &str) -> Result<ServerLine, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_status_orders_by_urgency() {
        assert!(AgentStatus::Blocked < AgentStatus::Working);
        assert!(AgentStatus::Working < AgentStatus::Done);
        assert!(AgentStatus::Done < AgentStatus::Idle);
        assert!(AgentStatus::Idle < AgentStatus::Unknown);
        assert_eq!(
            serde_json::from_str::<AgentStatus>("\"blocked\"").unwrap(),
            AgentStatus::Blocked
        );
    }

    #[test]
    fn parses_pong_response() {
        let line = r#"{"id":"hs-1","result":{"type":"pong","version":"0.9.3","protocol":22,"capabilities":{"health_check":true}}}"#;
        match parse_line(line).unwrap() {
            ServerLine::Response {
                id,
                result: ResponseResult::Pong { version, protocol },
            } => {
                assert_eq!(id, "hs-1");
                assert_eq!(version, "0.9.3");
                assert_eq!(protocol, 22);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_error_response() {
        let line = r#"{"id":"sub","error":{"code":"invalid_request","message":"invalid request: missing field `pane_id`"}}"#;
        match parse_line(line).unwrap() {
            ServerLine::Error { id, error } => {
                assert_eq!(id, "sub");
                assert_eq!(error.code, "invalid_request");
                assert!(error.message.contains("pane_id"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_unit_results_with_extra_fields() {
        let ok = r#"{"id":"k","result":{"type":"ok"}}"#;
        assert!(matches!(
            parse_line(ok).unwrap(),
            ServerLine::Response {
                result: ResponseResult::Ok,
                ..
            }
        ));
        let started = r#"{"id":"s","result":{"type":"subscription_started"}}"#;
        assert!(matches!(
            parse_line(started).unwrap(),
            ServerLine::Response {
                result: ResponseResult::SubscriptionStarted,
                ..
            }
        ));
        let prompted = r#"{"id":"p","result":{"type":"agent_prompted","agent":{"pane_id":"w1:p1","agent_status":"working"}}}"#;
        assert!(matches!(
            parse_line(prompted).unwrap(),
            ServerLine::Response {
                result: ResponseResult::AgentPrompted,
                ..
            }
        ));
    }

    #[test]
    fn unknown_result_type_is_other() {
        let line = r#"{"id":"x","result":{"type":"plugin_list","plugins":[]}}"#;
        assert!(matches!(
            parse_line(line).unwrap(),
            ServerLine::Response {
                result: ResponseResult::Other,
                ..
            }
        ));
    }

    #[test]
    fn parses_pane_read_result() {
        let line = r#"{"id":"r","result":{"type":"pane_read","read":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","source":"recent_unwrapped","format":"text","text":"line a\nline b","revision":7,"truncated":false}}}"#;
        match parse_line(line).unwrap() {
            ServerLine::Response {
                result: ResponseResult::PaneRead { read },
                ..
            } => {
                assert_eq!(read.text, "line a\nline b");
                assert!(!read.truncated);
                assert_eq!(read.revision, 7);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_per_pane_status_event() {
        // Captured live from herdr 0.9.3 (dotted name, no `type` inside data).
        let line = r#"{"data":{"agent":"claude","agent_status":"idle","pane_id":"w2:p1","workspace_id":"w2"},"event":"pane.agent_status_changed"}"#;
        match parse_line(line).unwrap() {
            ServerLine::Event(Event::PaneAgentStatusChanged(e)) => {
                assert_eq!(e.pane_id, "w2:p1");
                assert_eq!(e.workspace_id, "w2");
                assert_eq!(e.agent_status, AgentStatus::Idle);
                assert_eq!(e.agent.as_deref(), Some("claude"));
                assert_eq!(e.title, None);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_lifecycle_events_with_snake_case_names() {
        // herdr serialises lifecycle events as its raw envelope: snake_case `event`
        // and a repeated `type` inside `data`.
        let closed = r#"{"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}"#;
        assert!(
            matches!(parse_line(closed).unwrap(), ServerLine::Event(Event::PaneClosed { pane_id, .. }) if pane_id == "w1:p2")
        );

        let created = r#"{"event":"pane_created","data":{"type":"pane_created","pane":{"pane_id":"w1:p3","terminal_id":"t","workspace_id":"w1","tab_id":"w1:t2","focused":false,"agent_status":"unknown","revision":0}}}"#;
        assert!(
            matches!(parse_line(created).unwrap(), ServerLine::Event(Event::PaneCreated { pane }) if pane.pane_id == "w1:p3")
        );

        let detected = r#"{"event":"pane_agent_detected","data":{"type":"pane_agent_detected","pane_id":"w1:p3","workspace_id":"w1","agent":"codex","released":false}}"#;
        assert!(
            matches!(parse_line(detected).unwrap(), ServerLine::Event(Event::PaneAgentDetected { pane_id, agent }) if pane_id == "w1:p3" && agent.as_deref() == Some("codex"))
        );

        let exited = r#"{"event":"pane_exited","data":{"type":"pane_exited","pane_id":"w1:p3","workspace_id":"w1"}}"#;
        assert!(matches!(
            parse_line(exited).unwrap(),
            ServerLine::Event(Event::PaneExited { .. })
        ));
    }

    #[test]
    fn unknown_event_is_other() {
        let line = r#"{"event":"layout_updated","data":{"type":"layout_updated"}}"#;
        assert!(matches!(
            parse_line(line).unwrap(),
            ServerLine::Event(Event::Other)
        ));
    }

    #[test]
    fn rejects_garbage_and_unrecognised_shapes() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line(r#"{"hello":"world"}"#).is_err());
        assert!(parse_line("").is_err());
    }
}
