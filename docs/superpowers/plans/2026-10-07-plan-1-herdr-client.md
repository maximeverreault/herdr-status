# herdr-client Protocol Crate Implementation Plan (Plan 1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust crate that speaks herdr's newline-JSON socket API over a Unix socket, a Windows named pipe, or a child-process bridge, with a fake herdr server for tests.

**Architecture:** `herdr-client` has three layers: `protocol` (serde types and line parsing), `transport` (a `Transport` trait producing one-shot `Connection`s, with Unix socket, named pipe and child-process bridge implementations), and `client` (a `Client` that runs one request per connection and holds an `EventStream` for subscriptions). A `FakeHerdrServer` behind the `test-support` feature lets every layer be tested without herdr. Later plans (core, Tauri app) depend only on this crate's public types.

**Tech Stack:** Rust 2021 (stable 1.99), tokio 1 (`net`, `process`, `io-util`, `time`), serde + serde_json, thiserror, async-trait, tracing. `windows-sys` on Windows for `ERROR_PIPE_BUSY`.

**Spec:** `docs/superpowers/specs/2026-10-07-herdr-status-bar-design.md`, sections 4 and 6. Every wire-format fact below was verified against herdr 0.9.3 (protocol 22).

**Conventions for every task:** run commands from the repo root `/Users/maximeverreault/AGCO/herdr-status`. Unit tests live in a `#[cfg(test)] mod tests` at the bottom of the file they test; integration tests live in `crates/herdr-client/tests/`. Commit after each task with the message given. Never commit `target/`.

---

## File structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | Workspace root: members, shared dependency versions |
| `crates/herdr-client/Cargo.toml` | Crate manifest, `test-support` feature |
| `crates/herdr-client/src/lib.rs` | Module list and re-exports |
| `crates/herdr-client/src/protocol.rs` | `AgentStatus`, `ErrorBody`, `ResponseResult`, `Event`, `ServerLine`, `parse_line`, `Method`, `Subscription`, request encoding |
| `crates/herdr-client/src/snapshot.rs` | `SessionSnapshot`, `AgentInfo`, `PaneInfo`, `WorkspaceInfo`, `ReadResult` |
| `crates/herdr-client/src/transport.rs` | `Transport`, `Connection`, `TransportError`, `StreamConnection` (generic over AsyncRead/AsyncWrite) |
| `crates/herdr-client/src/transport/unix.rs` | `UnixSocketTransport` (cfg unix) |
| `crates/herdr-client/src/transport/named_pipe.rs` | `NamedPipeTransport` (cfg windows) |
| `crates/herdr-client/src/transport/bridge.rs` | `ChildCommand`, `BridgeTransport` (spawn child, skip to ready marker) |
| `crates/herdr-client/src/client.rs` | `Client`, `EventStream`, `ClientError` |
| `crates/herdr-client/src/test_support.rs` | `FakeHerdrServer` (feature `test-support`, cfg unix) |
| `crates/herdr-client/tests/fixtures/snapshot.json` | A real `session.snapshot` response captured from herdr 0.9.3 |
| `crates/herdr-client/tests/client_fake_server.rs` | Client request/subscribe tests against the fake |
| `crates/herdr-client/tests/bridge_transport.rs` | Bridge tests using `sh -c` child scripts |

---

### Task 1: Workspace and crate scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `crates/herdr-client/Cargo.toml`
- Create: `crates/herdr-client/src/lib.rs`
- Create: `rust-toolchain.toml`

- [ ] **Step 1: Create the workspace manifest**

`Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/herdr-client"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"

[workspace.dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "net", "io-util", "process", "time", "macros", "sync"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
async-trait = "0.1"
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
```

- [ ] **Step 2: Create the crate manifest**

`crates/herdr-client/Cargo.toml`:

```toml
[package]
name = "herdr-client"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Client for the herdr terminal multiplexer socket API"

[features]
test-support = []

[dependencies]
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
async-trait.workspace = true

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_Foundation"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Create an empty library**

`crates/herdr-client/src/lib.rs`:

```rust
//! Client for the herdr socket API: newline-delimited JSON over a Unix socket,
//! a Windows named pipe, or a child-process bridge (`herdr remote-api-bridge`).
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build --workspace`
Expected: `Finished` with no warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml rust-toolchain.toml crates/herdr-client/Cargo.toml crates/herdr-client/src/lib.rs
git commit -m "chore: scaffold cargo workspace with herdr-client crate"
```

---

### Task 2: Protocol types: status, errors, responses, events, line parsing

**Files:**
- Create: `crates/herdr-client/src/protocol.rs`
- Modify: `crates/herdr-client/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-client/src/protocol.rs` (tests only for now; the types come in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_status_orders_by_urgency() {
        assert!(AgentStatus::Blocked < AgentStatus::Working);
        assert!(AgentStatus::Working < AgentStatus::Done);
        assert!(AgentStatus::Done < AgentStatus::Idle);
        assert!(AgentStatus::Idle < AgentStatus::Unknown);
        assert_eq!(serde_json::from_str::<AgentStatus>("\"blocked\"").unwrap(), AgentStatus::Blocked);
    }

    #[test]
    fn parses_pong_response() {
        let line = r#"{"id":"hs-1","result":{"type":"pong","version":"0.9.3","protocol":22,"capabilities":{"health_check":true}}}"#;
        match parse_line(line).unwrap() {
            ServerLine::Response { id, result: ResponseResult::Pong { version, protocol } } => {
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
        assert!(matches!(parse_line(ok).unwrap(), ServerLine::Response { result: ResponseResult::Ok, .. }));
        let started = r#"{"id":"s","result":{"type":"subscription_started"}}"#;
        assert!(matches!(parse_line(started).unwrap(), ServerLine::Response { result: ResponseResult::SubscriptionStarted, .. }));
        let prompted = r#"{"id":"p","result":{"type":"agent_prompted","agent":{"pane_id":"w1:p1","agent_status":"working"}}}"#;
        assert!(matches!(parse_line(prompted).unwrap(), ServerLine::Response { result: ResponseResult::AgentPrompted, .. }));
    }

    #[test]
    fn unknown_result_type_is_other() {
        let line = r#"{"id":"x","result":{"type":"plugin_list","plugins":[]}}"#;
        assert!(matches!(parse_line(line).unwrap(), ServerLine::Response { result: ResponseResult::Other, .. }));
    }

    #[test]
    fn parses_pane_read_result() {
        let line = r#"{"id":"r","result":{"type":"pane_read","read":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","source":"recent_unwrapped","format":"text","text":"line a\nline b","revision":7,"truncated":false}}}"#;
        match parse_line(line).unwrap() {
            ServerLine::Response { result: ResponseResult::PaneRead { read }, .. } => {
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
        assert!(matches!(parse_line(closed).unwrap(), ServerLine::Event(Event::PaneClosed { pane_id, .. }) if pane_id == "w1:p2"));

        let created = r#"{"event":"pane_created","data":{"type":"pane_created","pane":{"pane_id":"w1:p3","terminal_id":"t","workspace_id":"w1","tab_id":"w1:t2","focused":false,"agent_status":"unknown","revision":0}}}"#;
        assert!(matches!(parse_line(created).unwrap(), ServerLine::Event(Event::PaneCreated { pane }) if pane.pane_id == "w1:p3"));

        let detected = r#"{"event":"pane_agent_detected","data":{"type":"pane_agent_detected","pane_id":"w1:p3","workspace_id":"w1","agent":"codex","released":false}}"#;
        assert!(matches!(parse_line(detected).unwrap(), ServerLine::Event(Event::PaneAgentDetected { pane_id, agent }) if pane_id == "w1:p3" && agent.as_deref() == Some("codex")));

        let exited = r#"{"event":"pane_exited","data":{"type":"pane_exited","pane_id":"w1:p3","workspace_id":"w1"}}"#;
        assert!(matches!(parse_line(exited).unwrap(), ServerLine::Event(Event::PaneExited { .. })));
    }

    #[test]
    fn unknown_event_is_other() {
        let line = r#"{"event":"layout_updated","data":{"type":"layout_updated"}}"#;
        assert!(matches!(parse_line(line).unwrap(), ServerLine::Event(Event::Other)));
    }

    #[test]
    fn rejects_garbage_and_unrecognised_shapes() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line(r#"{"hello":"world"}"#).is_err());
        assert!(parse_line("").is_err());
    }
}
```

Add to `crates/herdr-client/src/lib.rs`:

```rust
pub mod protocol;
pub mod snapshot;
```

And create a stub `crates/herdr-client/src/snapshot.rs` so the crate compiles (the real types arrive in Task 4):

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-client protocol`
Expected: compile error, `cannot find type AgentStatus`, `parse_line` not found.

- [ ] **Step 3: Write the protocol types**

Put this above the `#[cfg(test)]` module in `crates/herdr-client/src/protocol.rs`:

```rust
//! Wire types for herdr's newline-delimited JSON API (protocol 22).
//!
//! Every line the server sends is one of: a success response (`id` + `result`),
//! an error response (`id` + `error`), or a pushed event (`event` + `data`).

use serde::{Deserialize, Serialize};

use crate::snapshot::{PaneInfo, ReadResult, SessionSnapshot};

/// Agent lifecycle state as herdr reports it. Declaration order is urgency
/// order: the most attention-worthy state sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Blocked,
    Working,
    Done,
    Idle,
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum Event {
    #[serde(rename = "pane.agent_status_changed", alias = "pane_agent_status_changed")]
    PaneAgentStatusChanged(PaneAgentStatusChanged),
    #[serde(rename = "pane_created", alias = "pane.created")]
    PaneCreated { pane: PaneInfo },
    #[serde(rename = "pane_updated", alias = "pane.updated")]
    PaneUpdated { pane: PaneInfo },
    #[serde(rename = "pane_closed", alias = "pane.closed")]
    PaneClosed { pane_id: String, workspace_id: String },
    #[serde(rename = "pane_exited", alias = "pane.exited")]
    PaneExited { pane_id: String, workspace_id: String },
    #[serde(rename = "pane_agent_detected", alias = "pane.agent_detected")]
    PaneAgentDetected {
        pane_id: String,
        #[serde(default)]
        agent: Option<String>,
    },
    #[serde(other)]
    Other,
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client protocol`
Expected: `test result: ok. 10 passed`.

If `parses_unit_results_with_extra_fields` fails on `agent_prompted`, change the variant to `AgentPrompted {}` (an empty struct variant also ignores unknown fields) and re-run.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/protocol.rs crates/herdr-client/src/snapshot.rs crates/herdr-client/src/lib.rs
git commit -m "feat(client): protocol response, error and event parsing"
```

---

### Task 3: Request encoding: `Method` and `Subscription`

**Files:**
- Modify: `crates/herdr-client/src/protocol.rs`

- [ ] **Step 1: Write the failing tests**

Add inside the existing `mod tests` in `protocol.rs`:

```rust
    fn encoded(method: &Method) -> serde_json::Value {
        serde_json::from_str(&encode_request("hs-7", method)).unwrap()
    }

    #[test]
    fn encodes_ping_and_snapshot() {
        assert_eq!(encoded(&Method::Ping), serde_json::json!({"id":"hs-7","method":"ping","params":{}}));
        assert_eq!(encoded(&Method::SessionSnapshot), serde_json::json!({"id":"hs-7","method":"session.snapshot","params":{}}));
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
        let read = Method::AgentRead { target: "w1:p1".into(), lines: 30 };
        assert_eq!(
            encoded(&read),
            serde_json::json!({"id":"hs-7","method":"agent.read","params":{"target":"w1:p1","source":"recent_unwrapped","lines":30,"format":"text","strip_ansi":true}})
        );
        let keys = Method::AgentSendKeys { target: "w1:p1".into(), keys: vec!["ctrl+c".into()] };
        assert_eq!(
            encoded(&keys),
            serde_json::json!({"id":"hs-7","method":"agent.send_keys","params":{"target":"w1:p1","keys":["ctrl+c"]}})
        );
        let prompt = Method::AgentPrompt { target: "w1:p1".into(), text: "continue".into() };
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-client protocol`
Expected: compile error, `Method` and `encode_request` not found.

- [ ] **Step 3: Implement `Method`, `Subscription`, `encode_request`**

Add to `protocol.rs` above the tests:

```rust
/// A subscription entry for `events.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Subscription {
    Lifecycle { r#type: String },
    PaneAgentStatus { r#type: &'static str, pane_id: String },
}

impl Subscription {
    /// A parameterless lifecycle subscription such as `pane.created`.
    pub fn lifecycle(kind: &str) -> Self {
        Subscription::Lifecycle { r#type: kind.to_string() }
    }

    /// A per-pane status subscription. The pane must exist when the request is sent.
    pub fn pane_agent_status(pane_id: &str) -> Self {
        Subscription::PaneAgentStatus { r#type: "pane.agent_status_changed", pane_id: pane_id.to_string() }
    }
}

/// Every API method the bar uses.
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
            Method::AgentSendKeys { target, keys } => serde_json::json!({ "target": target, "keys": keys }),
            Method::AgentPrompt { target, text } => serde_json::json!({ "target": target, "text": text, "wait": null }),
        }
    }
}

/// Encode a request as one JSON line, newline included.
pub fn encode_request(id: &str, method: &Method) -> String {
    let value = serde_json::json!({ "id": id, "method": method.name(), "params": method.params() });
    let mut line = serde_json::to_string(&value).expect("request is valid JSON");
    line.push('\n');
    line
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client protocol`
Expected: `test result: ok. 14 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/protocol.rs
git commit -m "feat(client): request encoding for ping, snapshot, subscribe and agent actions"
```

---

### Task 4: Snapshot record types

**Files:**
- Modify: `crates/herdr-client/src/snapshot.rs` (replace the Task 2 stub)
- Already present: `crates/herdr-client/tests/fixtures/snapshot.json` (a real `session.snapshot` response from herdr 0.9.3: 2 agents, 3 panes, 2 workspaces `w1` = `swat-db-tools`, `w2` = `herdr-status`; pane `w2:p1` is a `claude` agent in status `working` with title `Herdr agent status bar`)

- [ ] **Step 1: Write the failing tests**

Replace the whole of `crates/herdr-client/src/snapshot.rs` with the tests first (types follow in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{parse_line, AgentStatus, ResponseResult, ServerLine};

    const FIXTURE: &str = include_str!("../tests/fixtures/snapshot.json");

    fn fixture_snapshot() -> SessionSnapshot {
        let compact: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        match parse_line(&compact.to_string()).unwrap() {
            ServerLine::Response { result: ResponseResult::SessionSnapshot { snapshot }, .. } => snapshot,
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
        let w2 = snapshot.agents.iter().find(|a| a.pane_id == "w2:p1").unwrap();
        assert_eq!(w2.workspace_id, "w2");
        assert_eq!(w2.tab_id, "w2:t1");
        assert_eq!(w2.agent.as_deref(), Some("claude"));
        assert_eq!(w2.agent_status, AgentStatus::Working);
        assert_eq!(w2.terminal_title_stripped.as_deref(), Some("Herdr agent status bar"));
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-client snapshot`
Expected: compile errors (`AgentInfo`, `workspace_label`, `pane_ids` missing).

- [ ] **Step 3: Write the snapshot types**

Put this at the top of `crates/herdr-client/src/snapshot.rs`, above the tests:

```rust
//! Record types returned by `session.snapshot` and `agent.read`.
//! Every optional field defaults so herdr additions and omissions never break parsing.

use serde::Deserialize;

use crate::protocol::AgentStatus;

impl Default for AgentStatus {
    fn default() -> Self {
        AgentStatus::Unknown
    }
}

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
```

- [ ] **Step 4: Run all crate tests to verify they pass**

Run: `cargo test -p herdr-client`
Expected: all tests pass (14 protocol + 2 snapshot).

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/snapshot.rs crates/herdr-client/tests/fixtures/snapshot.json
git commit -m "feat(client): session snapshot record types with real fixture"
```

---

### Task 5: Transport trait, generic stream connection, Unix socket transport

**Files:**
- Create: `crates/herdr-client/src/transport.rs`
- Create: `crates/herdr-client/src/transport/unix.rs`
- Modify: `crates/herdr-client/src/lib.rs`

- [ ] **Step 1: Write the failing test**

`crates/herdr-client/src/transport/unix.rs` (test first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Transport, TransportError};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    /// Accepts one connection, echoes one line back prefixed with `echo:`, then closes.
    async fn echo_once(listener: UnixListener) {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut line = String::new();
        BufReader::new(r).read_line(&mut line).await.unwrap();
        w.write_all(format!("echo:{}", line.trim_end()).as_bytes()).await.unwrap();
        w.write_all(b"\n").await.unwrap();
    }

    #[tokio::test]
    async fn connects_writes_and_reads_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(echo_once(listener));

        let transport = UnixSocketTransport::new(&path);
        let mut conn = transport.connect().await.unwrap();
        conn.write_line("hello").await.unwrap();
        assert_eq!(conn.read_line().await.unwrap().as_deref(), Some("echo:hello"));
        assert_eq!(conn.read_line().await.unwrap(), None, "server closed after one line");
    }

    #[tokio::test]
    async fn missing_socket_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let transport = UnixSocketTransport::new(dir.path().join("absent.sock"));
        match transport.connect().await {
            Err(TransportError::NotRunning(msg)) => assert!(msg.contains("absent.sock")),
            other => panic!("expected NotRunning, got {other:?}"),
        }
        assert!(transport.describe().contains("absent.sock"));
    }
}
```

Add to `crates/herdr-client/src/lib.rs`:

```rust
pub mod transport;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p herdr-client transport`
Expected: compile error, `UnixSocketTransport` and `Transport` not found.

- [ ] **Step 3: Implement the transport trait and the Unix transport**

`crates/herdr-client/src/transport.rs`:

```rust
//! Transports produce one-shot connections to a herdr server.
//!
//! herdr closes a connection after one normal response and keeps it open only
//! for `events.subscribe`, so a [`Transport`] hands out a fresh [`Connection`]
//! per request and the caller drops it when done.

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub mod bridge;
#[cfg(windows)]
pub mod named_pipe;
#[cfg(unix)]
pub mod unix;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The server is not reachable at all (socket missing, connection refused).
    #[error("herdr is not running ({0})")]
    NotRunning(String),
    /// A child-process bridge failed to start or never printed its ready marker.
    #[error("bridge failed: {0}")]
    Launch(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// One open connection: write JSON lines, read JSON lines.
#[async_trait]
pub trait Connection: Send {
    async fn write_line(&mut self, line: &str) -> Result<(), TransportError>;
    /// The next line without its trailing newline, or `None` at EOF.
    async fn read_line(&mut self) -> Result<Option<String>, TransportError>;
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError>;
    /// Human-readable description for logs and tooltips, e.g. `unix socket /x/herdr.sock`.
    fn describe(&self) -> String;
}

/// A [`Connection`] over any async reader/writer pair.
pub struct StreamConnection<R, W> {
    reader: BufReader<R>,
    writer: W,
}

impl<R, W> StreamConnection<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader: BufReader::new(reader), writer }
    }
}

#[async_trait]
impl<R, W> Connection for StreamConnection<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    async fn write_line(&mut self, line: &str) -> Result<(), TransportError> {
        self.writer.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            self.writer.write_all(b"\n").await?;
        }
        self.writer.flush().await?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<Option<String>, TransportError> {
        let mut buf = String::new();
        let n = self.reader.read_line(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
        while buf.ends_with('\n') || buf.ends_with('\r') {
            buf.pop();
        }
        Ok(Some(buf))
    }
}
```

`crates/herdr-client/src/transport/unix.rs` (above the tests):

```rust
//! Unix domain socket transport (macOS, Linux).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::net::UnixStream;

use super::{Connection, StreamConnection, Transport, TransportError};

#[derive(Debug, Clone)]
pub struct UnixSocketTransport {
    path: PathBuf,
}

impl UnixSocketTransport {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self { path: path.as_ref().to_path_buf() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl Transport for UnixSocketTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        match UnixStream::connect(&self.path).await {
            Ok(stream) => {
                let (r, w) = stream.into_split();
                Ok(Box::new(StreamConnection::new(r, w)))
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                Err(TransportError::NotRunning(format!("{}: {e}", self.path.display())))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn describe(&self) -> String {
        format!("unix socket {}", self.path.display())
    }
}
```

Create `crates/herdr-client/src/transport/bridge.rs` as a stub so the module list compiles (Task 8 fills it in):

```rust
//! Child-process bridge transport (ssh / wsl.exe / local herdr.exe). See Task 8.
```

On a Mac the `named_pipe` module is `cfg(windows)` and not compiled; it is created in Task 9.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client transport`
Expected: `2 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/transport.rs crates/herdr-client/src/transport/unix.rs crates/herdr-client/src/transport/bridge.rs crates/herdr-client/src/lib.rs
git commit -m "feat(client): transport trait, stream connection and unix socket transport"
```

---

### Task 6: `FakeHerdrServer` test helper

**Files:**
- Create: `crates/herdr-client/src/test_support.rs`
- Modify: `crates/herdr-client/src/lib.rs`
- Modify: `crates/herdr-client/Cargo.toml` (feature deps)

The fake speaks exactly like herdr: one request per connection, then close; `events.subscribe` gets an ack and stays open; pushed lines go to every open subscription connection.

- [ ] **Step 1: Write the failing test**

Create `crates/herdr-client/tests/fake_server.rs`:

```rust
//! The fake must behave like herdr before anything is tested against it.
#![cfg(all(unix, feature = "test-support"))]

use herdr_client::test_support::{FakeHerdrServer, Reply};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

async fn raw_exchange(server: &FakeHerdrServer, request: &str) -> Vec<String> {
    let stream = UnixStream::connect(server.socket_path()).await.unwrap();
    let (r, mut w) = stream.into_split();
    w.write_all(request.as_bytes()).await.unwrap();
    w.write_all(b"\n").await.unwrap();
    let mut lines = Vec::new();
    let mut reader = BufReader::new(r);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap() == 0 {
            break;
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

#[tokio::test]
async fn answers_ping_then_closes() {
    let server = FakeHerdrServer::start().await;
    let lines = raw_exchange(&server, r#"{"id":"a","method":"ping","params":{}}"#).await;
    assert_eq!(lines.len(), 1, "one response then EOF");
    assert!(lines[0].contains(r#""type":"pong""#));
    assert_eq!(server.requests().len(), 1);
    assert_eq!(server.requests()[0]["method"], "ping");
}

#[tokio::test]
async fn default_snapshot_is_the_fixture() {
    let server = FakeHerdrServer::start().await;
    let lines = raw_exchange(&server, r#"{"id":"s","method":"session.snapshot","params":{}}"#).await;
    assert!(lines[0].contains(r#""type":"session_snapshot""#));
    assert!(lines[0].contains("w2:p1"));
}

#[tokio::test]
async fn custom_responder_and_error_reply() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _params| match method {
        "agent.read" => Reply::Error { code: "not_found".into(), message: "no such agent".into() },
        _ => Reply::Result(serde_json::json!({"type":"ok"})),
    });
    let lines = raw_exchange(&server, r#"{"id":"r","method":"agent.read","params":{"target":"x","source":"recent_unwrapped"}}"#).await;
    assert_eq!(lines, vec![r#"{"id":"r","error":{"code":"not_found","message":"no such agent"}}"#.to_string()]);
}

#[tokio::test]
async fn subscription_stays_open_and_receives_pushed_lines() {
    let server = FakeHerdrServer::start().await;
    let stream = UnixStream::connect(server.socket_path()).await.unwrap();
    let (r, mut w) = stream.into_split();
    w.write_all(br#"{"id":"sub","method":"events.subscribe","params":{"subscriptions":[{"type":"pane.created"}]}}"#).await.unwrap();
    w.write_all(b"\n").await.unwrap();
    let mut reader = BufReader::new(r);
    let mut ack = String::new();
    reader.read_line(&mut ack).await.unwrap();
    assert!(ack.contains("subscription_started"));

    server.wait_for_subscribers(1).await;
    server.push_json(serde_json::json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}));
    let mut event = String::new();
    reader.read_line(&mut event).await.unwrap();
    assert!(event.contains(r#""event":"pane_closed""#));

    server.close_subscriptions();
    let mut end = String::new();
    assert_eq!(reader.read_line(&mut end).await.unwrap(), 0, "closed by server");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p herdr-client --features test-support --test fake_server`
Expected: compile error, `herdr_client::test_support` not found.

- [ ] **Step 3: Implement the fake server**

Add to `crates/herdr-client/src/lib.rs`:

```rust
#[cfg(all(unix, feature = "test-support"))]
pub mod test_support;
```

Change the `[features]` and `[dependencies]` sections of `crates/herdr-client/Cargo.toml` so the fake can use a temp dir:

```toml
[features]
test-support = ["dep:tempfile"]

[dependencies]
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
async-trait.workspace = true
tempfile = { version = "3", optional = true }
```

`crates/herdr-client/src/test_support.rs`:

```rust
//! An in-process fake herdr server for tests. Unix only.
//!
//! Behaviour mirrors herdr: each connection serves one request and is closed,
//! except `events.subscribe`, which is acknowledged and kept open so pushed
//! lines can be delivered until [`FakeHerdrServer::close_subscriptions`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

use crate::transport::unix::UnixSocketTransport;

const SNAPSHOT_FIXTURE: &str = include_str!("../tests/fixtures/snapshot.json");
const CLOSE_SENTINEL: &str = "\u{0}close\u{0}";

/// What the fake answers for one request.
pub enum Reply {
    /// A success `result` object (must contain `"type"`).
    Result(Value),
    Error { code: String, message: String },
    /// Close the connection without writing anything.
    CloseSilently,
}

pub type Responder = Arc<dyn Fn(&str, &Value) -> Reply + Send + Sync>;

struct Inner {
    responder: Mutex<Responder>,
    requests: Mutex<Vec<Value>>,
    pushed: broadcast::Sender<String>,
    subscribers: AtomicUsize,
}

pub struct FakeHerdrServer {
    path: PathBuf,
    _dir: tempfile::TempDir,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

/// The `result` object of the committed snapshot fixture.
pub fn fixture_snapshot_result() -> Value {
    let v: Value = serde_json::from_str(SNAPSHOT_FIXTURE).expect("fixture is valid JSON");
    v["result"].clone()
}

fn default_responder() -> Responder {
    Arc::new(|method, _params| match method {
        "ping" => Reply::Result(serde_json::json!({"type":"pong","version":"0.9.3","protocol":22,"capabilities":{}})),
        "session.snapshot" => Reply::Result(fixture_snapshot_result()),
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        "agent.read" => Reply::Result(serde_json::json!({"type":"pane_read","read":{
            "pane_id":"w2:p1","workspace_id":"w2","tab_id":"w2:t1","source":"recent_unwrapped",
            "format":"text","text":"$ echo hi\nhi","revision":1,"truncated":false}})),
        "agent.send_keys" => Reply::Result(serde_json::json!({"type":"ok"})),
        "agent.prompt" => Reply::Result(serde_json::json!({"type":"agent_prompted","agent":{"pane_id":"w2:p1","workspace_id":"w2","agent_status":"working"}})),
        other => Reply::Error { code: "invalid_request".into(), message: format!("unknown method {other}") },
    })
}

impl FakeHerdrServer {
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).expect("bind fake socket");
        let (pushed, _) = broadcast::channel(64);
        let inner = Arc::new(Inner {
            responder: Mutex::new(default_responder()),
            requests: Mutex::new(Vec::new()),
            pushed,
            subscribers: AtomicUsize::new(0),
        });
        let accept_inner = Arc::clone(&inner);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                tokio::spawn(serve_connection(stream, Arc::clone(&accept_inner)));
            }
        });
        Self { path, _dir: dir, inner, task }
    }

    pub fn socket_path(&self) -> &Path {
        &self.path
    }

    pub fn transport(&self) -> UnixSocketTransport {
        UnixSocketTransport::new(&self.path)
    }

    /// Replace the responder for all later requests.
    pub fn set_responder(&self, f: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static) {
        *self.inner.responder.lock().unwrap() = Arc::new(f);
    }

    /// Every request received so far, as parsed JSON, in order.
    pub fn requests(&self) -> Vec<Value> {
        self.inner.requests.lock().unwrap().clone()
    }

    /// Push one raw line to every open subscription connection.
    pub fn push_line(&self, line: &str) {
        let _ = self.inner.pushed.send(line.to_string());
    }

    pub fn push_json(&self, value: Value) {
        self.push_line(&value.to_string());
    }

    /// Close every open subscription connection (simulates herdr dropping the stream).
    pub fn close_subscriptions(&self) {
        let _ = self.inner.pushed.send(CLOSE_SENTINEL.to_string());
    }

    pub fn subscriber_count(&self) -> usize {
        self.inner.subscribers.load(Ordering::SeqCst)
    }

    /// Wait until at least `n` subscription connections are open (bounded to 2 s).
    pub async fn wait_for_subscribers(&self, n: usize) {
        for _ in 0..200 {
            if self.subscriber_count() >= n {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("expected {n} subscribers, have {}", self.subscriber_count());
    }
}

impl Drop for FakeHerdrServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_connection(stream: UnixStream, inner: Arc<Inner>) {
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let request: Value = match serde_json::from_str(line.trim_end()) {
        Ok(v) => v,
        Err(e) => {
            let _ = w.write_all(format!(r#"{{"id":"","error":{{"code":"invalid_request","message":"{e}"}}}}"#).as_bytes()).await;
            let _ = w.write_all(b"\n").await;
            return;
        }
    };
    inner.requests.lock().unwrap().push(request.clone());
    let id = request["id"].as_str().unwrap_or("").to_string();
    let method = request["method"].as_str().unwrap_or("").to_string();
    let params = request["params"].clone();
    let responder = Arc::clone(&*inner.responder.lock().unwrap());
    let reply = responder(&method, &params);
    let (response, keep_open) = match reply {
        Reply::Result(result) => (serde_json::json!({"id": id, "result": result}), method == "events.subscribe"),
        Reply::Error { code, message } => (serde_json::json!({"id": id, "error": {"code": code, "message": message}}), false),
        Reply::CloseSilently => return,
    };
    if w.write_all(format!("{response}\n").as_bytes()).await.is_err() {
        return;
    }
    if !keep_open {
        return;
    }
    let mut rx = inner.pushed.subscribe();
    inner.subscribers.fetch_add(1, Ordering::SeqCst);
    loop {
        match rx.recv().await {
            Ok(msg) if msg == CLOSE_SENTINEL => break,
            Ok(msg) => {
                if w.write_all(format!("{msg}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    inner.subscribers.fetch_sub(1, Ordering::SeqCst);
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client --features test-support --test fake_server`
Expected: `4 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/test_support.rs crates/herdr-client/tests/fake_server.rs crates/herdr-client/src/lib.rs crates/herdr-client/Cargo.toml
git commit -m "test(client): in-process fake herdr server behind test-support feature"
```

---

### Task 7: `Client::request` (one request per connection)

**Files:**
- Create: `crates/herdr-client/src/client.rs`
- Create: `crates/herdr-client/tests/client_fake_server.rs`
- Modify: `crates/herdr-client/src/test_support.rs` (add `Reply::RawLine`)
- Modify: `crates/herdr-client/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

`crates/herdr-client/tests/client_fake_server.rs`:

```rust
#![cfg(all(unix, feature = "test-support"))]

use std::sync::Arc;

use herdr_client::client::{Client, ClientError};
use herdr_client::protocol::{AgentStatus, Method, ResponseResult};
use herdr_client::test_support::{FakeHerdrServer, Reply};
use herdr_client::transport::unix::UnixSocketTransport;

fn client_for(server: &FakeHerdrServer) -> Client {
    Client::new(Arc::new(server.transport()))
}

#[tokio::test]
async fn ping_returns_pong() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    match client.request(Method::Ping).await.unwrap() {
        ResponseResult::Pong { version, protocol } => {
            assert_eq!(version, "0.9.3");
            assert_eq!(protocol, 22);
        }
        other => panic!("unexpected {other:?}"),
    }
    // ids are unique and increasing
    client.request(Method::Ping).await.unwrap();
    let reqs = server.requests();
    assert_eq!(reqs[0]["id"], "hs-1");
    assert_eq!(reqs[1]["id"], "hs-2");
}

#[tokio::test]
async fn snapshot_is_typed() {
    let server = FakeHerdrServer::start().await;
    let snapshot = client_for(&server).snapshot().await.unwrap();
    assert_eq!(snapshot.agents.len(), 2);
    assert_eq!(snapshot.agents.iter().find(|a| a.pane_id == "w2:p1").unwrap().agent_status, AgentStatus::Working);
}

#[tokio::test]
async fn api_error_is_returned_as_client_error_api() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::Error { code: "agent_blocked".into(), message: "waiting at a prompt".into() });
    let err = client_for(&server).request(Method::AgentPrompt { target: "w2:p1".into(), text: "hi".into() }).await.unwrap_err();
    match err {
        ClientError::Api(body) => {
            assert_eq!(body.code, "agent_blocked");
            assert_eq!(body.message, "waiting at a prompt");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn connection_closed_before_response_is_protocol_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::CloseSilently);
    let err = client_for(&server).request(Method::Ping).await.unwrap_err();
    assert!(matches!(err, ClientError::Protocol(msg) if msg.contains("closed")), "{err:?}");
}

#[tokio::test]
async fn malformed_line_is_protocol_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::RawLine("this is not json".into()));
    let err = client_for(&server).request(Method::Ping).await.unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn mismatched_id_is_protocol_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::RawLine(r#"{"id":"someone-else","result":{"type":"ok"}}"#.into()));
    let err = client_for(&server).request(Method::Ping).await.unwrap_err();
    assert!(matches!(err, ClientError::Protocol(msg) if msg.contains("id")), "{err:?}");
}

#[tokio::test]
async fn not_running_transport_error_passes_through() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(Arc::new(UnixSocketTransport::new(dir.path().join("none.sock"))));
    let err = client.request(Method::Ping).await.unwrap_err();
    assert!(matches!(err, ClientError::Transport(herdr_client::transport::TransportError::NotRunning(_))), "{err:?}");
}
```

Add the raw-line variant to `Reply` in `crates/herdr-client/src/test_support.rs`:

```rust
pub enum Reply {
    Result(Value),
    Error { code: String, message: String },
    CloseSilently,
    /// Write exactly this line (no id handling) and close. For malformed-response tests.
    RawLine(String),
}
```

and handle it in `serve_connection`, replacing the `let (response, keep_open) = match reply { ... };` block with:

```rust
    let (response, keep_open) = match reply {
        Reply::Result(result) => (serde_json::json!({"id": id, "result": result}).to_string(), method == "events.subscribe"),
        Reply::Error { code, message } => (serde_json::json!({"id": id, "error": {"code": code, "message": message}}).to_string(), false),
        Reply::RawLine(line) => (line, false),
        Reply::CloseSilently => return,
    };
```

(`response` is now a `String`; the following `write_all(format!("{response}\n")…)` line is unchanged.)

Add to `crates/herdr-client/src/lib.rs`:

```rust
pub mod client;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-client --features test-support --test client_fake_server`
Expected: compile error, `herdr_client::client` not found.

- [ ] **Step 3: Implement `Client::request`**

`crates/herdr-client/src/client.rs`:

```rust
//! High-level client: one request per connection, plus event streams.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::protocol::{encode_request, parse_line, ErrorBody, Method, ResponseResult, ServerLine, Subscription};
use crate::snapshot::SessionSnapshot;
use crate::transport::{Connection, Transport, TransportError};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// herdr answered with an error object.
    #[error("herdr error {}: {}", .0.code, .0.message)]
    Api(ErrorBody),
    /// The server broke the protocol (closed early, unparseable line, wrong id).
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl ClientError {
    pub fn api_code(&self) -> Option<&str> {
        match self {
            ClientError::Api(body) => Some(body.code.as_str()),
            _ => None,
        }
    }
}

pub struct Client {
    transport: Arc<dyn Transport>,
    next_id: AtomicU64,
}

impl Client {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport, next_id: AtomicU64::new(1) }
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    fn next_id(&self) -> String {
        format!("hs-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Open a connection, send one request, read one response, drop the connection.
    pub async fn request(&self, method: Method) -> Result<ResponseResult, ClientError> {
        let id = self.next_id();
        let mut conn = self.transport.connect().await?;
        conn.write_line(&encode_request(&id, &method)).await?;
        let line = conn
            .read_line()
            .await?
            .ok_or_else(|| ClientError::Protocol(format!("connection closed before response to {}", method.name())))?;
        match parse_line(&line).map_err(|e| ClientError::Protocol(format!("unparseable response: {e}: {line}")))? {
            ServerLine::Response { id: got, result } if got == id => Ok(result),
            ServerLine::Error { id: got, error } if got == id => Err(ClientError::Api(error)),
            ServerLine::Response { id: got, .. } | ServerLine::Error { id: got, .. } => {
                Err(ClientError::Protocol(format!("response id {got} does not match request id {id}")))
            }
            ServerLine::Event(_) => Err(ClientError::Protocol("got an event while waiting for a response".into())),
        }
    }

    /// `session.snapshot`, typed.
    pub async fn snapshot(&self) -> Result<SessionSnapshot, ClientError> {
        match self.request(Method::SessionSnapshot).await? {
            ResponseResult::SessionSnapshot { snapshot } => Ok(snapshot),
            other => Err(ClientError::Protocol(format!("expected session_snapshot, got {other:?}"))),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client --features test-support`
Expected: all pass, including the 7 new client tests and the 4 fake-server tests.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/client.rs crates/herdr-client/tests/client_fake_server.rs crates/herdr-client/src/test_support.rs crates/herdr-client/src/lib.rs
git commit -m "feat(client): one-shot request client with typed snapshot"
```

---

### Task 8: `Client::subscribe` and `EventStream`

**Files:**
- Modify: `crates/herdr-client/src/client.rs`
- Modify: `crates/herdr-client/tests/client_fake_server.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/herdr-client/tests/client_fake_server.rs`:

```rust
use herdr_client::protocol::{Event, Subscription};

#[tokio::test]
async fn subscribe_acks_then_streams_events_until_eof() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    let mut stream = client
        .subscribe(vec![Subscription::lifecycle("pane.closed"), Subscription::pane_agent_status("w2:p1")])
        .await
        .unwrap();
    let sent = &server.requests()[0];
    assert_eq!(sent["method"], "events.subscribe");
    assert_eq!(sent["params"]["subscriptions"][1]["pane_id"], "w2:p1");

    server.wait_for_subscribers(1).await;
    server.push_json(serde_json::json!({"event":"pane.agent_status_changed","data":{"pane_id":"w2:p1","workspace_id":"w2","agent_status":"blocked","agent":"claude"}}));
    server.push_json(serde_json::json!({"event":"workspace_renamed","data":{"type":"workspace_renamed","workspace_id":"w1","label":"x"}}));
    server.push_json(serde_json::json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}));

    match stream.next().await.unwrap().unwrap() {
        Event::PaneAgentStatusChanged(e) => assert_eq!(e.agent_status, AgentStatus::Blocked),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(stream.next().await.unwrap().unwrap(), Event::Other));
    assert!(matches!(stream.next().await.unwrap().unwrap(), Event::PaneClosed { pane_id, .. } if pane_id == "w1:p2"));

    server.close_subscriptions();
    assert!(stream.next().await.unwrap().is_none(), "EOF after server closes");
}

#[tokio::test]
async fn rejected_subscription_is_api_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::Error { code: "invalid_request".into(), message: "missing field `pane_id`".into() });
    let err = client_for(&server).subscribe(vec![Subscription::lifecycle("pane.closed")]).await.unwrap_err();
    assert_eq!(err.api_code(), Some("invalid_request"));
}

#[tokio::test]
async fn events_lost_surfaces_as_api_error_from_next() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    let mut stream = client.subscribe(vec![Subscription::lifecycle("pane.closed")]).await.unwrap();
    server.wait_for_subscribers(1).await;
    server.push_line(r#"{"id":"hs-1","error":{"code":"events_lost","message":"subscriber fell behind"}}"#);
    let err = stream.next().await.unwrap_err();
    assert_eq!(err.api_code(), Some("events_lost"));
}

#[tokio::test]
async fn malformed_event_line_is_skipped_not_fatal() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    let mut stream = client.subscribe(vec![Subscription::lifecycle("pane.closed")]).await.unwrap();
    server.wait_for_subscribers(1).await;
    server.push_line("garbage line");
    server.push_json(serde_json::json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}));
    assert!(matches!(stream.next().await.unwrap().unwrap(), Event::PaneClosed { .. }));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-client --features test-support --test client_fake_server`
Expected: compile error, `subscribe` not found on `Client`.

- [ ] **Step 3: Implement `subscribe` and `EventStream`**

Add to `crates/herdr-client/src/client.rs`:

```rust
/// An open `events.subscribe` connection.
pub struct EventStream {
    conn: Box<dyn Connection>,
}

impl EventStream {
    /// The next event. `Ok(None)` at EOF. An error line from the server (for
    /// example `events_lost`) is returned as `ClientError::Api`. Unparseable
    /// lines are logged and skipped.
    pub async fn next(&mut self) -> Result<Option<crate::protocol::Event>, ClientError> {
        loop {
            let Some(line) = self.conn.read_line().await? else {
                return Ok(None);
            };
            match parse_line(&line) {
                Ok(ServerLine::Event(event)) => return Ok(Some(event)),
                Ok(ServerLine::Error { error, .. }) => return Err(ClientError::Api(error)),
                Ok(ServerLine::Response { .. }) => {
                    tracing::warn!(line, "unexpected response on event stream; ignoring");
                }
                Err(e) => {
                    tracing::warn!(error = %e, line, "skipping unparseable event line");
                }
            }
        }
    }
}

impl Client {
    /// Open a subscription. The first line must be the `subscription_started`
    /// ack; an error line means herdr rejected the whole request.
    pub async fn subscribe(&self, subscriptions: Vec<Subscription>) -> Result<EventStream, ClientError> {
        let id = self.next_id();
        let mut conn = self.transport.connect().await?;
        conn.write_line(&encode_request(&id, &Method::EventsSubscribe(subscriptions))).await?;
        let line = conn
            .read_line()
            .await?
            .ok_or_else(|| ClientError::Protocol("connection closed before subscription ack".into()))?;
        match parse_line(&line).map_err(|e| ClientError::Protocol(format!("unparseable subscription ack: {e}: {line}")))? {
            ServerLine::Response { id: got, result: ResponseResult::SubscriptionStarted } if got == id => Ok(EventStream { conn }),
            ServerLine::Error { error, .. } => Err(ClientError::Api(error)),
            other => Err(ClientError::Protocol(format!("expected subscription_started, got {other:?}"))),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client --features test-support`
Expected: all pass (4 new subscription tests included).

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/client.rs crates/herdr-client/tests/client_fake_server.rs
git commit -m "feat(client): event subscriptions with events_lost and EOF handling"
```

---

### Task 9: `BridgeTransport` (child process with ready marker)

**Files:**
- Modify: `crates/herdr-client/src/transport/bridge.rs` (replace stub)
- Modify: `crates/herdr-client/src/transport.rs` (add `StreamConnection::from_buffered`)
- Create: `crates/herdr-client/tests/bridge_transport.rs`

The bridge runs a command whose stdin/stdout carry the JSON stream (`ssh host … remote-api-bridge`, `wsl.exe …`, or a local `herdr.exe … remote-api-bridge`). Everything on stdout before the line `herdr-remote-output-ready:1` is noise (SSH banners) and is discarded. The exact command lines are built in Plan 2; this crate only runs them.

- [ ] **Step 1: Write the failing tests**

`crates/herdr-client/tests/bridge_transport.rs`:

```rust
#![cfg(unix)]

use std::time::Duration;

use herdr_client::transport::bridge::{BridgeTransport, ChildCommand, READY_MARKER};
use herdr_client::transport::{Transport, TransportError};

fn sh(script: &str) -> ChildCommand {
    ChildCommand { program: "sh".into(), args: vec!["-c".into(), script.into()] }
}

#[tokio::test]
async fn skips_banner_then_bridges_lines_both_ways() {
    let script = format!(
        r#"echo "Welcome to host"; echo; printf '\n%s\n' '{READY_MARKER}'; while IFS= read -r l; do echo "echo:$l"; done"#
    );
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    conn.write_line(r#"{"id":"1","method":"ping","params":{}}"#).await.unwrap();
    assert_eq!(
        conn.read_line().await.unwrap().as_deref(),
        Some(r#"echo:{"id":"1","method":"ping","params":{}}"#)
    );
    conn.write_line("second").await.unwrap();
    assert_eq!(conn.read_line().await.unwrap().as_deref(), Some("echo:second"));
}

#[tokio::test]
async fn eof_after_marker_reads_none() {
    let script = format!(r#"printf '\n%s\n' '{READY_MARKER}'; exit 0"#);
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    assert_eq!(conn.read_line().await.unwrap(), None);
}

#[tokio::test]
async fn child_exit_before_marker_is_launch_error_with_stderr() {
    let transport = BridgeTransport::new(sh(r#"echo "herdr: command not found" >&2; exit 127"#), "test bridge");
    match transport.connect().await {
        Err(TransportError::Launch(msg)) => assert!(msg.contains("command not found"), "{msg}"),
        other => panic!("expected Launch error, got {other:?}"),
    }
}

#[tokio::test]
async fn no_marker_within_timeout_is_timeout_error() {
    let transport = BridgeTransport::new(sh(r#"echo "still negotiating"; sleep 5"#), "test bridge")
        .with_ready_timeout(Duration::from_millis(300));
    assert!(matches!(transport.connect().await, Err(TransportError::Timeout(_))));
}

#[tokio::test]
async fn missing_program_is_launch_error() {
    let command = ChildCommand { program: "definitely-not-a-program-xyz".into(), args: vec![] };
    let transport = BridgeTransport::new(command, "nope");
    assert!(matches!(transport.connect().await, Err(TransportError::Launch(_))));
    assert_eq!(transport.describe(), "nope");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p herdr-client --test bridge_transport`
Expected: compile error, `BridgeTransport` not found.

- [ ] **Step 3: Implement the bridge transport**

Add this constructor to `impl<R, W> StreamConnection<R, W>` in `crates/herdr-client/src/transport.rs`:

```rust
    /// Wrap a reader that is already buffered (the bridge consumes leading
    /// lines up to the ready marker and must not lose what is buffered).
    pub fn from_buffered(reader: BufReader<R>, writer: W) -> Self {
        Self { reader, writer }
    }
```

Replace `crates/herdr-client/src/transport/bridge.rs` with:

```rust
//! Child-process bridge transport.
//!
//! Spawns a command whose stdin/stdout carry herdr's JSON stream, for example
//! `ssh host /bin/sh -c '… exec herdr --session default remote-api-bridge'`,
//! `wsl.exe -- sh -lc '…'`, or a local `herdr.exe --session default remote-api-bridge`.
//! Stdout lines before [`READY_MARKER`] are discarded so SSH banners never
//! reach the JSON parser.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::{Connection, StreamConnection, Transport, TransportError};

/// Printed by the remote script right before the JSON stream starts. Same
/// string herdr's own saved-machine bridge uses.
pub const READY_MARKER: &str = "herdr-remote-output-ready:1";

const STDERR_CAP: usize = 4096;

/// A program plus arguments, run without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BridgeTransport {
    command: ChildCommand,
    description: String,
    ready_timeout: Duration,
}

impl BridgeTransport {
    pub fn new(command: ChildCommand, description: impl Into<String>) -> Self {
        Self { command, description: description.into(), ready_timeout: Duration::from_secs(20) }
    }

    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    pub fn command(&self) -> &ChildCommand {
        &self.command
    }
}

/// Keeps the child alive for as long as the connection is used; `kill_on_drop`
/// reaps it afterwards.
struct BridgeConnection {
    _child: Child,
    inner: StreamConnection<ChildStdout, ChildStdin>,
}

#[async_trait]
impl Connection for BridgeConnection {
    async fn write_line(&mut self, line: &str) -> Result<(), TransportError> {
        self.inner.write_line(line).await
    }

    async fn read_line(&mut self) -> Result<Option<String>, TransportError> {
        self.inner.read_line().await
    }
}

#[async_trait]
impl Transport for BridgeTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        let mut cmd = Command::new(&self.command.program);
        cmd.args(&self.command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Launch(format!("{}: cannot start {}: {e}", self.description, self.command.program)))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        tokio::spawn(collect_stderr(stderr, Arc::clone(&stderr_buf)));

        let mut reader = BufReader::new(stdout);
        match tokio::time::timeout(self.ready_timeout, skip_to_marker(&mut reader)).await {
            Err(_elapsed) => Err(TransportError::Timeout(format!(
                "{}: no ready marker within {:?}",
                self.description, self.ready_timeout
            ))),
            Ok(Err(e)) => Err(e.into()),
            Ok(Ok(false)) => {
                // Give the stderr collector a moment to drain what the child wrote before exiting.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stderr_text = stderr_buf.lock().unwrap().trim().to_string();
                Err(TransportError::Launch(format!(
                    "{}: exited before ready marker: {stderr_text}",
                    self.description
                )))
            }
            Ok(Ok(true)) => Ok(Box::new(BridgeConnection {
                _child: child,
                inner: StreamConnection::from_buffered(reader, stdin),
            })),
        }
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

/// Returns `Ok(true)` once the marker line was consumed, `Ok(false)` at EOF.
async fn skip_to_marker(reader: &mut BufReader<ChildStdout>) -> std::io::Result<bool> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(false);
        }
        if line.trim_end() == READY_MARKER {
            return Ok(true);
        }
        tracing::debug!(line = line.trim_end(), "bridge output before ready marker");
    }
}

async fn collect_stderr(mut stderr: ChildStderr, buf: Arc<Mutex<String>>) {
    let mut chunk = [0u8; 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut text = buf.lock().unwrap();
                if text.len() < STDERR_CAP {
                    text.push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
        }
    }
}
```

On Windows `creation_flags` needs `use std::os::windows::process::CommandExt;` at the top of the file under `#[cfg(windows)]`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p herdr-client --test bridge_transport`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/transport/bridge.rs crates/herdr-client/src/transport.rs crates/herdr-client/tests/bridge_transport.rs
git commit -m "feat(client): child-process bridge transport with ready-marker framing"
```

---

### Task 10: Named pipe name helper and `NamedPipeTransport` (Windows)

**Files:**
- Create: `crates/herdr-client/src/transport/pipe_name.rs`
- Create: `crates/herdr-client/src/transport/named_pipe.rs`
- Modify: `crates/herdr-client/src/transport.rs` (module list)

The name helper compiles and is tested on every OS. The transport itself only compiles on Windows; its test runs on the Windows CI job added in Plan 3.

- [ ] **Step 1: Write the failing test for the name helper**

`crates/herdr-client/src/transport/pipe_name.rs`:

```rust
//! Windows named-pipe name derivation. Platform independent so it is unit
//! tested on the Mac too.

/// herdr opens its socket "path" through the `interprocess` crate's
/// `GenericNamespaced` mapping, which prepends this prefix to the path string
/// verbatim (backslashes and drive letter included).
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

pub fn pipe_name_for(socket_path: &str) -> String {
    format!("{PIPE_PREFIX}{socket_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepends_pipe_prefix_verbatim() {
        assert_eq!(
            pipe_name_for(r"C:\Users\me\AppData\Roaming\herdr\herdr.sock"),
            r"\\.\pipe\C:\Users\me\AppData\Roaming\herdr\herdr.sock"
        );
    }
}
```

Add to the module list in `crates/herdr-client/src/transport.rs`:

```rust
pub mod pipe_name;
```

- [ ] **Step 2: Run the test to verify it passes** (it is pure; TDD here means the test exists before any caller)

Run: `cargo test -p herdr-client pipe_name`
Expected: `1 passed`.

- [ ] **Step 3: Write the Windows transport and its test**

`crates/herdr-client/src/transport/named_pipe.rs`:

```rust
//! Windows named pipe transport (local native herdr).

use std::io::ErrorKind;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::net::windows::named_pipe::ClientOptions;
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use super::pipe_name::pipe_name_for;
use super::{Connection, StreamConnection, Transport, TransportError};

#[derive(Debug, Clone)]
pub struct NamedPipeTransport {
    pipe_name: String,
}

impl NamedPipeTransport {
    pub fn new(pipe_name: impl Into<String>) -> Self {
        Self { pipe_name: pipe_name.into() }
    }

    /// Build from herdr's socket path string (`%APPDATA%\herdr\herdr.sock`).
    pub fn for_socket_path(socket_path: &str) -> Self {
        Self::new(pipe_name_for(socket_path))
    }
}

#[async_trait]
impl Transport for NamedPipeTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match ClientOptions::new().open(&self.pipe_name) {
                Ok(client) => {
                    let (r, w) = tokio::io::split(client);
                    return Ok(Box::new(StreamConnection::new(r, w)));
                }
                // The pipe exists but the server is between accepts: retry briefly.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    return Err(TransportError::NotRunning(format!("{}: {e}", self.pipe_name)));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn describe(&self) -> String {
        format!("named pipe {}", self.pipe_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    #[tokio::test]
    async fn connects_writes_and_reads_a_line() {
        let name = format!(r"\\.\pipe\herdr-client-test-{}", std::process::id());
        let server = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
        tokio::spawn(async move {
            server.connect().await.unwrap();
            let (r, mut w) = tokio::io::split(server);
            let mut line = String::new();
            BufReader::new(r).read_line(&mut line).await.unwrap();
            w.write_all(format!("echo:{}\n", line.trim_end()).as_bytes()).await.unwrap();
        });
        let transport = NamedPipeTransport::new(&name);
        let mut conn = transport.connect().await.unwrap();
        conn.write_line("hello").await.unwrap();
        assert_eq!(conn.read_line().await.unwrap().as_deref(), Some("echo:hello"));
    }

    #[tokio::test]
    async fn missing_pipe_is_not_running() {
        let transport = NamedPipeTransport::new(r"\\.\pipe\herdr-client-test-absent-pipe");
        assert!(matches!(transport.connect().await, Err(TransportError::NotRunning(_))));
    }
}
```

- [ ] **Step 4: Verify the crate still builds on macOS and the Windows file is syntactically sound**

Run: `cargo test -p herdr-client` (macOS: the `named_pipe` module is not compiled).
Expected: all previous tests pass.

Run: `cargo check -p herdr-client --target x86_64-pc-windows-msvc` if that target is installed (`rustup target add x86_64-pc-windows-msvc`); otherwise skip and rely on the Windows CI job from Plan 3.
Expected when run: `Finished` (no linker is needed for `check`).

- [ ] **Step 5: Commit**

```bash
git add crates/herdr-client/src/transport/pipe_name.rs crates/herdr-client/src/transport/named_pipe.rs crates/herdr-client/src/transport.rs
git commit -m "feat(client): windows named pipe transport and pipe name helper"
```

---

### Task 11: Re-exports, docs, lint pass

**Files:**
- Modify: `crates/herdr-client/src/lib.rs`

- [ ] **Step 1: Add the public re-exports**

Replace `crates/herdr-client/src/lib.rs` with:

```rust
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
pub use protocol::{AgentStatus, ErrorBody, Event, Method, PaneAgentStatusChanged, ResponseResult, Subscription};
pub use snapshot::{AgentInfo, PaneInfo, ReadResult, SessionSnapshot, WorkspaceInfo};
pub use transport::{Connection, Transport, TransportError};
```

- [ ] **Step 2: Format and lint**

Run: `cargo fmt --all && cargo clippy -p herdr-client --all-targets --features test-support -- -D warnings`
Expected: no warnings. Fix anything reported (typical: unused imports in test files after refactors, `needless_return`).

- [ ] **Step 3: Run everything once more**

Run: `cargo test -p herdr-client --features test-support`
Expected: all tests pass. Count: protocol 14, snapshot 2, unix transport 2, pipe_name 1, fake server 4, client 11, bridge 5.

- [ ] **Step 4: Commit**

```bash
git add -A crates/herdr-client
git commit -m "chore(client): public re-exports, docs and clippy clean"
```

---

## Plan 1 done criteria

- `cargo test -p herdr-client --features test-support` is green on macOS.
- `cargo clippy … -D warnings` is clean.
- The crate exposes: `Client`, `EventStream`, `ClientError`, `Method`, `Subscription`, `Event`, `AgentStatus`, `SessionSnapshot`, `AgentInfo`, `PaneInfo`, `Transport`, `TransportError`, `UnixSocketTransport`, `NamedPipeTransport` (Windows), `BridgeTransport`, `ChildCommand`, `READY_MARKER`, `pipe_name_for`, and `test_support::FakeHerdrServer`.
- Plan 2 (`herdr-status-core`) builds on exactly these names.
