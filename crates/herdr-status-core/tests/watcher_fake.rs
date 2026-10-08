#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use herdr_client::test_support::{fixture_snapshot_result, FakeHerdrServer, Reply};
use herdr_client::transport::unix::UnixSocketTransport;
use herdr_status_core::model::{AgentStatus, ConnectionState, ServerKind, ServerState};
use herdr_status_core::transport_source::FixedSource;
use herdr_status_core::watcher::{
    spawn_watcher, WatcherConfig, WatcherHandle, LIFECYCLE_SUBSCRIPTIONS,
};
use tokio::sync::mpsc;

fn test_cfg() -> WatcherConfig {
    WatcherConfig {
        reconcile: Duration::from_secs(60),
        debounce: Duration::from_millis(20),
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        not_running_retry: Duration::from_millis(50),
        stable_after: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
    }
}

fn start(
    server: &FakeHerdrServer,
    cfg: WatcherConfig,
) -> (WatcherHandle, mpsc::Receiver<ServerState>) {
    let (tx, rx) = mpsc::channel(64);
    let source = Arc::new(FixedSource(Arc::new(server.transport())));
    let handle = spawn_watcher(
        ServerState::new("local", "local", ServerKind::Local),
        source,
        cfg,
        tx,
    );
    (handle, rx)
}

async fn next_matching(
    rx: &mut mpsc::Receiver<ServerState>,
    pred: impl Fn(&ServerState) -> bool,
) -> ServerState {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let s = rx.recv().await.expect("watcher is alive");
            if pred(&s) {
                return s;
            }
        }
    })
    .await
    .expect("expected a matching state within 3s")
}

fn methods(server: &FakeHerdrServer) -> Vec<String> {
    server
        .requests()
        .iter()
        .map(|r| r["method"].as_str().unwrap().to_string())
        .collect()
}

async fn wait_for_subscribe_count(server: &FakeHerdrServer, n: usize) -> Vec<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let subs: Vec<_> = server
                .requests()
                .into_iter()
                .filter(|r| r["method"] == "events.subscribe")
                .collect();
            if subs.len() >= n {
                return subs;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected subscribe requests")
}

/// Every state published during `window`; asserts the watcher stayed online.
async fn assert_stays_online(rx: &mut mpsc::Receiver<ServerState>, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(s)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        assert!(
            s.is_online(),
            "unexpected non-online state: {:?}",
            s.connection
        );
    }
}

#[tokio::test]
async fn goes_online_with_snapshot_rows_and_full_subscription() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    let online = next_matching(&mut rx, |s| s.is_online()).await;
    assert_eq!(online.rows.len(), 2);
    assert!(online.transport.contains("unix socket"));
    // The one-time protocol ping may already have been sent after S2.
    assert_eq!(
        methods(&server)[..3],
        ["session.snapshot", "events.subscribe", "session.snapshot"]
    );
    let subs = server.requests()[1]["params"]["subscriptions"].clone();
    let types: Vec<String> = subs
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["type"].as_str().unwrap().to_string())
        .collect();
    for t in LIFECYCLE_SUBSCRIPTIONS {
        assert!(
            types.contains(&t.to_string()),
            "missing lifecycle subscription {t}"
        );
    }
    let panes: Vec<&str> = subs
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["pane_id"].as_str())
        .collect();
    assert_eq!(panes, vec!["w1:p1", "w1:p2", "w2:p1"]);
    handle.shutdown().await;
}

#[tokio::test]
async fn status_event_updates_the_row() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.push_json(serde_json::json!({"event":"pane.agent_status_changed","data":{"pane_id":"w2:p1","workspace_id":"w2","agent_status":"blocked","agent":"claude"}}));
    let s = next_matching(&mut rx, |s| {
        s.rows
            .iter()
            .any(|r| r.pane_id == "w2:p1" && r.status == AgentStatus::Blocked)
    })
    .await;
    let row = s.rows.iter().find(|r| r.pane_id == "w2:p1").unwrap();
    assert!(row.since.is_some());
    assert_eq!(row.flash_seq, 1);
    handle.shutdown().await;
}

#[tokio::test]
async fn pane_created_rebuilds_subscription_with_the_new_pane_without_going_offline() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.set_responder(|method, _| match method {
        "session.snapshot" => {
            let mut r = fixture_snapshot_result();
            r["snapshot"]["panes"].as_array_mut().unwrap().push(serde_json::json!({
                "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0
            }));
            Reply::Result(r)
        }
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        other => Reply::Error { code: "invalid_request".into(), message: format!("unexpected {other}") },
    });
    server.push_json(serde_json::json!({"event":"pane_created","data":{"type":"pane_created","pane":{
        "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0}}}));
    let subs = wait_for_subscribe_count(&server, 2).await;
    assert!(subs[1]["params"]["subscriptions"]
        .to_string()
        .contains("w2:p2"));
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn events_lost_rebuilds_without_going_offline() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.push_line(
        r#"{"id":"hs-2","error":{"code":"events_lost","message":"subscriber fell behind"}}"#,
    );
    wait_for_subscribe_count(&server, 2).await;
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn server_close_goes_offline_keeps_rows_and_recovers() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.set_responder(|_, _| Reply::CloseSilently);
    server.close_subscriptions();
    let off = next_matching(&mut rx, |s| {
        matches!(s.connection, ConnectionState::Offline { .. })
    })
    .await;
    assert_eq!(off.rows.len(), 2, "rows retained while offline");
    server.reset_responder();
    next_matching(&mut rx, |s| s.is_online()).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn reconcile_timer_takes_periodic_snapshots() {
    let server = FakeHerdrServer::start().await;
    let mut cfg = test_cfg();
    cfg.reconcile = Duration::from_millis(100);
    let (handle, mut rx) = start(&server, cfg);
    next_matching(&mut rx, |s| s.is_online()).await;
    let count = || {
        methods(&server)
            .iter()
            .filter(|m| *m == "session.snapshot")
            .count()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while count() < 5 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let snapshots = count();
    assert!(
        snapshots >= 5,
        "expected initial 2 plus at least 3 reconciles, got {snapshots}"
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn reconnect_command_resubscribes() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    handle.reconnect();
    wait_for_subscribe_count(&server, 2).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn not_running_server_reports_reason_and_keeps_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let transport: Arc<dyn herdr_client::Transport> =
        Arc::new(UnixSocketTransport::new(dir.path().join("absent.sock")));
    let (tx, mut rx) = mpsc::channel(64);
    let handle = spawn_watcher(
        ServerState::new("local", "local", ServerKind::Local),
        Arc::new(FixedSource(transport)),
        test_cfg(),
        tx,
    );
    let off = next_matching(&mut rx, |s| {
        matches!(s.connection, ConnectionState::Offline { .. })
    })
    .await;
    assert!(
        matches!(&off.connection, ConnectionState::Offline { reason, .. } if reason == "herdr is not running")
    );
    let again = next_matching(&mut rx, |s| {
        matches!(s.connection, ConnectionState::Offline { .. })
    })
    .await;
    assert!(
        matches!(again.connection, ConnectionState::Offline { .. }),
        "retries keep reporting offline"
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn shutdown_ends_the_task_promptly() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    tokio::time::timeout(Duration::from_secs(1), handle.shutdown())
        .await
        .expect("shutdown within 1s");
}

#[tokio::test]
async fn pane_set_change_seen_at_reconcile_triggers_rebuild() {
    let server = FakeHerdrServer::start().await;
    let mut cfg = test_cfg();
    cfg.reconcile = Duration::from_millis(100);
    let (handle, mut rx) = start(&server, cfg);
    next_matching(&mut rx, |s| s.is_online()).await;
    server.set_responder(|method, _| match method {
        "session.snapshot" => {
            let mut r = fixture_snapshot_result();
            r["snapshot"]["panes"].as_array_mut().unwrap().push(serde_json::json!({
                "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0
            }));
            Reply::Result(r)
        }
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        other => Reply::Error { code: "invalid_request".into(), message: format!("unexpected {other}") },
    });
    let subs = wait_for_subscribe_count(&server, 2).await;
    assert!(subs[1]["params"]["subscriptions"]
        .to_string()
        .contains("w2:p2"));
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn eof_rebuilds_without_going_offline() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.close_subscriptions();
    wait_for_subscribe_count(&server, 2).await;
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

#[tokio::test]
async fn pane_churn_does_not_trip_the_rebuild_cap() {
    let server = FakeHerdrServer::start().await;
    // Long enough that no stable-session reset can mask a counted pane rebuild.
    let mut cfg = test_cfg();
    cfg.backoff_min = Duration::from_secs(10);
    cfg.stable_after = Duration::from_secs(10);
    let (handle, mut rx) = start(&server, cfg);
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    // Every snapshot alternates between including and omitting w2:p2.
    let flip = Arc::new(std::sync::atomic::AtomicBool::new(false));
    server.set_responder(move |method, _| match method {
        "session.snapshot" => {
            let mut r = fixture_snapshot_result();
            if !flip.fetch_xor(true, std::sync::atomic::Ordering::SeqCst) {
                r["snapshot"]["panes"].as_array_mut().unwrap().push(serde_json::json!({
                    "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0
                }));
            }
            Reply::Result(r)
        }
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        other => Reply::Error { code: "invalid_request".into(), message: format!("unexpected {other}") },
    });
    for i in 0..7 {
        server.wait_for_subscribers(1).await;
        server.push_json(serde_json::json!({"event":"pane_created","data":{"type":"pane_created","pane":{
            "pane_id":"w2:p2","terminal_id":"t9","workspace_id":"w2","tab_id":"w2:t1","focused":false,"agent_status":"unknown","revision":0}}}));
        wait_for_subscribe_count(&server, i + 2).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    server.wait_for_subscribers(1).await;
    server.close_subscriptions();
    wait_for_subscribe_count(&server, 9).await;
    assert_stays_online(&mut rx, Duration::from_millis(300)).await;
    handle.shutdown().await;
}

fn offline_reason(s: &ServerState) -> Option<&str> {
    match &s.connection {
        ConnectionState::Offline { reason, .. } => Some(reason),
        _ => None,
    }
}

#[tokio::test]
async fn request_timeout_fires_and_reports_offline() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _| match method {
        "session.snapshot" => Reply::Hang,
        _ => Reply::Result(serde_json::json!({"type":"subscription_started"})),
    });
    let mut cfg = test_cfg();
    cfg.request_timeout = Duration::from_millis(100);
    let (handle, mut rx) = start(&server, cfg);
    let off = next_matching(&mut rx, |s| offline_reason(s).is_some()).await;
    assert_eq!(offline_reason(&off), Some("herdr did not answer in time"));
    handle.shutdown().await;
}

#[tokio::test]
async fn shutdown_is_prompt_while_a_snapshot_hangs() {
    let server = FakeHerdrServer::start().await;
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.set_responder(|_, _| Reply::Hang);
    handle.reconnect();
    // Let the reconnect reach the hanging first snapshot.
    let before = server.requests().len();
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.requests().len() == before {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reconnect sent a request");
    tokio::time::timeout(Duration::from_secs(1), handle.shutdown())
        .await
        .expect("shutdown within 1s while a snapshot hangs");
}

#[tokio::test]
async fn shutdown_during_backoff_sleep_is_prompt() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::CloseSilently);
    let mut cfg = test_cfg();
    cfg.backoff_min = Duration::from_secs(30);
    cfg.backoff_max = Duration::from_secs(60);
    let (handle, mut rx) = start(&server, cfg);
    next_matching(&mut rx, |s| offline_reason(s).is_some()).await;
    tokio::time::timeout(Duration::from_secs(1), handle.shutdown())
        .await
        .expect("shutdown within 1s during the backoff sleep");
}

#[tokio::test]
async fn rejected_subscribe_rebuilds_then_recovers() {
    let server = FakeHerdrServer::start().await;
    let subscribes = Arc::new(AtomicUsize::new(0));
    let counter = subscribes.clone();
    server.set_responder(move |method, _| match method {
        "session.snapshot" => Reply::Result(fixture_snapshot_result()),
        "events.subscribe" if counter.fetch_add(1, Ordering::SeqCst) == 0 => Reply::Error {
            code: "invalid_request".into(),
            message: "unknown pane w1:p1".into(),
        },
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        other => Reply::Error {
            code: "invalid_request".into(),
            message: format!("unexpected {other}"),
        },
    });
    let (handle, mut rx) = start(&server, test_cfg());
    let online = next_matching(&mut rx, |s| {
        assert!(
            offline_reason(s).is_none(),
            "went offline: {:?}",
            s.connection
        );
        s.is_online()
    })
    .await;
    assert_eq!(online.rows.len(), 2);
    assert_eq!(subscribes.load(Ordering::SeqCst), 2);
    handle.shutdown().await;
}

#[tokio::test]
async fn streak_cap_trips_to_offline() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _| match method {
        "session.snapshot" => Reply::Result(fixture_snapshot_result()),
        _ => Reply::CloseSilently,
    });
    let (handle, mut rx) = start(&server, test_cfg());
    let off = next_matching(&mut rx, |s| offline_reason(s).is_some()).await;
    assert_eq!(
        offline_reason(&off),
        Some("connection closed before subscription ack")
    );
    let subscribes = methods(&server)
        .iter()
        .filter(|m| *m == "events.subscribe")
        .count();
    assert_eq!(
        subscribes, 6,
        "five rebuilds allowed, the sixth closed subscribe fails"
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn backoff_resets_after_stable_session_ends_in_eof() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::CloseSilently);
    let mut cfg = test_cfg();
    cfg.backoff_min = Duration::from_millis(50);
    cfg.backoff_max = Duration::from_secs(2);
    cfg.stable_after = Duration::from_millis(100);
    let (handle, mut rx) = start(&server, cfg);
    // Four failures: waits of 50, 100, 200 and 400 ms; the next would be 800 ms.
    for _ in 0..4 {
        next_matching(&mut rx, |s| offline_reason(s).is_some()).await;
    }
    server.reset_responder();
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    server.set_responder(|_, _| Reply::CloseSilently);
    server.close_subscriptions();
    let off = next_matching(&mut rx, |s| offline_reason(s).is_some()).await;
    let ConnectionState::Offline { retry_at, .. } = off.connection else {
        unreachable!()
    };
    let wait = retry_at
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    assert!(
        wait <= Duration::from_millis(150),
        "expected about backoff_min, got {wait:?}"
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn logs_protocol_mismatch_without_failing() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _| match method {
        "ping" => Reply::Result(serde_json::json!({"type":"pong","version":"9.9.9","protocol":23})),
        "session.snapshot" => Reply::Result(fixture_snapshot_result()),
        "events.subscribe" => Reply::Result(serde_json::json!({"type":"subscription_started"})),
        other => Reply::Error {
            code: "invalid_request".into(),
            message: format!("unexpected {other}"),
        },
    });
    let (handle, mut rx) = start(&server, test_cfg());
    next_matching(&mut rx, |s| s.is_online()).await;
    server.wait_for_subscribers(1).await;
    server.reset_responder();
    server.close_subscriptions();
    wait_for_subscribe_count(&server, 2).await;
    server.wait_for_subscribers(1).await;
    let pings = methods(&server).iter().filter(|m| *m == "ping").count();
    assert_eq!(pings, 1, "protocol is checked once per watcher");
    handle.shutdown().await;
}
