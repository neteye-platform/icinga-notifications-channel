//! Tokio integration: serve a [`Server`] over any `AsyncRead` / `AsyncWrite` pair
//! framed by a `tokio_util::codec`.
//!
//! This module is enabled via the `tokio_util` feature. It frames incoming JSON-RPC
//! messages as `String`s (one per decoded frame) and forwards serialized responses
//! using the provided codec. Typical usage leverages `LinesCodec`, but any codec
//! implementing `Decoder` (with `Item: Into<Vec<u8>>`) and `Encoder<String>` works.
//!
//! Example:
//!
//! ```no_run
//! use jsonrpc_rs::{jsonrpc, server::ServerBuilder};
//!
//! #[jsonrpc] async fn add(a: i64, b: i64) -> i64 { a + b }
//!
//! fn main() {
//!     let codec = tokio_util::codec::LinesCodec::new();
//!     let server = ServerBuilder::new().register_named(add).build();
//!
//!     tokio::runtime::Builder::new_current_thread()
//!         .build().unwrap()
//!         .block_on(server.serve_with_codec(
//!             codec, tokio::io::stdin(), tokio::io::stdout()))
//!         .unwrap();
//! }
//! ```
//!
//! Each inbound frame must be a complete JSON-RPC request (single or batch). Outbound
//! responses are serialized as a single line/frame.
use std::{fmt::Display, pin::Pin};

use futures_util::{SinkExt, StreamExt, future::ready};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

use crate::server::{
    JsonRpcErrorCode, JsonRpcMethodCall, JsonRpcResponse, JsonRpcResponseSingle, JsonRpcResult,
    Server,
};

impl Server {
    /// Serves one connection over an `AsyncRead`/`AsyncWrite` pair using `codec`
    /// for framing.
    ///
    /// Each decoded frame must be one JSON-RPC request or batch. Frames that fail
    /// to decode are logged and skipped. Responses are serialized with
    /// `serde_json` and encoded as one `String` frame each.
    pub async fn serve_with_codec<Codec>(
        &self,
        codec: Codec,
        read: impl AsyncRead,
        write: impl AsyncWrite,
    ) -> Result<(), <Codec as Encoder<String>>::Error>
    where
        Codec: Decoder + Encoder<String> + Clone,
        Codec::Item: Into<Vec<u8>>,
        <Codec as Decoder>::Error: Display,
    {
        let source = FramedRead::new(read, codec.clone()).filter_map(|frame| {
            ready(
                frame
                    .inspect_err(|error| tracing::warn!(%error, "Skipping undecodable frame"))
                    .ok(),
            )
        });
        let sink = FramedWrite::new(write, codec).with(|response: JsonRpcResponse| {
            // Serializing a response cannot fail: all keys are strings.
            ready(Ok(serde_json::to_string(&response).unwrap()))
        });
        self.serve(source, sink).await
    }
}

/// Wraps a JSON-RPC method future in a Tokio task for parallel execution.
///
/// This helper can be passed to `ServerBuilder::map_requests(&tokio_parallel)`
/// to offload each method future onto the Tokio runtime. It preserves the
/// original future's output. If the spawned task panics or is cancelled, an
/// `Internal error` (-32603) response is generated to avoid silently dropping
/// the request.
///
/// # Example
/// ```rust
/// use jsonrpc_rs::{jsonrpc, server::ServerBuilder, tokio::tokio_parallel};
///
/// #[jsonrpc] async fn add(a: i32, b: i32) -> i32 { a + b }
///
/// fn main() {
///     let codec = tokio_util::codec::LinesCodec::new();
///     let server = ServerBuilder::new()
///         .register_named(add)
///         .map_requests(&tokio_parallel)
///         .build();
///     let serve = server.serve_with_codec(
///         codec, tokio::io::stdin(), tokio::io::stdout());
///     // drive `serve` on a runtime ...
/// }
/// ```
///
/// # Errors
/// Returns an `Internal error` (-32603) JSON-RPC response if the spawned task
/// fails to join (panic or cancellation).
pub fn tokio_parallel(input: Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall> {
    Box::pin(async move {
        match tokio::task::spawn(input).await {
            Ok(result) => result,
            Err(_) => JsonRpcResponse::Single(JsonRpcResponseSingle::new(JsonRpcResult::Err {
                code: JsonRpcErrorCode::internal_error(),
                message: "Internal server error".to_string(),
                data: None,
            })),
        }
    })
}
