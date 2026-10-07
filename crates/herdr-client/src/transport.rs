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
pub mod pipe_name;
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
    ///
    /// Cancellation-safe: if the future is dropped mid-line, the bytes read so
    /// far are kept and the next call continues the same line.
    /// Invalid UTF-8 is replaced lossily rather than failing the stream.
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
    /// Partial line carried across cancelled `read_line` calls.
    buf: Vec<u8>,
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
            buf: Vec::new(),
        }
    }

    /// Wrap a reader that is already buffered (the bridge consumes leading
    /// lines up to the ready marker and must not lose what is buffered).
    pub fn from_buffered(reader: BufReader<R>, writer: W) -> Self {
        Self {
            reader,
            writer,
            buf: Vec::new(),
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
        self.reader.read_until(b'\n', &mut self.buf).await?;
        if self.buf.is_empty() {
            return Ok(None);
        }
        let mut buf = std::mem::take(&mut self.buf);
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(input: &'static [u8]) -> StreamConnection<&'static [u8], Vec<u8>> {
        StreamConnection::new(input, Vec::new())
    }

    #[tokio::test]
    async fn final_line_without_newline_then_eof() {
        let mut c = conn(b"one\ntwo");
        assert_eq!(c.read_line().await.unwrap().as_deref(), Some("one"));
        assert_eq!(c.read_line().await.unwrap().as_deref(), Some("two"));
        assert_eq!(c.read_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn crlf_is_stripped() {
        let mut c = conn(b"one\r\n");
        assert_eq!(c.read_line().await.unwrap().as_deref(), Some("one"));
    }

    #[tokio::test]
    async fn invalid_utf8_is_decoded_lossily() {
        let mut c = conn(b"a\xffb\nnext\n");
        assert_eq!(c.read_line().await.unwrap().as_deref(), Some("a\u{fffd}b"));
        assert_eq!(c.read_line().await.unwrap().as_deref(), Some("next"));
    }

    #[tokio::test]
    async fn write_line_appends_newline_only_when_missing() {
        let mut c = conn(b"");
        c.write_line("a").await.unwrap();
        c.write_line("b\n").await.unwrap();
        assert_eq!(c.writer, b"a\nb\n");
    }

    #[tokio::test]
    async fn read_line_is_cancel_safe() {
        use std::time::Duration;
        let (client, mut server) = tokio::io::duplex(64);
        let (r, w) = tokio::io::split(client);
        let mut c = StreamConnection::new(r, w);
        server.write_all(b"hel").await.unwrap();
        let timed_out = tokio::time::timeout(Duration::from_millis(50), c.read_line()).await;
        assert!(timed_out.is_err(), "no full line yet");
        server.write_all(b"lo\n").await.unwrap();
        assert_eq!(c.read_line().await.unwrap().as_deref(), Some("hello"));
    }
}
