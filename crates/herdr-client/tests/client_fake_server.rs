#![cfg(all(unix, feature = "test-support"))]

use std::sync::Arc;

use herdr_client::client::{Client, ClientError};
use herdr_client::protocol::{AgentStatus, Event, Method, ResponseResult, Subscription};
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
async fn connection_closed_before_response_is_closed_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::CloseSilently);
    let err = client_for(&server).request(Method::Ping).await.unwrap_err();
    assert!(matches!(&err, ClientError::Closed(_)), "{err:?}");
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

#[tokio::test]
async fn subscribe_acks_then_streams_events_until_eof() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    let mut stream = client
        .subscribe(vec![
            Subscription::lifecycle("pane.closed"),
            Subscription::pane_agent_status("w2:p1"),
        ])
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
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        Event::Other
    ));
    assert!(
        matches!(stream.next().await.unwrap().unwrap(), Event::PaneClosed { pane_id, .. } if pane_id == "w1:p2")
    );

    server.close_subscriptions();
    assert!(
        stream.next().await.unwrap().is_none(),
        "EOF after server closes"
    );
}

#[tokio::test]
async fn rejected_subscription_is_api_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::Error {
        code: "invalid_request".into(),
        message: "missing field `pane_id`".into(),
    });
    let err = client_for(&server)
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap_err();
    assert_eq!(err.api_code(), Some("invalid_request"));
}

#[tokio::test]
async fn events_lost_surfaces_as_api_error_from_next() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    let mut stream = client
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap();
    server.wait_for_subscribers(1).await;
    server.push_line(
        r#"{"id":"hs-1","error":{"code":"events_lost","message":"subscriber fell behind"}}"#,
    );
    let err = stream.next().await.unwrap_err();
    assert_eq!(err.api_code(), Some("events_lost"));
}

#[tokio::test]
async fn malformed_event_line_is_skipped_not_fatal() {
    let server = FakeHerdrServer::start().await;
    let client = client_for(&server);
    let mut stream = client
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap();
    server.wait_for_subscribers(1).await;
    server.push_line("garbage line");
    server.push_json(serde_json::json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}));
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        Event::PaneClosed { .. }
    ));
}

#[tokio::test]
async fn error_line_without_id_is_api_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| {
        Reply::RawLine(r#"{"error":{"code":"invalid_request","message":"bad"}}"#.into())
    });
    let err = client_for(&server).request(Method::Ping).await.unwrap_err();
    assert_eq!(err.api_code(), Some("invalid_request"));
}

#[tokio::test]
async fn events_lost_without_id_surfaces_from_next() {
    let server = FakeHerdrServer::start().await;
    let mut stream = client_for(&server)
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap();
    server.wait_for_subscribers(1).await;
    server.push_line(r#"{"error":{"code":"events_lost","message":"subscriber fell behind"}}"#);
    let err = stream.next().await.unwrap_err();
    assert_eq!(err.api_code(), Some("events_lost"));
}

#[tokio::test]
async fn subscription_ack_with_wrong_id_is_protocol_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| {
        Reply::RawLine(r#"{"id":"other","result":{"type":"subscription_started"}}"#.into())
    });
    let err = client_for(&server)
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap_err();
    assert!(matches!(&err, ClientError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn stray_response_on_stream_is_skipped() {
    let server = FakeHerdrServer::start().await;
    let mut stream = client_for(&server)
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap();
    server.wait_for_subscribers(1).await;
    server.push_line(r#"{"id":"x","result":{"type":"ok"}}"#);
    server.push_json(serde_json::json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p2","workspace_id":"w1"}}));
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        Event::PaneClosed { .. }
    ));
}

#[tokio::test]
async fn subscribe_on_missing_socket_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::new(Arc::new(UnixSocketTransport::new(
        dir.path().join("none.sock"),
    )));
    let err = client
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            ClientError::Transport(herdr_client::transport::TransportError::NotRunning(_))
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn subscribe_closed_silently_is_closed_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| Reply::CloseSilently);
    let err = client_for(&server)
        .subscribe(vec![Subscription::lifecycle("pane.closed")])
        .await
        .unwrap_err();
    assert!(matches!(&err, ClientError::Closed(_)), "{err:?}");
}

#[tokio::test]
async fn snapshot_with_wrong_result_type_is_protocol_error() {
    let server = FakeHerdrServer::start().await;
    server.set_responder(|_, _| {
        Reply::Result(serde_json::json!({"type":"pong","version":"0.9.3","protocol":22}))
    });
    let err = client_for(&server).snapshot().await.unwrap_err();
    assert!(matches!(&err, ClientError::Protocol(_)), "{err:?}");
}
