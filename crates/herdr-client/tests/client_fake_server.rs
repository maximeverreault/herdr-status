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
    assert_eq!(
        snapshot
            .agents
            .iter()
            .find(|a| a.pane_id == "w2:p1")
            .unwrap()
            .agent_status,
        AgentStatus::Working
    );
}

#[tokio::test]
async fn api_error_is_returned_as_client_error_api() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::Error {
        code: "agent_blocked".into(),
        message: "waiting at a prompt".into(),
    });
    let err = client_for(&server)
        .request(Method::AgentPrompt {
            target: "w2:p1".into(),
            text: "hi".into(),
        })
        .await
        .unwrap_err();
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
    assert!(
        matches!(&err, ClientError::Protocol(msg) if msg.contains("closed")),
        "{err:?}"
    );
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
    server.set_responder(|_, _| {
        Reply::RawLine(r#"{"id":"someone-else","result":{"type":"ok"}}"#.into())
    });
    let err = client_for(&server).request(Method::Ping).await.unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(msg) if msg.contains("id")),
        "{err:?}"
    );
}

#[tokio::test]
async fn not_running_transport_error_passes_through() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(Arc::new(UnixSocketTransport::new(
        dir.path().join("none.sock"),
    )));
    let err = client.request(Method::Ping).await.unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Transport(herdr_client::transport::TransportError::NotRunning(_))
        ),
        "{err:?}"
    );
}
