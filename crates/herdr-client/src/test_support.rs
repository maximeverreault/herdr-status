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
    Error {
        code: String,
        message: String,
    },
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
        "ping" => Reply::Result(
            serde_json::json!({"type":"pong","version":"0.9.3","protocol":22,"capabilities":{}}),
        ),
        "session.snapshot" => Reply::Result(fixture_snapshot_result()),
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        "agent.read" => Reply::Result(serde_json::json!({"type":"pane_read","read":{
            "pane_id":"w2:p1","workspace_id":"w2","tab_id":"w2:t1","source":"recent_unwrapped",
            "format":"text","text":"$ echo hi\nhi","revision":1,"truncated":false}})),
        "agent.send_keys" => Reply::Result(serde_json::json!({"type":"ok"})),
        "agent.prompt" => Reply::Result(
            serde_json::json!({"type":"agent_prompted","agent":{"pane_id":"w2:p1","workspace_id":"w2","agent_status":"working"}}),
        ),
        other => Reply::Error {
            code: "invalid_request".into(),
            message: format!("unknown method {other}"),
        },
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
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve_connection(stream, Arc::clone(&accept_inner)));
            }
        });
        Self {
            path,
            _dir: dir,
            inner,
            task,
        }
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

// Structs (not `json!`) so `id` stays first: serde_json's default map sorts keys.
#[derive(serde::Serialize)]
struct ResultEnvelope<'a> {
    id: &'a str,
    result: Value,
}

#[derive(serde::Serialize)]
struct ErrorFields {
    code: String,
    message: String,
}

#[derive(serde::Serialize)]
struct ErrorEnvelope<'a> {
    id: &'a str,
    error: ErrorFields,
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
            let _ = w
                .write_all(
                    format!(r#"{{"id":"","error":{{"code":"invalid_request","message":"{e}"}}}}"#)
                        .as_bytes(),
                )
                .await;
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
        Reply::Result(result) => (
            serde_json::to_string(&ResultEnvelope { id: &id, result }),
            method == "events.subscribe",
        ),
        Reply::Error { code, message } => (
            serde_json::to_string(&ErrorEnvelope {
                id: &id,
                error: ErrorFields { code, message },
            }),
            false,
        ),
        Reply::CloseSilently => return,
    };
    let response = response.expect("envelope serialises");
    if w.write_all(format!("{response}\n").as_bytes())
        .await
        .is_err()
    {
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
