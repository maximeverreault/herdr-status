//! Windows named pipe transport (local native herdr).

use std::io::ErrorKind;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::net::windows::named_pipe::ClientOptions;
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

use super::pipe_name::pipe_name_for;
use super::{Connection, StreamConnection, Transport, TransportError};

#[derive(Debug, Clone)]
pub struct NamedPipeTransport {
    pipe_name: String,
}

impl NamedPipeTransport {
    pub fn new(pipe_name: impl Into<String>) -> Self {
        Self {
            pipe_name: pipe_name.into(),
        }
    }

    /// Build from herdr's socket path string (`%APPDATA%\herdr\herdr.sock`).
    pub fn for_socket_path(socket_path: &str) -> Self {
        Self::new(pipe_name_for(socket_path))
    }
}

#[async_trait]
impl Transport for NamedPipeTransport {
    async fn connect(&self) -> Result<Box<dyn Connection>, TransportError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match ClientOptions::new().open(&self.pipe_name) {
                Ok(client) => {
                    let (r, w) = tokio::io::split(client);
                    return Ok(Box::new(StreamConnection::new(r, w)));
                }
                // The pipe exists but the server is between accepts: retry briefly.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    if Instant::now() >= deadline {
                        return Err(TransportError::Timeout(format!(
                            "{}: pipe busy for 2 s",
                            self.pipe_name
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    return Err(TransportError::NotRunning(format!(
                        "{}: {e}",
                        self.pipe_name
                    )));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn describe(&self) -> String {
        format!("named pipe {}", self.pipe_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    #[tokio::test]
    async fn connects_writes_and_reads_a_line() {
        let name = format!(r"\\.\pipe\herdr-client-test-{}", std::process::id());
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        tokio::spawn(async move {
            server.connect().await.unwrap();
            let (r, mut w) = tokio::io::split(server);
            let mut line = String::new();
            BufReader::new(r).read_line(&mut line).await.unwrap();
            w.write_all(format!("echo:{}\n", line.trim_end()).as_bytes())
                .await
                .unwrap();
        });
        let transport = NamedPipeTransport::new(&name);
        let mut conn = transport.connect().await.unwrap();
        conn.write_line("hello").await.unwrap();
        assert_eq!(
            conn.read_line().await.unwrap().as_deref(),
            Some("echo:hello")
        );
    }

    #[tokio::test]
    async fn missing_pipe_is_not_running() {
        let transport = NamedPipeTransport::new(r"\\.\pipe\herdr-client-test-absent-pipe");
        assert!(matches!(
            transport.connect().await,
            Err(TransportError::NotRunning(_))
        ));
    }
}
