//! High-level client: one request per connection, plus event streams.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::protocol::{
    encode_request, parse_line, ErrorBody, Event, Method, ResponseResult, ServerLine, Subscription,
};
use crate::snapshot::SessionSnapshot;
use crate::transport::{Connection, Transport, TransportError};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// herdr answered with an error object.
    #[error("herdr error {}: {}", .0.code, .0.message)]
    Api(ErrorBody),
    /// The server closed the connection before answering.
    #[error("connection closed {0}")]
    Closed(String),
    /// The server broke the protocol (unparseable line, wrong id, unexpected line).
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl ClientError {
    /// The herdr error code when this is [`ClientError::Api`].
    pub fn api_code(&self) -> Option<&str> {
        match self {
            ClientError::Api(body) => Some(body.code.as_str()),
            _ => None,
        }
    }
}

/// Truncate text embedded in error messages to 200 chars.
fn excerpt(s: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(200).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Client for the herdr socket API over any [`Transport`].
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

    /// The transport this client connects through.
    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    fn next_id(&self) -> String {
        format!("hs-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Open a connection, send one request, read one response, drop the connection.
    ///
    /// No timeout is applied; wrap the call in `tokio::time::timeout` when the
    /// transport can stall (bridges spawn ssh).
    pub async fn request(&self, method: Method) -> Result<ResponseResult, ClientError> {
        let id = self.next_id();
        let mut conn = self.transport.connect().await?;
        conn.write_line(&encode_request(&id, &method)).await?;
        let line = conn
            .read_line()
            .await?
            .ok_or_else(|| ClientError::Closed(format!("before response to {}", method.name())))?;
        match parse_line(&line).map_err(|e| {
            ClientError::Protocol(format!("unparseable response: {e}: {}", excerpt(&line)))
        })? {
            ServerLine::Response { id: got, result } if got == id => Ok(result),
            // herdr can reject a request before it parsed the id, and the API
            // code matters more than the id.
            ServerLine::Error { id: got, error } if got == id || got.is_empty() => {
                Err(ClientError::Api(error))
            }
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
                "expected session_snapshot, got {}",
                excerpt(&format!("{other:?}"))
            ))),
        }
    }

    /// Open a subscription. The first line must be the `subscription_started`
    /// ack; an error line means herdr rejected the whole request. herdr may
    /// also close without an error line when a `pane_id` does not exist, which
    /// surfaces as [`ClientError::Closed`].
    ///
    /// No timeout is applied; wrap the call in `tokio::time::timeout` when the
    /// transport can stall (bridges spawn ssh).
    pub async fn subscribe(
        &self,
        subscriptions: Vec<Subscription>,
    ) -> Result<EventStream, ClientError> {
        let id = self.next_id();
        let mut conn = self.transport.connect().await?;
        conn.write_line(&encode_request(
            &id,
            &Method::EventsSubscribe(subscriptions),
        ))
        .await?;
        let line = conn
            .read_line()
            .await?
            .ok_or_else(|| ClientError::Closed("before subscription ack".into()))?;
        match parse_line(&line).map_err(|e| {
            ClientError::Protocol(format!(
                "unparseable subscription ack: {e}: {}",
                excerpt(&line)
            ))
        })? {
            ServerLine::Response {
                id: got,
                result: ResponseResult::SubscriptionStarted,
            } if got == id => Ok(EventStream { conn }),
            // herdr can reject a request before it parsed the id, and the API
            // code matters more than the id.
            ServerLine::Error { error, .. } => Err(ClientError::Api(error)),
            other => Err(ClientError::Protocol(format!(
                "expected subscription_started for {id}, got {}",
                excerpt(&format!("{other:?}"))
            ))),
        }
    }
}

/// An open `events.subscribe` connection.
pub struct EventStream {
    conn: Box<dyn Connection>,
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

impl EventStream {
    /// The next event. `Ok(None)` at EOF. An error line from the server (for
    /// example `events_lost`) is returned as `ClientError::Api`; after an
    /// `Err`, drop the stream and resubscribe. Unparseable lines and
    /// unexpected response lines are logged and skipped.
    ///
    /// Cancellation-safe: it may be polled inside `tokio::select!`; a partially
    /// read line is kept for the next call.
    pub async fn next(&mut self) -> Result<Option<Event>, ClientError> {
        loop {
            let Some(line) = self.conn.read_line().await? else {
                return Ok(None);
            };
            match parse_line(&line) {
                Ok(ServerLine::Event(event)) => return Ok(Some(event)),
                Ok(ServerLine::Error { error, .. }) => return Err(ClientError::Api(error)),
                Ok(ServerLine::Response { .. }) => {
                    tracing::warn!(line = %excerpt(&line), "unexpected response on event stream; ignoring");
                }
                Err(e) => {
                    tracing::warn!(error = %e, line = %excerpt(&line), "skipping unparseable event line");
                }
            }
        }
    }
}
