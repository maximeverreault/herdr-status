#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

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
    assert_eq!(
        methods(&server),
        vec!["session.snapshot", "events.subscribe", "session.snapshot"]
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
    tokio::time::sleep(Duration::from_millis(450)).await;
    let snapshots = methods(&server)
        .iter()
        .filter(|m| *m == "session.snapshot")
        .count();
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
