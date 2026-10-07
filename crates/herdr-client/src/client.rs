//! High-level client: one request per connection, plus event streams.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::protocol::{encode_request, parse_line, ErrorBody, Method, ResponseResult, ServerLine};
use crate::snapshot::SessionSnapshot;
use crate::transport::{Transport, TransportError};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// herdr answered with an error object.
    #[error("herdr error {}: {}", .0.code, .0.message)]
    Api(ErrorBody),
    /// The server broke the protocol (closed early, unparseable line, wrong id).
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl ClientError {
    pub fn api_code(&self) -> Option<&str> {
        match self {
            ClientError::Api(body) => Some(body.code.as_str()),
            _ => None,
        }
    }
}

pub struct Client {
    transport: Arc<dyn Transport>,
    next_id: AtomicU64,
}

impl Client {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    fn next_id(&self) -> String {
        format!("hs-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Open a connection, send one request, read one response, drop the connection.
    pub async fn request(&self, method: Method) -> Result<ResponseResult, ClientError> {
        let id = self.next_id();
        let mut conn = self.transport.connect().await?;
        conn.write_line(&encode_request(&id, &method)).await?;
        let line = conn.read_line().await?.ok_or_else(|| {
            ClientError::Protocol(format!(
                "connection closed before response to {}",
                method.name()
            ))
        })?;
        match parse_line(&line)
            .map_err(|e| ClientError::Protocol(format!("unparseable response: {e}: {line}")))?
        {
            ServerLine::Response { id: got, result } if got == id => Ok(result),
            ServerLine::Error { id: got, error } if got == id => Err(ClientError::Api(error)),
            ServerLine::Response { id: got, .. } | ServerLine::Error { id: got, .. } => Err(
                ClientError::Protocol(format!("response id {got} does not match request id {id}")),
            ),
            ServerLine::Event(_) => Err(ClientError::Protocol(
                "got an event while waiting for a response".into(),
            )),
        }
    }

    /// `session.snapshot`, typed.
    pub async fn snapshot(&self) -> Result<SessionSnapshot, ClientError> {
        match self.request(Method::SessionSnapshot).await? {
            ResponseResult::SessionSnapshot { snapshot } => Ok(snapshot),
            other => Err(ClientError::Protocol(format!(
                "expected session_snapshot, got {other:?}"
            ))),
        }
    }
}
