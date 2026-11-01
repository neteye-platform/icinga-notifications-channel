//! Errors a channel reports to Icinga Notifications.

use std::fmt;

use jsonrpc_rs::server::{JsonRpcErrorCode, JsonRpcResult};

/// The JSON-RPC code used by [`ChannelError::new`].
pub const DEFAULT_ERROR_CODE: i32 = -32000;

/// An error returned as a JSON-RPC error response.
///
/// The message is stored in the notification history, so it should be
/// meaningful to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelError {
    code: i32,
    message: String,
}

impl ChannelError {
    /// An error with the code `-32000`.
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_code(DEFAULT_ERROR_CODE, message)
    }

    /// An error with a custom JSON-RPC code.
    pub fn with_code(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The JSON-RPC error code.
    pub fn code(&self) -> i32 {
        self.code
    }

    /// The message shown to the user.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ChannelError {}

impl From<ChannelError> for JsonRpcResult {
    fn from(error: ChannelError) -> Self {
        JsonRpcResult::Err {
            code: JsonRpcErrorCode::custom_error(error.code),
            message: error.message,
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_json(error: ChannelError) -> serde_json::Value {
        serde_json::to_value(JsonRpcResult::from(error)).unwrap()
    }

    #[test]
    fn converts_with_default_code() {
        assert_eq!(
            to_json(ChannelError::new("boom")),
            serde_json::json!({"error": {"code": -32000, "message": "boom"}})
        );
    }

    #[test]
    fn converts_with_custom_code() {
        assert_eq!(
            to_json(ChannelError::with_code(-32001, "x"))["error"]["code"],
            -32001
        );
    }
}
