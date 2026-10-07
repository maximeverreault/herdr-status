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
use tokio::sync::{broadcast, watch};

use crate::transport::unix::UnixSocketTransport;

const SNAPSHOT_FIXTURE: &str = include_str!("../tests/fixtures/snapshot.json");

/// What the fake answers for one request.
pub enum Reply {
    /// A success `result` object (must contain `"type"`).
    Result(Value),
    /// An error response with the given code and message.
    Error { code: String, message: String },
    /// Close the connection without writing anything.
    CloseSilently,
    /// Write exactly this line (no id handling) and close. For malformed-response tests.
    RawLine(String),
}

/// Decides the [`Reply`] for a request, given its method name and `params`.
pub type Responder = Arc<dyn Fn(&str, &Value) -> Reply + Send + Sync>;

struct Inner {
    responder: Mutex<Responder>,
    requests: Mutex<Vec<Value>>,
    pushed: broadcast::Sender<String>,
    subscribers: AtomicUsize,
    /// Close generation: bumping it makes every open subscription loop exit.
    close: watch::Sender<u64>,
}

/// Decrements the subscriber count on every exit path of a subscription.
struct SubscriberGuard(Arc<Inner>);

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.0.subscribers.fetch_sub(1, Ordering::SeqCst);
    }
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
    /// Bind a fresh socket in a temp dir and start serving. Must run inside a tokio runtime.
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap_or_else(|e| {
            panic!(
                "bind fake socket {} ({} bytes; macOS limits sun_path to 104): {e}",
                path.display(),
                path.as_os_str().len()
            )
        });
        let (pushed, _) = broadcast::channel(64);
        let inner = Arc::new(Inner {
            responder: Mutex::new(default_responder()),
            requests: Mutex::new(Vec::new()),
            pushed,
            subscribers: AtomicUsize::new(0),
            close: watch::channel(0).0,
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

    /// Replace the responder; it applies to requests received after this call.
    /// A panic inside a responder only shows up to the client as EOF.
    pub fn set_responder(&self, f: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static) {
        *self.inner.responder.lock().unwrap() = Arc::new(f);
    }

    /// Every request received so far, as parsed JSON, in order.
    pub fn requests(&self) -> Vec<Value> {
        self.inner.requests.lock().unwrap().clone()
    }

    /// Push one raw line to every open subscription connection.
    ///
    /// A push with no registered subscriber is dropped, so call
    /// [`wait_for_subscribers`](Self::wait_for_subscribers) first. The
    /// broadcast buffer holds 64 lines; a subscriber that falls further behind
    /// gets an `events_lost` error line and its connection is closed.
    pub fn push_line(&self, line: &str) {
        let _ = self.inner.pushed.send(line.to_string());
    }

    /// Like [`push_line`](Self::push_line) with a JSON value; the same drop-without-subscriber caveat applies.
    pub fn push_json(&self, value: Value) {
        self.push_line(&value.to_string());
    }

    /// Close every open subscription connection (simulates herdr dropping the stream).
    pub fn close_subscriptions(&self) {
        self.inner.close.send_modify(|g| *g += 1);
    }

    /// Number of subscription connections currently open.
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
        self.inner.close.send_modify(|g| *g += 1);
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
            let reply = ErrorEnvelope {
                id: "",
                error: ErrorFields {
                    code: "invalid_request".into(),
                    message: e.to_string(),
                },
            };
            let body = serde_json::to_string(&reply).expect("envelope serialises");
            let _ = w.write_all(format!("{body}\n").as_bytes()).await;
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
        Reply::RawLine(line) => (Ok(line), false),
        Reply::CloseSilently => return,
    };
    let response = response.expect("envelope serialises");
    // Register before the ack so every push after the client reads it is delivered.
    let subscription = keep_open.then(|| {
        let rx = inner.pushed.subscribe();
        let mut closed = inner.close.subscribe();
        closed.borrow_and_update();
        inner.subscribers.fetch_add(1, Ordering::SeqCst);
        (rx, closed, SubscriberGuard(Arc::clone(&inner)))
    });
    if w.write_all(format!("{response}\n").as_bytes())
        .await
        .is_err()
    {
        return;
    }
    let Some((mut rx, mut closed, _guard)) = subscription else {
        return;
    };
    let mut scratch = Vec::new();
    loop {
        tokio::select! {
            biased;
            _ = closed.changed() => break,
            msg = rx.recv() => match msg {
                Ok(msg) => {
                    if w.write_all(format!("{msg}\n").as_bytes()).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = w
                        .write_all(b"{\"error\":{\"code\":\"events_lost\",\"message\":\"subscriber fell behind\"}}\n")
                        .await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            read = reader.read_until(b'\n', &mut scratch) => match read {
                Ok(0) | Err(_) => break,
                Ok(_) => scratch.clear(),
            },
        }
    }
}
