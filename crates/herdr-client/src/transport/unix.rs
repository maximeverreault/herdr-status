//! Unix domain socket transport (macOS, Linux).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::net::UnixStream;

use super::{Connection, StreamConnection, Transport, TransportError};

#[derive(Debug, Clone)]
pub struct UnixSocketTransport {
    path: PathBuf,
}

impl UnixSocketTransport {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl Transport for UnixSocketTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        match UnixStream::connect(&self.path).await {
            Ok(stream) => {
                let (r, w) = stream.into_split();
                Ok(Box::new(StreamConnection::new(r, w)))
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                Err(TransportError::NotRunning(format!(
                    "{}: {e}",
                    self.path.display()
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn describe(&self) -> String {
        format!("unix socket {}", self.path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Transport, TransportError};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    /// Accepts one connection, echoes one line back prefixed with `echo:`, then closes.
    async fn echo_once(listener: UnixListener) {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut line = String::new();
        BufReader::new(r).read_line(&mut line).await.unwrap();
        w.write_all(format!("echo:{}", line.trim_end()).as_bytes())
            .await
            .unwrap();
        w.write_all(b"\n").await.unwrap();
    }

    #[tokio::test]
    async fn connects_writes_and_reads_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(echo_once(listener));

        let transport = UnixSocketTransport::new(&path);
        let mut conn = transport.connect().await.unwrap();
        conn.write_line("hello").await.unwrap();
        assert_eq!(
            conn.read_line().await.unwrap().as_deref(),
            Some("echo:hello")
        );
        assert_eq!(
            conn.read_line().await.unwrap(),
            None,
            "server closed after one line"
        );
    }

    #[tokio::test]
    async fn missing_socket_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let transport = UnixSocketTransport::new(dir.path().join("absent.sock"));
        match transport.connect().await {
            Err(TransportError::NotRunning(msg)) => assert!(msg.contains("absent.sock")),
            other => panic!("expected NotRunning, got {:?}", other.map(|_| ())),
        }
        assert!(transport.describe().contains("absent.sock"));
    }
}
