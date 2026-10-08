//! Talks to the real local herdr. Opt in with `HERDR_STATUS_LIVE=1`.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use herdr_client::transport::unix::UnixSocketTransport;
use herdr_client::{Client, Method, ResponseResult, Subscription};

fn live_socket() -> Option<std::path::PathBuf> {
    if std::env::var("HERDR_STATUS_LIVE").ok().as_deref() != Some("1") {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".config/herdr/herdr.sock");
    path.exists().then_some(path)
}

#[tokio::test]
async fn ping_snapshot_and_short_subscription() {
    let Some(path) = live_socket() else {
        eprintln!("skipped: set HERDR_STATUS_LIVE=1 with herdr running");
        return;
    };
    let client = Client::new(Arc::new(UnixSocketTransport::new(path)));
    match client.request(Method::Ping).await.unwrap() {
        ResponseResult::Pong { protocol, .. } => assert!(protocol >= 22),
        other => panic!("unexpected {other:?}"),
    }
    let snapshot = client.snapshot().await.unwrap();
    assert!(!snapshot.workspaces.is_empty());
    let ids: Vec<String> = snapshot.pane_ids().iter().map(|s| s.to_string()).collect();
    let subs = [Subscription::lifecycle("pane.created")]
        .into_iter()
        .chain(ids.iter().map(|p| Subscription::pane_agent_status(p)))
        .collect();
    let mut stream = client.subscribe(subs).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.next()).await;
}
