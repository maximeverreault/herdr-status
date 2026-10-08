#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use herdr_client::test_support::FakeHerdrServer;
use herdr_client::{Method, ResponseResult};
use herdr_status_core::config::Config;
use herdr_status_core::hub::{specs_from_config, Hub, ServerSpec, SourceFactory};
use herdr_status_core::model::{AgentList, ServerKind};
use herdr_status_core::paths::Platform;
use herdr_status_core::transport_source::{FixedSource, TransportSource};
use herdr_status_core::watcher::WatcherConfig;

fn fast() -> WatcherConfig {
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

/// Maps server ids to fake servers; unknown ids get a transport to a missing socket.
fn factory(map: HashMap<String, Arc<FakeHerdrServer>>) -> SourceFactory {
    let map = Arc::new(Mutex::new(map));
    Arc::new(move |spec: &ServerSpec| -> Arc<dyn TransportSource> {
        let guard = map.lock().unwrap();
        match guard.get(spec.id()) {
            Some(server) => Arc::new(FixedSource(Arc::new(server.transport()))),
            None => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("absent.sock");
                std::mem::forget(dir);
                Arc::new(FixedSource(Arc::new(
                    herdr_client::transport::unix::UnixSocketTransport::new(path),
                )))
            }
        }
    })
}

async fn wait_list(
    rx: &mut tokio::sync::watch::Receiver<AgentList>,
    pred: impl Fn(&AgentList) -> bool,
) -> AgentList {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if pred(&rx.borrow()) {
                return rx.borrow().clone();
            }
            rx.changed().await.expect("hub alive");
        }
    })
    .await
    .expect("expected list within 3s")
}

#[tokio::test]
async fn apply_config_starts_local_and_publishes_rows() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(
        factory(HashMap::from([("local".to_string(), local.clone())])),
        fast(),
    );
    let mut rx = hub.subscribe();
    hub.apply_config(&Config::parse("").unwrap(), Platform::Mac)
        .await;
    let list = wait_list(&mut rx, |l| l.rows.len() == 2).await;
    assert_eq!(list.servers.len(), 1);
    assert_eq!(list.servers[0].id, "local");
    assert_eq!(list.servers[0].kind, ServerKind::Local);
    assert_eq!(list.counts.working, 1);
    hub.shutdown().await;
}

#[tokio::test]
async fn config_changes_add_and_remove_servers() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let remote = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(
        factory(HashMap::from([
            ("local".to_string(), local.clone()),
            ("box".to_string(), remote.clone()),
        ])),
        fast(),
    );
    let mut rx = hub.subscribe();
    let with_remote = Config::parse("[[remote]]\nlabel = \"box\"\nssh_target = \"box\"\n").unwrap();
    hub.apply_config(&with_remote, Platform::Mac).await;
    let list = wait_list(&mut rx, |l| l.servers.len() == 2 && l.rows.len() == 4).await;
    assert_eq!(list.servers[1].id, "box");
    assert_eq!(list.servers[1].kind, ServerKind::Remote);
    assert!(list.rows.iter().any(|r| r.server_id == "box"));

    hub.apply_config(&Config::parse("").unwrap(), Platform::Mac)
        .await;
    let list = wait_list(&mut rx, |l| l.servers.len() == 1).await;
    assert!(list.rows.iter().all(|r| r.server_id == "local"));
    assert_eq!(list.rows.len(), 2);
    hub.shutdown().await;
}

#[tokio::test]
async fn unchanged_servers_are_not_restarted() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(
        factory(HashMap::from([("local".to_string(), local.clone())])),
        fast(),
    );
    let mut rx = hub.subscribe();
    let cfg = Config::parse("").unwrap();
    hub.apply_config(&cfg, Platform::Mac).await;
    wait_list(&mut rx, |l| l.rows.len() == 2).await;
    let before = local.requests().len();
    hub.apply_config(&cfg, Platform::Mac).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        local.requests().len(),
        before,
        "same config must not reconnect"
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn reconnect_all_and_client_for() {
    let local = Arc::new(FakeHerdrServer::start().await);
    let mut hub = Hub::new(
        factory(HashMap::from([("local".to_string(), local.clone())])),
        fast(),
    );
    let mut rx = hub.subscribe();
    hub.apply_config(&Config::parse("").unwrap(), Platform::Mac)
        .await;
    wait_list(&mut rx, |l| l.rows.len() == 2).await;
    hub.reconnect_all();
    tokio::time::timeout(Duration::from_secs(3), async {
        while local
            .requests()
            .iter()
            .filter(|r| r["method"] == "events.subscribe")
            .count()
            < 2
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("resubscribed");
    let client = hub.client_for("local").await.expect("client for local");
    assert!(matches!(
        client.request(Method::Ping).await.unwrap(),
        ResponseResult::Pong { .. }
    ));
    assert!(hub.client_for("nope").await.is_none());
    hub.shutdown().await;
}

#[test]
fn specs_follow_platform_and_enabled_flag() {
    let cfg = Config::parse(
        "[[wsl]]\nlabel = \"u\"\n[[remote]]\nlabel = \"box\"\nssh_target = \"box\"\n",
    )
    .unwrap();
    let mac: Vec<String> = specs_from_config(&cfg, Platform::Mac)
        .iter()
        .map(|s| s.id().to_string())
        .collect();
    assert_eq!(
        mac,
        vec!["local", "box"],
        "wsl entries are ignored off Windows"
    );
    let win: Vec<String> = specs_from_config(&cfg, Platform::Windows)
        .iter()
        .map(|s| s.id().to_string())
        .collect();
    assert_eq!(win, vec!["local", "u", "box"]);
    let off = Config::parse("[local]\nenabled = false\n").unwrap();
    assert!(specs_from_config(&off, Platform::Mac).is_empty());
}
