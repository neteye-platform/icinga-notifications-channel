//! # jsonrpc-rs
//!
//! An implementation of the [JSON-RPC 2.0](https://www.jsonrpc.org/specification) protocol for Rust,
//! designed for ergonomics and transport agnosticism. The crate focuses on:
//!
//! * Zero opinion on transport (works over any `Stream<Item = String>`).
//! * Async method execution with bounded parallelism.
//! * Batch request support.
//! * Notifications (requests without an `id`).
//! * Managed shared state injection.
//! * Optional Tokio integration (via `tokio_util` codecs).
//! * Procedural macro (`#[jsonrpc]`) to eliminate boilerplate.
//!
//! JSON-RPC 2.0 is a stateless, lightweight RPC protocol defining message formats and error semantics
//! without dictating the underlying transport. This crate lets you plug in stdin/stdout, TCP, websockets,
//! in-memory channels, or any other line/packet oriented source producing JSON strings.
//!
//! ## Features
//!
//! * `macros` – enables the `#[jsonrpc]` attribute macro.
//! * `tokio_util` – enables the `tokio` module with `BuilderTokioServerExt`.
//!
//! ## Quick Start (Macro-Based)
//!
//! ```rust,no_run
//! use jsonrpc_rs::{jsonrpc, server::ServerBuilder, tokio::BuilderTokioServerExt};
//!
//! struct ManagedState { greeting: String }
//!
//! #[jsonrpc]
//! async fn add(a: i64, b: i64) -> i64 { a + b }
//!
//! #[jsonrpc(name = "ping")]
//! async fn ping() -> &'static str { "pong" }
//!
//! #[jsonrpc(state = "app")]
//! async fn greet(app: &ManagedState, name: String) -> String {
//!     format!("{}, {name}!", app.greeting)
//! }
//!
//! fn main() {
//!     let codec = tokio_util::codec::LinesCodec::new();
//!     let server = ServerBuilder::new()
//!         .manage(ManagedState { greeting: "Hello".into() })
//!         .register_named(add)
//!         .register_named(ping)
//!         .register_named(greet)
//!         .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());
//!
//!     tokio::runtime::Builder::new_current_thread()
//!         .build().unwrap()
//!         .block_on(server).unwrap();
//! }
//! ```
//!
//! Example client messages (one JSON entity per line):
//!
//! ```text
//! {"jsonrpc":"2.0","method":"add","params":{"a":7,"b":35},"id":1}
//! {"jsonrpc":"2.0","method":"ping","id":2}
//! {"jsonrpc":"2.0","method":"greet","params":{"name":"World"},"id":3}
//! ```
//!
//! Responses:
//!
//! ```text
//! {"jsonrpc":"2.0","result":42,"id":1}
//! {"jsonrpc":"2.0","result":"pong","id":2}
//! {"jsonrpc":"2.0","result":"Hello, World!","id":3}
//! ```
//!
//! ## Managed State
//!
//! Use `.manage(value)` on `ServerBuilder` to inject shared state. With the macro you mark an argument
//! as coming from managed state using `#[jsonrpc(state = "field_name")]` where the parameter name
//! matches the identifier you pass to the macro (`state = "app"` in the example above).
//!
//! Manually, you can access typed managed state via `ServerState::get_arc_lock::<T>()`.
//!
//! ## Notifications
//!
//! A request without an `id` (or with `null` id) is a notification. No response will be emitted, per spec.
//!
//! ```
//! {"jsonrpc":"2.0","method":"add","params":[1,2]}
//! ```
//!
//! ## Batch Requests
//!
//! Provide a JSON array of request objects. An empty array yields an `Invalid Request` error (spec-compliant).
//!
//! ```text
//! [
//!   {"jsonrpc":"2.0","method":"add","params":[4,5],"id":1},
//!   {"jsonrpc":"2.0","method":"ping","id":2},
//!   {"jsonrpc":"2.0","method":"no_such","id":3},
//!   {"jsonrpc":"2.0","method":"add","params":[10,7]} // notification
//! ]
//! ```
//!
//! Example response (notification omitted):
//!
//! ```text
//! [
//!   {"jsonrpc":"2.0","result":9,"id":1},
//!   {"jsonrpc":"2.0","result":"pong","id":2},
//!   {"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":3}
//! ]
//! ```
//!
//! ## Manual Registration (Without Macros)
//!
//! You can bypass the macro for custom parsing / validation:
//!
//! ```rust
//! use jsonrpc_rs::server::{ServerBuilder, JsonRpcRequest, JsonRpcMethodCall,
//!     JsonRpcResponseSingle, JsonRpcResult, JsonRpcErrorCode, parse_params, ServerState};
//! use futures_util::stream;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct DivParams { numerator: i64, denominator: i64 }
//!
//! fn div_dispatch(_: &ServerState, req: &JsonRpcRequest)
//!     -> Result<JsonRpcMethodCall, JsonRpcResponseSingle>
//! {
//!     let params: DivParams = parse_params(req)?;
//!     if params.denominator == 0 {
//!         return Err(JsonRpcResponseSingle::new(JsonRpcResult::Err {
//!             code: JsonRpcErrorCode::invalid_params(),
//!             message: "Division by zero".into(),
//!             data: None,
//!         }));
//!     }
//!     Ok(Box::new(async move {
//!         JsonRpcResponseSingle::new(JsonRpcResult::Ok(
//!             Box::new(params.numerator / params.denominator)
//!         )).into()
//!     }))
//! }
//!
//! # #[tokio::main(flavor="current_thread")]
//! # async fn main() {
//! let inputs = vec![
//!     r#"{"jsonrpc":"2.0","method":"div","params":{"numerator":10,"denominator":2},"id":1}"#.to_string(),
//!     r#"{"jsonrpc":"2.0","method":"div","params":{"numerator":5,"denominator":0},"id":2}"#.to_string(),
//! ];
//! let mut server = ServerBuilder::new()
//!     .register("div".to_string(), &div_dispatch)
//!     .build(stream::iter(inputs));
//! while let Some(resp) = server.next().await {
//!     println!("{}", serde_json::to_string(&resp).unwrap());
//! }
//! # }
//! ```
//!
//! ## Concurrency & Mapping
//!
//! Use `.max_parallel_requests(n)` to bound in-flight method futures. Use `.map_requests` to wrap
//! each method future (e.g., spawn, add tracing, apply timeouts):
//!
//! ```rust
//! .map_requests(&|fut| Box::pin(async move {
//!     // Example: spawn onto Tokio
//!     tokio::spawn(fut).await.unwrap()
//! }))
//! ```
//!
//! ## Tokio Integration
//!
//! The `tokio` module (feature `tokio_util`) adds `BuilderTokioServerExt::build_with_codec` to transform
//! a `ServerBuilder` plus an `AsyncRead/AsyncWrite` pair and a `tokio_util::codec` into a `Future` that you
//! can `await` directly.
//!
//! ## Error Codes
//!
//! Implemented helpers:
//!
//! * `parse_error()`  → -32700
//! * `invalid_request()` → -32600
//! * `method_not_found()` → -32601
//! * `invalid_params()` → -32602
//! * `internal_error()` → -32603
//! * `server_error(code)` / `custom_error(code)` for application & extension ranges
//!
//! ## Best Practices
//!
//! * Return `invalid_params` for semantically wrong but well-formed input.
//! * Use notifications for fire-and-forget work to reduce overhead.
//! * Keep methods small; offload expensive processing via `.map_requests` or internal spawning.
//! * Provide structured `data` in errors for easier client-side diagnostics.
//!
//! ## Related Specification Sections
//!
//! | Spec Concept        | API Surface                                      |
//! |---------------------|--------------------------------------------------|
//! | Request Object      | `JsonRpcRequest`                                 |
//! | Response Object     | `JsonRpcResponseSingle`, `JsonRpcResult`         |
//! | Batch               | `JsonRpcResponse::Bulk`                          |
//! | Error Object        | `JsonRpcResult::Err`, `JsonRpcErrorCode`         |
//! | Notification        | `id: None` (suppressed response)                 |
//! | Transport Agnostic  | Generic `Stream<Item = String>` input            |
//!
//! ## License & Contribution
//!
//! Contributions are welcome. Please follow typical Rust coding and documentation standards. Additional
//! examples (HTTP, WebSocket) can be layered externally by adapting a transport into a `Stream` of JSON lines.
//!
//! ---
pub mod server;

#[cfg(feature = "tokio_util")]
pub mod tokio;

#[cfg(feature = "macros")]
pub use jsonrpc_rs_macros::jsonrpc;

#[doc(hidden)]
pub use serde;
