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
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

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
            "transport",
            "herdr closed the connection; the pane may be gone.",
        ),
        ClientError::Transport(_) => {
            ActionError::new("transport", format!("Could not reach {server_label}."))
        }
        ClientError::Protocol(msg) => {
            ActionError::new("protocol", format!("Unexpected reply from herdr: {msg}"))
        }
    }
}

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
        Ok(other) => Err(ActionError::new(
            "protocol",
            format!("Unexpected reply from herdr: {other:?}"),
        )),
        Err(e) => Err(friendly(&e, server_label)),
    }
}

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
    client
        .request(Method::AgentPrompt {
            target: pane_id.to_string(),
            text: text.to_string(),
        })
        .await
        .map(|_| ())
        .map_err(|e| friendly(&e, server_label))
}
