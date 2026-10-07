//! Transports produce one-shot connections to a herdr server.
//!
//! herdr closes a connection after one normal response and keeps it open only
//! for `events.subscribe`, so a [`Transport`] hands out a fresh [`Connection`]
//! per request and the caller drops it when done.

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub mod bridge;
#[cfg(windows)]
pub mod named_pipe;
#[cfg(unix)]
pub mod unix;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The server is not reachable at all (socket missing, connection refused).
    #[error("herdr is not running ({0})")]
    NotRunning(String),
    /// A child-process bridge failed to start or never printed its ready marker.
    #[error("bridge failed: {0}")]
    Launch(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// One open connection: write JSON lines, read JSON lines.
#[async_trait]
pub trait Connection: Send {
    async fn write_line(&mut self, line: &str) -> Result<(), TransportError>;
    /// The next line without its trailing newline, or `None` at EOF.
    async fn read_line(&mut self) -> Result<Option<String>, TransportError>;
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError>;
    /// Human-readable description for logs and tooltips, e.g. `unix socket /x/herdr.sock`.
    fn describe(&self) -> String;
}

/// A [`Connection`] over any async reader/writer pair.
pub struct StreamConnection<R, W> {
    reader: BufReader<R>,
    writer: W,
}

impl<R, W> StreamConnection<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }
}

#[async_trait]
impl<R, W> Connection for StreamConnection<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    async fn write_line(&mut self, line: &str) -> Result<(), TransportError> {
        self.writer.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            self.writer.write_all(b"\n").await?;
        }
        self.writer.flush().await?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<Option<String>, TransportError> {
        let mut buf = String::new();
        let n = self.reader.read_line(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
        while buf.ends_with('\n') || buf.ends_with('\r') {
            buf.pop();
        }
        Ok(Some(buf))
    }
}
