//! Child-process bridge transport.
//!
//! Spawns a command whose stdin/stdout carry herdr's JSON stream, for example
//! `ssh host /bin/sh -c '… exec herdr --session default remote-api-bridge'`,
//! `wsl.exe -- sh -lc '…'`, or a local `herdr.exe --session default remote-api-bridge`.
//! Stdout lines before [`READY_MARKER`] are discarded so SSH banners never
//! reach the JSON parser.

use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::{Connection, StreamConnection, Transport, TransportError};

/// Printed by the remote script right before the JSON stream starts. Same
/// string herdr's own saved-machine bridge uses.
pub const READY_MARKER: &str = "herdr-remote-output-ready:1";

/// Keep the last this many bytes of the child's stderr.
const STDERR_CAP: usize = 4096;
/// Longest line tolerated before the ready marker.
const MAX_PRE_MARKER_LINE: usize = 64 * 1024;

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
    ready_marker: Option<&'static str>,
}

impl BridgeTransport {
    pub fn new(command: ChildCommand, description: impl Into<String>) -> Self {
        Self {
            command,
            description: description.into(),
            ready_timeout: Duration::from_secs(20),
            ready_marker: Some(READY_MARKER),
        }
    }

    /// Do not wait for [`READY_MARKER`]: the JSON stream starts immediately.
    ///
    /// Use this when the command is `herdr … remote-api-bridge` run directly
    /// (for example a local `herdr.exe`): herdr itself never prints the marker,
    /// only the wrapping sh or PowerShell scripts do.
    pub fn without_ready_marker(mut self) -> Self {
        self.ready_marker = None;
        self
    }

    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    pub fn command(&self) -> &ChildCommand {
        &self.command
    }
}

/// Keeps the child alive for as long as the connection is used. Dropping it
/// kills the child (`kill_on_drop`); tokio's orphan queue reaps it best-effort.
struct BridgeConnection {
    // Declared first so stdin closes (fields drop in order) before the kill.
    inner: StreamConnection<ChildStdout, ChildStdin>,
    _child: Child,
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
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));
        let collector = tokio::spawn(collect_stderr(stderr, Arc::clone(&stderr_buf)));

        let mut reader = BufReader::new(stdout);
        let ready = match self.ready_marker {
            Some(marker) => {
                tokio::time::timeout(self.ready_timeout, skip_to_marker(&mut reader, marker)).await
            }
            None => Ok(Ok(true)),
        };
        match ready {
            Err(_elapsed) => Err(TransportError::Timeout(format!(
                "{}: no ready marker within {:?}",
                self.description, self.ready_timeout
            ))),
            Ok(Err(SkipError::Io(e))) => Err(e.into()),
            Ok(Err(SkipError::TooLong)) => Err(TransportError::Launch(format!(
                "{}: bridge output too long before ready marker",
                self.description
            ))),
            Ok(Ok(false)) => {
                // Close stdin so a child blocked reading it sees EOF and exits.
                drop(stdin);
                let grace = Duration::from_millis(500);
                let status = tokio::time::timeout(grace, child.wait())
                    .await
                    .ok()
                    .and_then(Result::ok);
                // Let the collector drain what the child wrote before exiting.
                let _ = tokio::time::timeout(grace, collector).await;
                let stderr_text = {
                    let bytes = stderr_buf.lock().unwrap_or_else(PoisonError::into_inner);
                    String::from_utf8_lossy(&bytes).trim().to_string()
                };
                let how = match status {
                    Some(st) => match st.code() {
                        Some(code) => format!("exited with status {code}"),
                        None => format!("exited ({st})"),
                    },
                    None => "closed its output".to_string(),
                };
                Err(TransportError::Launch(format!(
                    "{}: {how} before ready marker: {stderr_text}",
                    self.description
                )))
            }
            Ok(Ok(true)) => Ok(Box::new(BridgeConnection {
                inner: StreamConnection::from_buffered(reader, stdin),
                _child: child,
            })),
        }
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

enum SkipError {
    Io(std::io::Error),
    TooLong,
}

/// Returns `Ok(true)` once the marker line was consumed, `Ok(false)` at EOF.
/// Works on bytes so non-UTF-8 banner text is simply discarded.
async fn skip_to_marker(
    reader: &mut BufReader<ChildStdout>,
    marker: &str,
) -> Result<bool, SkipError> {
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = (&mut *reader)
            .take(MAX_PRE_MARKER_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(SkipError::Io)?;
        if n == 0 {
            return Ok(false);
        }
        if line.trim_ascii_end() == marker.as_bytes() {
            return Ok(true);
        }
        if line.last() != Some(&b'\n') {
            if line.len() > MAX_PRE_MARKER_LINE {
                return Err(SkipError::TooLong);
            }
            return Ok(false); // EOF in the middle of a line
        }
        tracing::trace!(line = %String::from_utf8_lossy(line.trim_ascii_end()), "bridge output before ready marker");
    }
}

/// Collects the last [`STDERR_CAP`] bytes of stderr (ssh's decisive line is
/// usually the last one).
async fn collect_stderr(mut stderr: ChildStderr, buf: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut bytes = buf.lock().unwrap_or_else(PoisonError::into_inner);
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.len() > STDERR_CAP {
                    let excess = bytes.len() - STDERR_CAP;
                    bytes.drain(..excess);
                }
            }
        }
    }
    let bytes = buf.lock().unwrap_or_else(PoisonError::into_inner);
    if !bytes.is_empty() {
        tracing::debug!(stderr = %String::from_utf8_lossy(&bytes).trim(), "bridge stderr");
    }
}
