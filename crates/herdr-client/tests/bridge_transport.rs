#![cfg(unix)]

use std::time::Duration;

use herdr_client::transport::bridge::{BridgeTransport, ChildCommand, READY_MARKER};
use herdr_client::transport::{Transport, TransportError};

fn sh(script: &str) -> ChildCommand {
    ChildCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
    }
}

#[tokio::test]
async fn skips_banner_then_bridges_lines_both_ways() {
    let script = format!(
        r#"echo "Welcome to host"; echo; printf '\n%s\n' '{READY_MARKER}'; while IFS= read -r l; do echo "echo:$l"; done"#
    );
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    conn.write_line(r#"{"id":"1","method":"ping","params":{}}"#)
        .await
        .unwrap();
    assert_eq!(
        conn.read_line().await.unwrap().as_deref(),
        Some(r#"echo:{"id":"1","method":"ping","params":{}}"#)
    );
    conn.write_line("second").await.unwrap();
    assert_eq!(
        conn.read_line().await.unwrap().as_deref(),
        Some("echo:second")
    );
}

#[tokio::test]
async fn eof_after_marker_reads_none() {
    let script = format!(r#"printf '\n%s\n' '{READY_MARKER}'; exit 0"#);
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    assert_eq!(conn.read_line().await.unwrap(), None);
}

#[tokio::test]
async fn child_exit_before_marker_is_launch_error_with_stderr() {
    let transport = BridgeTransport::new(
        sh(r#"echo "herdr: command not found" >&2; exit 127"#),
        "test bridge",
    );
    match transport.connect().await {
        Err(TransportError::Launch(msg)) => assert!(msg.contains("command not found"), "{msg}"),
        other => panic!("expected Launch error, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn no_marker_within_timeout_is_timeout_error() {
    let transport = BridgeTransport::new(
        sh(r#"echo "still negotiating"; exec sleep 5"#),
        "test bridge",
    )
    .with_ready_timeout(Duration::from_millis(300));
    assert!(matches!(
        transport.connect().await,
        Err(TransportError::Timeout(_))
    ));
}

#[tokio::test]
async fn missing_program_is_launch_error() {
    let command = ChildCommand {
        program: "definitely-not-a-program-xyz".into(),
        args: vec![],
    };
    let transport = BridgeTransport::new(command, "nope");
    assert!(matches!(
        transport.connect().await,
        Err(TransportError::Launch(_))
    ));
    assert_eq!(transport.describe(), "nope");
}

#[tokio::test]
async fn non_utf8_banner_before_marker_is_skipped() {
    let script = format!(
        r#"printf '\377\376 banner\n'; printf '\n%s\n' '{READY_MARKER}'; while IFS= read -r l; do echo "echo:$l"; done"#
    );
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    conn.write_line("hi").await.unwrap();
    assert_eq!(conn.read_line().await.unwrap().as_deref(), Some("echo:hi"));
}

#[tokio::test]
async fn bytes_after_marker_in_same_write_are_kept() {
    let script = format!(
        r#"printf '\n%s\n{{"id":"1","result":{{"type":"ok"}}}}\n' '{READY_MARKER}'; exec sleep 1"#
    );
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    assert_eq!(
        conn.read_line().await.unwrap().as_deref(),
        Some(r#"{"id":"1","result":{"type":"ok"}}"#)
    );
}

#[tokio::test]
async fn crlf_marker_is_accepted() {
    let script = format!(r#"printf '%s\r\n' '{READY_MARKER}'; echo after"#);
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    assert_eq!(conn.read_line().await.unwrap().as_deref(), Some("after"));
}

#[tokio::test]
async fn text_before_marker_on_same_line_does_not_match() {
    let script = format!(r#"printf 'MOTD %s\n' '{READY_MARKER}'; exec sleep 1"#);
    let transport = BridgeTransport::new(sh(&script), "test bridge")
        .with_ready_timeout(Duration::from_millis(300));
    assert!(matches!(
        transport.connect().await,
        Err(TransportError::Timeout(_))
    ));
}

#[tokio::test]
async fn write_after_child_exit_is_io_error() {
    let script = format!(r#"printf '\n%s\n' '{READY_MARKER}'; exit 0"#);
    let transport = BridgeTransport::new(sh(&script), "test bridge");
    let mut conn = transport.connect().await.unwrap();
    for _ in 0..50 {
        match conn.write_line("x").await {
            Err(TransportError::Io(_)) => return,
            Err(other) => panic!("unexpected error: {other}"),
            Ok(()) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    panic!("write never failed after child exit");
}

#[tokio::test]
async fn over_long_banner_without_newline_is_launch_error() {
    let transport = BridgeTransport::new(
        sh(r#"head -c 70000 /dev/zero | tr '\0' a; exec sleep 1"#),
        "test bridge",
    )
    .with_ready_timeout(Duration::from_secs(2));
    match transport.connect().await {
        Err(TransportError::Launch(msg)) => assert!(msg.contains("too long"), "{msg}"),
        other => panic!("expected Launch error, got {:?}", other.map(|_| ())),
    }
}
