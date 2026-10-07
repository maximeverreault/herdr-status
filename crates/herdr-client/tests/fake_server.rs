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
    let lines = raw_exchange(
        &server,
        r#"{"id":"s","method":"session.snapshot","params":{}}"#,
    )
    .await;
    assert!(lines[0].contains(r#""type":"session_snapshot""#));
    assert!(lines[0].contains("w2:p1"));
}

#[tokio::test]
async fn custom_responder_and_error_reply() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _params| match method {
        "agent.read" => Reply::Error {
            code: "not_found".into(),
            message: "no such agent".into(),
        },
        _ => Reply::Result(serde_json::json!({"type":"ok"})),
    });
    let lines = raw_exchange(
        &server,
        r#"{"id":"r","method":"agent.read","params":{"target":"x","source":"recent_unwrapped"}}"#,
    )
    .await;
    assert_eq!(
        lines,
        vec![r#"{"id":"r","error":{"code":"not_found","message":"no such agent"}}"#.to_string()]
    );
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
    assert_eq!(
        reader.read_line(&mut end).await.unwrap(),
        0,
        "closed by server"
    );
}
