#![cfg(unix)]

use std::sync::Arc;

use herdr_client::test_support::{FakeHerdrServer, Reply};
use herdr_client::Client;
use herdr_status_core::actions::{peek, send_key, send_message, KEYPAD};

async fn client(server: &FakeHerdrServer) -> Client {
    Client::new(Arc::new(server.transport()))
}

#[tokio::test]
async fn peek_reads_recent_unwrapped_text() {
    let server = FakeHerdrServer::start().await;
    let read = peek(&client(&server).await, "w2:p1", 30, "local")
        .await
        .unwrap();
    assert_eq!(read.text, "$ echo hi\nhi");
    let req = &server.requests()[0];
    assert_eq!(req["method"], "agent.read");
    assert_eq!(req["params"]["target"], "w2:p1");
    assert_eq!(req["params"]["lines"], 30);
    assert_eq!(req["params"]["source"], "recent_unwrapped");
}

#[tokio::test]
async fn send_key_validates_against_keypad_before_calling() {
    let server = FakeHerdrServer::start().await;
    let c = client(&server).await;
    for key in KEYPAD {
        send_key(&c, "w2:p1", key, "local").await.unwrap();
    }
    assert_eq!(server.requests().len(), KEYPAD.len());
    assert_eq!(
        server.requests()[9]["params"]["keys"],
        serde_json::json!(["ctrl+c"])
    );
    let err = send_key(&c, "w2:p1", "rm -rf", "local").await.unwrap_err();
    assert_eq!(err.code, "invalid_key");
    assert_eq!(
        server.requests().len(),
        KEYPAD.len(),
        "no request for a rejected key"
    );
}

#[tokio::test]
async fn send_message_trims_and_rejects_empty() {
    let server = FakeHerdrServer::start().await;
    let c = client(&server).await;
    send_message(&c, "w2:p1", "  continue  ", "local")
        .await
        .unwrap();
    assert_eq!(server.requests()[0]["params"]["text"], "continue");
    let err = send_message(&c, "w2:p1", "   ", "local").await.unwrap_err();
    assert_eq!(err.code, "empty_message");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn herdr_errors_get_friendly_text() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|method, _| match method {
        "agent.prompt" => Reply::Error {
            code: "agent_blocked".into(),
            message: "waiting".into(),
        },
        "agent.read" => Reply::Error {
            code: "not_found".into(),
            message: "gone".into(),
        },
        _ => Reply::Error {
            code: "weird".into(),
            message: "something else".into(),
        },
    });
    let c = client(&server).await;
    let blocked = send_message(&c, "w2:p1", "hi", "local").await.unwrap_err();
    assert_eq!(blocked.code, "agent_blocked");
    assert_eq!(
        blocked.message,
        "Waiting at a prompt. Answer it with the keypad first."
    );
    let gone = peek(&c, "w2:p1", 10, "local").await.unwrap_err();
    assert_eq!(gone.message, "That agent is gone.");
    let other = send_key(&c, "w2:p1", "enter", "local").await.unwrap_err();
    assert_eq!(other.code, "weird");
    assert_eq!(other.message, "something else");
}

#[tokio::test]
async fn transport_failure_names_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let c = Client::new(Arc::new(
        herdr_client::transport::unix::UnixSocketTransport::new(dir.path().join("none.sock")),
    ));
    let err = peek(&c, "w2:p1", 10, "winbox").await.unwrap_err();
    assert_eq!(err.code, "transport");
    assert_eq!(err.message, "Could not reach winbox.");
}

#[tokio::test]
async fn overlong_message_is_rejected_before_calling() {
    let server = FakeHerdrServer::start().await;
    let c = client(&server).await;
    let err = send_message(&c, "w2:p1", &"x".repeat(16 * 1024 + 1), "local")
        .await
        .unwrap_err();
    assert_eq!(err.code, "message_too_long");
    assert!(server.requests().is_empty());
}
