//! Child-process bridge transport.
//!
//! Spawns a command whose stdin/stdout carry herdr's JSON stream, for example
//! `ssh host /bin/sh -c '… exec herdr --session default remote-api-bridge'`,
//! `wsl.exe -- sh -lc '…'`, or a local `herdr.exe --session default remote-api-bridge`.
//! Stdout lines before [`READY_MARKER`] are discarded so SSH banners never
//! reach the JSON parser.

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::{Connection, StreamConnection, Transport, TransportError};

/// Printed by the remote script right before the JSON stream starts. Same
/// string herdr's own saved-machine bridge uses.
pub const READY_MARKER: &str = "herdr-remote-output-ready:1";

const STDERR_CAP: usize = 4096;

/// A program plus arguments, run without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BridgeTransport {
    command: ChildCommand,
    description: String,
    ready_timeout: Duration,
}

impl BridgeTransport {
    pub fn new(command: ChildCommand, description: impl Into<String>) -> Self {
        Self {
            command,
            description: description.into(),
            ready_timeout: Duration::from_secs(20),
        }
    }

    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    pub fn command(&self) -> &ChildCommand {
        &self.command
    }
}

/// Keeps the child alive for as long as the connection is used; `kill_on_drop`
/// reaps it afterwards.
struct BridgeConnection {
    _child: Child,
    inner: StreamConnection<ChildStdout, ChildStdin>,
}

#[async_trait]
impl Connection for BridgeConnection {
    async fn write_line(&mut self, line: &str) -> Result<(), TransportError> {
        self.inner.write_line(line).await
    }

    async fn read_line(&mut self) -> Result<Option<String>, TransportError> {
        self.inner.read_line().await
    }
}

#[async_trait]
impl Transport for BridgeTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        let mut cmd = Command::new(&self.command.program);
        cmd.args(&self.command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| {
            TransportError::Launch(format!(
                "{}: cannot start {}: {e}",
                self.description, self.command.program
            ))
        })?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        tokio::spawn(collect_stderr(stderr, Arc::clone(&stderr_buf)));

        let mut reader = BufReader::new(stdout);
        match tokio::time::timeout(self.ready_timeout, skip_to_marker(&mut reader)).await {
            Err(_elapsed) => Err(TransportError::Timeout(format!(
                "{}: no ready marker within {:?}",
                self.description, self.ready_timeout
            ))),
            Ok(Err(e)) => Err(e.into()),
            Ok(Ok(false)) => {
                // Give the stderr collector a moment to drain what the child wrote before exiting.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stderr_text = stderr_buf.lock().unwrap().trim().to_string();
                Err(TransportError::Launch(format!(
                    "{}: exited before ready marker: {stderr_text}",
                    self.description
                )))
            }
            Ok(Ok(true)) => Ok(Box::new(BridgeConnection {
                _child: child,
                inner: StreamConnection::from_buffered(reader, stdin),
            })),
        }
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

/// Returns `Ok(true)` once the marker line was consumed, `Ok(false)` at EOF.
async fn skip_to_marker(reader: &mut BufReader<ChildStdout>) -> std::io::Result<bool> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(false);
        }
        if line.trim_end() == READY_MARKER {
            return Ok(true);
        }
        tracing::debug!(line = line.trim_end(), "bridge output before ready marker");
    }
}

async fn collect_stderr(mut stderr: ChildStderr, buf: Arc<Mutex<String>>) {
    let mut chunk = [0u8; 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut text = buf.lock().unwrap();
                if text.len() < STDERR_CAP {
                    text.push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
        }
    }
}
