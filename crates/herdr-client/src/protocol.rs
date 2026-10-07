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
    // Do not reorder: the urgency sort depends on declaration order.
    Blocked,
    Working,
    Done,
    Idle,
    #[default]
    #[serde(other)]
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
#[derive(Debug, Clone, PartialEq)]
pub enum ServerLine {
    Response { id: String, result: ResponseResult },
    Error { id: String, error: ErrorBody },
    Event(Event),
}

impl<'de> Deserialize<'de> for ServerLine {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut map = serde_json::Map::deserialize(deserializer)?;
        if map.contains_key("event") {
            let event =
                serde_json::from_value(serde_json::Value::Object(map)).map_err(D::Error::custom)?;
            return Ok(ServerLine::Event(event));
        }
        let id =
            |map: &mut serde_json::Map<String, serde_json::Value>| -> Result<String, D::Error> {
                let value = map
                    .remove("id")
                    .ok_or_else(|| D::Error::missing_field("id"))?;
                serde_json::from_value(value).map_err(D::Error::custom)
            };
        if let Some(error) = map.remove("error") {
            return Ok(ServerLine::Error {
                id: id(&mut map)?,
                error: serde_json::from_value(error).map_err(D::Error::custom)?,
            });
        }
        if let Some(result) = map.remove("result") {
            return Ok(ServerLine::Response {
                id: id(&mut map)?,
                result: serde_json::from_value(result).map_err(D::Error::custom)?,
            });
        }
        Err(D::Error::custom("line has none of result, error or event"))
    }
}

/// Parse one line (without its trailing newline) from the server.
pub fn parse_line(line: &str) -> Result<ServerLine, serde_json::Error> {
    serde_json::from_str(line)
}

/// A subscription entry for `events.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subscription {
    r#type: std::borrow::Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_id: Option<String>,
}

impl Subscription {
    /// A parameterless lifecycle subscription such as `pane.created`.
    pub fn lifecycle(kind: &str) -> Self {
        Subscription {
            r#type: kind.to_string().into(),
            pane_id: None,
        }
    }

    /// A per-pane status subscription. The pane must exist when the request is sent.
    pub fn pane_agent_status(pane_id: &str) -> Self {
        Subscription {
            r#type: "pane.agent_status_changed".into(),
            pane_id: Some(pane_id.to_string()),
        }
    }
}

/// API methods this client can send.
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    Ping,
    SessionSnapshot,
    EventsSubscribe(Vec<Subscription>),
    AgentRead { target: String, lines: u32 },
    AgentSendKeys { target: String, keys: Vec<String> },
    AgentPrompt { target: String, text: String },
}

impl Method {
    /// The `method` string sent on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Method::Ping => "ping",
            Method::SessionSnapshot => "session.snapshot",
            Method::EventsSubscribe(_) => "events.subscribe",
            Method::AgentRead { .. } => "agent.read",
            Method::AgentSendKeys { .. } => "agent.send_keys",
            Method::AgentPrompt { .. } => "agent.prompt",
        }
    }

    fn params(&self) -> serde_json::Value {
        match self {
            Method::Ping | Method::SessionSnapshot => serde_json::json!({}),
            Method::EventsSubscribe(subs) => serde_json::json!({ "subscriptions": subs }),
            Method::AgentRead { target, lines } => serde_json::json!({
                "target": target,
                "source": "recent_unwrapped",
                "lines": lines,
                "format": "text",
                "strip_ansi": true,
            }),
            Method::AgentSendKeys { target, keys } => {
                serde_json::json!({ "target": target, "keys": keys })
            }
            Method::AgentPrompt { target, text } => {
                serde_json::json!({ "target": target, "text": text, "wait": null })
            }
        }
    }
}

/// Encode a request as one JSON line, newline included.
pub fn encode_request(id: &str, method: &Method) -> String {
    let value = serde_json::json!({ "id": id, "method": method.name(), "params": method.params() });
    // Serialising a `serde_json::Value` cannot fail.
    let mut line = serde_json::to_string(&value).expect("request is valid JSON");
    line.push('\n');
    line
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

    fn encoded(method: &Method) -> serde_json::Value {
        serde_json::from_str(&encode_request("hs-7", method)).unwrap()
    }

    #[test]
    fn encodes_ping_and_snapshot() {
        assert_eq!(
            encoded(&Method::Ping),
            serde_json::json!({"id":"hs-7","method":"ping","params":{}})
        );
        assert_eq!(
            encoded(&Method::SessionSnapshot),
            serde_json::json!({"id":"hs-7","method":"session.snapshot","params":{}})
        );
    }

    #[test]
    fn encodes_subscribe_with_lifecycle_and_per_pane_entries() {
        let method = Method::EventsSubscribe(vec![
            Subscription::lifecycle("pane.created"),
            Subscription::pane_agent_status("w1:p1"),
        ]);
        assert_eq!(
            encoded(&method),
            serde_json::json!({"id":"hs-7","method":"events.subscribe","params":{"subscriptions":[
                {"type":"pane.created"},
                {"type":"pane.agent_status_changed","pane_id":"w1:p1"}
            ]}})
        );
    }

    #[test]
    fn encodes_agent_actions() {
        let read = Method::AgentRead {
            target: "w1:p1".into(),
            lines: 30,
        };
        assert_eq!(
            encoded(&read),
            serde_json::json!({"id":"hs-7","method":"agent.read","params":{"target":"w1:p1","source":"recent_unwrapped","lines":30,"format":"text","strip_ansi":true}})
        );
        let keys = Method::AgentSendKeys {
            target: "w1:p1".into(),
            keys: vec!["ctrl+c".into()],
        };
        assert_eq!(
            encoded(&keys),
            serde_json::json!({"id":"hs-7","method":"agent.send_keys","params":{"target":"w1:p1","keys":["ctrl+c"]}})
        );
        let prompt = Method::AgentPrompt {
            target: "w1:p1".into(),
            text: "continue".into(),
        };
        assert_eq!(
            encoded(&prompt),
            serde_json::json!({"id":"hs-7","method":"agent.prompt","params":{"target":"w1:p1","text":"continue","wait":null}})
        );
    }

    #[test]
    fn encoded_request_is_one_line_with_newline() {
        let s = encode_request("a", &Method::Ping);
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
    }

    #[test]
    fn unknown_status_string_is_unknown() {
        assert_eq!(
            serde_json::from_str::<AgentStatus>("\"waiting\"").unwrap(),
            AgentStatus::Unknown
        );
        let json = r#"{"version":"1","protocol":22,"agents":[{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"waiting"}]}"#;
        let s: crate::snapshot::SessionSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(s.agents[0].agent_status, AgentStatus::Unknown);
    }

    #[test]
    fn malformed_known_event_payload_is_an_error() {
        let line = r#"{"event":"pane_closed","data":{"type":"pane_closed"}}"#;
        let err = parse_line(line).unwrap_err();
        assert!(err.to_string().contains("pane_id"), "{err}");
    }

    #[test]
    fn known_event_with_extra_keys_parses() {
        let line = r#"{"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1","future":1}}"#;
        assert!(matches!(
            parse_line(line).unwrap(),
            ServerLine::Event(Event::PaneClosed { .. })
        ));
    }
}
