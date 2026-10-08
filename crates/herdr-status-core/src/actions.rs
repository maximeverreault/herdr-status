//! The three quick actions, with the keypad allow-list and friendly errors.

use herdr_client::{Client, ClientError, Method, ReadResult, ResponseResult};
use serde::Serialize;

/// Keys the UI keypad may send, in display order.
pub const KEYPAD: [&str; 10] = [
    "1", "2", "3", "y", "n", "up", "down", "enter", "esc", "ctrl+c",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionError {
    pub code: String,
    pub message: String,
}

impl ActionError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    /// The server did not answer within the action's time budget.
    pub fn timeout(server_label: &str) -> Self {
        Self::new("timeout", format!("{server_label} did not answer in time."))
    }

    /// No connection to the server could be made.
    pub fn unreachable(server_label: &str) -> Self {
        Self::new("transport", format!("Could not reach {server_label}."))
    }

    /// The UI named a server the hub does not run.
    pub fn unknown_server(server_id: &str) -> Self {
        Self::new("unknown_server", format!("Unknown server '{server_id}'."))
    }
}

/// Longest message `send_message` accepts, in bytes.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;

/// Map a client error to what the drawer shows (spec section 10).
pub fn friendly(error: &ClientError, server_label: &str) -> ActionError {
    match error {
        ClientError::Api(body) => match body.code.as_str() {
            "agent_blocked" => ActionError::new(
                "agent_blocked",
                "Waiting at a prompt. Answer it with the keypad first.",
            ),
            "not_found" => ActionError::new("not_found", "That agent is gone."),
            other => ActionError::new(other, body.message.clone()),
        },
        ClientError::Closed(_) => ActionError::new(
            "closed",
            "herdr closed the connection; the pane may be gone.",
        ),
        ClientError::Transport(_) => ActionError::unreachable(server_label),
        ClientError::Protocol(msg) => {
            ActionError::new("protocol", format!("Unexpected reply from herdr: {msg}"))
        }
    }
}

/// Read the last `lines` lines of a pane's recent unwrapped output.
pub async fn peek(
    client: &Client,
    pane_id: &str,
    lines: u32,
    server_label: &str,
) -> Result<ReadResult, ActionError> {
    match client
        .request(Method::AgentRead {
            target: pane_id.to_string(),
            lines,
        })
        .await
    {
        Ok(ResponseResult::PaneRead { read }) => Ok(read),
        Ok(other) => {
            let debug = format!("{other:?}");
            let variant: String = debug
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            Err(ActionError::new(
                "protocol",
                format!("Unexpected reply from herdr: {variant}"),
            ))
        }
        Err(e) => Err(friendly(&e, server_label)),
    }
}

/// Send one keypad key (see [`KEYPAD`]); anything else is rejected locally.
pub async fn send_key(
    client: &Client,
    pane_id: &str,
    key: &str,
    server_label: &str,
) -> Result<(), ActionError> {
    if !KEYPAD.contains(&key) {
        return Err(ActionError::new(
            "invalid_key",
            format!("'{key}' is not a keypad key"),
        ));
    }
    client
        .request(Method::AgentSendKeys {
            target: pane_id.to_string(),
            keys: vec![key.to_string()],
        })
        .await
        .map(|_| ())
        .map_err(|e| friendly(&e, server_label))
}

/// Send a trimmed one-line message to the agent in a pane (empty or over 16 KiB is rejected).
pub async fn send_message(
    client: &Client,
    pane_id: &str,
    text: &str,
    server_label: &str,
) -> Result<(), ActionError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ActionError::new("empty_message", "Type a message first."));
    }
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(ActionError::new("message_too_long", "Message is too long."));
    }
    client
        .request(Method::AgentPrompt {
            target: pane_id.to_string(),
            text: text.to_string(),
        })
        .await
        .map(|_| ())
        .map_err(|e| friendly(&e, server_label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_client::transport::TransportError;
    use herdr_client::ErrorBody;

    #[test]
    fn friendly_maps_each_client_error() {
        let closed = friendly(&ClientError::Closed("early".into()), "box");
        assert_eq!(closed.code, "closed");
        assert_eq!(
            closed.message,
            "herdr closed the connection; the pane may be gone."
        );
        let protocol = friendly(&ClientError::Protocol("bad".into()), "box");
        assert_eq!(protocol.code, "protocol");
        assert_eq!(protocol.message, "Unexpected reply from herdr: bad");
        let transport = friendly(
            &ClientError::Transport(TransportError::NotRunning("x".into())),
            "box",
        );
        assert_eq!(transport, ActionError::unreachable("box"));
        assert_eq!(transport.message, "Could not reach box.");
        let api = friendly(
            &ClientError::Api(ErrorBody {
                code: "weird".into(),
                message: "m".into(),
            }),
            "box",
        );
        assert_eq!((api.code.as_str(), api.message.as_str()), ("weird", "m"));
    }

    #[test]
    fn constructors_have_stable_codes() {
        assert_eq!(ActionError::timeout("box").code, "timeout");
        assert_eq!(
            ActionError::timeout("box").message,
            "box did not answer in time."
        );
        assert_eq!(ActionError::unknown_server("x").code, "unknown_server");
        assert_eq!(
            ActionError::unknown_server("x").message,
            "Unknown server 'x'."
        );
    }
}
