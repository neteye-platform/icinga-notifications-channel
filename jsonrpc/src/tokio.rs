//! Tokio integration: turn a `ServerBuilder` plus any `AsyncRead` / `AsyncWrite` pair and
//! a `tokio_util::codec` into a runnable `Future` driving request/response flow.
//!
//! This module is enabled via the `tokio_util` feature. It frames incoming JSON-RPC
//! messages as `String`s (one per decoded frame) and forwards serialized responses
//! using the provided codec. Typical usage leverages `LinesCodec`, but any codec
//! implementing `Decoder<Item = String>` and `Encoder<String>` works.
//!
//! Example:
//!
//! ```no_run
//! use jsonrpc_rs::{jsonrpc, server::ServerBuilder, tokio::BuilderTokioServerExt};
//!
//! #[jsonrpc] async fn add(a: i64, b: i64) -> i64 { a + b }
//!
//! fn main() {
//!     let codec = tokio_util::codec::LinesCodec::new();
//!     let server = ServerBuilder::new()
//!         .register_named(add)
//!         .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());
//!
//!     tokio::runtime::Builder::new_current_thread()
//!         .build().unwrap()
//!         .block_on(server).unwrap();
//! }
//! ```
//!
//! Each inbound frame must be a complete JSON-RPC request (single or batch). Outbound
//! responses are serialized as a single line/frame.
use std::{
    io,
    marker::PhantomData,
    pin::{Pin, pin},
    task::Poll,
};

use futures_util::{Sink, Stream, StreamExt, ready, stream::Forward};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

use crate::server::{
    JsonRpcErrorCode, JsonRpcMethodCall, JsonRpcResponse, JsonRpcResponseSingle, JsonRpcResult,
    Server, ServerBuilder,
};

/// A `Future` (and internal stream forwarder) that drives a JSON-RPC server
/// over an `AsyncRead`/`AsyncWrite` pair using a user-supplied codec.
///
/// Created via [`BuilderTokioServerExt::build_with_codec`]. Polling / awaiting
/// this future will:
/// * Decode incoming frames into `String` JSON payloads.
/// * Feed them into the core `Server`.
/// * Encode `JsonRpcResponse` values back onto the writer.
///
/// The future resolves to `Ok(())` when the underlying stream ends or to an
/// error if the codec writer encounters one.
pub struct TokioServer<I, O, C, R, W>(
    Forward<
        TryStreamAdapter<<C as Encoder<O>>::Error, Server<CodecCompatRead<R, C>>>,
        CodecCompatWrite<O, W, C>,
    >,
)
where
    I: Into<Vec<u8>>,
    O: From<String>,
    C: Decoder<Item = I> + Encoder<O> + Clone,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin;

impl<I, O, C, R, W> Future for TokioServer<I, O, C, R, W>
where
    I: Into<Vec<u8>>,
    O: From<String>,
    C: Decoder<Item = I> + Encoder<O> + Clone,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    type Output = Result<(), <C as Encoder<O>>::Error>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        let fut = pin!(&mut self.0);
        fut.poll(cx)
    }
}

/// Extension trait adding a Tokio / codec based constructor to `ServerBuilder`.
///
/// This consumes the builder and returns a [`TokioServer`] future that can be
/// `.await`ed directly. The codec must produce `String` items (decoded JSON
/// requests) and accept `String` for encoded responses.
///
/// See module-level docs for a full example.
pub trait BuilderTokioServerExt {
    /// Finalize the `ServerBuilder` into a [`TokioServer`] using the provided `codec`,
    /// `read` (implementing `AsyncRead + Unpin`) and `write` (implementing `AsyncWrite + Unpin`).
    ///
    /// Each decoded frame must be valid UTF-8 JSON (single request or batch).
    /// Responses are serialized with `serde_json::to_string` and passed to the codec encoder.
    fn build_with_codec<I, O, Codec, Read, Write>(
        self,
        codec: Codec,
        read: Read,
        write: Write,
    ) -> TokioServer<I, O, Codec, Read, Write>
    where
        I: Into<Vec<u8>>,
        O: From<String>,
        Codec: Encoder<O> + Decoder<Item = I> + Clone,
        Read: AsyncRead + Unpin,
        Write: AsyncWrite + Unpin;
}

impl BuilderTokioServerExt for ServerBuilder {
    fn build_with_codec<I, O, Codec, Read, Write>(
        self,
        codec: Codec,
        read: Read,
        write: Write,
    ) -> TokioServer<I, O, Codec, Read, Write>
    where
        I: Into<Vec<u8>>,
        O: From<String>,
        Codec: Encoder<O> + Decoder<Item = I> + Clone,
        Read: AsyncRead + Unpin,
        Write: AsyncWrite + Unpin,
    {
        let reader = FramedRead::new(read, codec.clone());
        let writer = FramedWrite::new(write, codec);

        let server = TryStreamAdapter(self.build(CodecCompatRead(reader)), PhantomData)
            .forward(CodecCompatWrite(writer, PhantomData));

        TokioServer(server)
    }
}

struct CodecCompatRead<S, D>(FramedRead<S, D>);

impl<I: Into<Vec<u8>>, S: AsyncRead + Unpin, D: Decoder<Item = I>> Stream
    for CodecCompatRead<S, D>
{
    type Item = I;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let fut = pin!(&mut self.0);
        let Some(item) = ready!(fut.poll_next(cx)) else {
            return Poll::Ready(None);
        };

        // ToDo: Log error here (?)
        Poll::Ready(item.ok())
    }
}

struct CodecCompatWrite<O, S, E: Encoder<O>>(FramedWrite<S, E>, PhantomData<O>);

impl<O, T, E> Sink<JsonRpcResponse> for CodecCompatWrite<O, T, E>
where
    O: From<String>,
    T: AsyncWrite + Unpin,
    E: Encoder<O>,
    E::Error: From<io::Error>,
{
    type Error = E::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let fut = pin!(&mut self.0);
        fut.poll_ready(cx)
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        item: JsonRpcResponse,
    ) -> Result<(), Self::Error> {
        let result = serde_json::to_string(&item).unwrap();
        let pin = pin!(&mut self.0);
        pin.start_send(result.into())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let fut = pin!(&mut self.0);
        fut.poll_flush(cx)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let fut = pin!(&mut self.0);
        fut.poll_close(cx)
    }
}

impl<O, S, E: Encoder<O>> Unpin for CodecCompatWrite<O, S, E> {}

struct TryStreamAdapter<E, S>(S, PhantomData<E>);

impl<E, S: Stream + Unpin> Stream for TryStreamAdapter<E, S> {
    type Item = Result<S::Item, E>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let fut = pin!(&mut self.0);
        let res = ready!(fut.poll_next(cx));
        Poll::Ready(res.map(Ok))
    }
}

impl<E, S> Unpin for TryStreamAdapter<E, S> {}

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
/// use jsonrpc_rs::{jsonrpc, server::ServerBuilder, tokio::BuilderTokioServerExt};
/// use jsonrpc_rs::tokio::tokio_parallel;
///
/// #[jsonrpc] async fn add(a: i32, b: i32) -> i32 { a + b }
///
/// fn main() {
///     let codec = tokio_util::codec::LinesCodec::new();
///     let server = ServerBuilder::new()
///         .register_named(add)
///         .map_requests(&tokio_parallel)
///         .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());
///     // drive the server on a runtime ...
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
