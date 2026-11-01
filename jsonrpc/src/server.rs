//! Core server components for the `jsonrpc-rs` crate.
//!
//! This module implements the transport‑agnostic processing pipeline for
//! JSON-RPC 2.0 requests: single, batch, and notification handling; method
//! dispatch; managed state access; error generation; and concurrency control.
//!
//! Most users will interact with the higher level [`ServerBuilder`] and (with
//! the `macros` feature) the `#[jsonrpc]` procedural macro. Manual method
//! registration is also supported for advanced use cases (custom validation,
//! dynamic dispatch, instrumentation, etc.).
use std::{
    any::Any,
    collections::HashMap,
    pin::{Pin, pin},
    sync::Arc,
    task::Poll,
    usize,
};

use async_lock::RwLock;
use erased_serde::Serialize as DynSerialize;
use futures_util::{
    Stream, StreamExt, ready,
    stream::{Fuse, FuturesUnordered},
};
use monostate::MustBe;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json, value::RawValue};

#[derive(Deserialize)]
/// A deserialized JSON-RPC 2.0 request object.
///
/// Represents either:
/// * A standard method call (has `id`)
/// * A notification (no `id`)
pub struct JsonRpcRequest<'request> {
    #[allow(dead_code)]
    jsonrpc: MustBe!("2.0"),
    method: String,
    #[serde(borrow)]
    params: Option<&'request RawValue>,
    id: Option<Value>,
}

impl JsonRpcRequest<'_> {
    /// Returns the raw JSON string of the `params` object/array.
    ///
    /// If `params` is absent in the request, this returns `"{}"` (an empty object),
    /// enabling ergonomic parsing for methods expecting named parameters.
    ///
    /// # Examples
    /// ```rust
    /// # use jsonrpc_rs::server::JsonRpcRequest;
    /// let req: JsonRpcRequest = serde_json::from_str(r#"{
    ///     "jsonrpc": "2.0",
    ///     "method": "foo",
    ///     "params": { "a": 1 }
    /// }"#);
    /// assert_eq!(req.param_str(), r#"{ "a": 1}"#);
    /// ```
    pub fn param_str(&self) -> &str {
        // If no params are given, default to an empty object
        self.params.map(|r| r.get()).unwrap_or("{}")
    }

    /// Returns the request `id`, if present.
    ///
    /// `None` denotes a notification per JSON-RPC 2.0. Such calls must not
    /// produce a response.
    pub fn id(&self) -> Option<&Value> {
        self.id.as_ref()
    }
}

#[derive(Serialize)]
/// Result of executing a JSON-RPC method:
///
/// * `Ok`  → serialized as `"result": <value>`
/// * `Err` → serialized as `"error": { "code": <int>, "message": <string>, "data"?: <json> }`
///
/// The `data` field (if provided) gives clients machine-readable context about
/// the failure.
pub enum JsonRpcResult {
    #[serde(rename = "result")]
    Ok(Box<dyn DynSerialize + Send>),
    #[serde(rename = "error")]
    Err {
        code: JsonRpcErrorCode,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Box<dyn DynSerialize + Send>>,
    },
}

#[derive(Serialize)]
/// A single JSON-RPC 2.0 response object (`{"jsonrpc":"2.0", ..., "id": <id>}`).
///
/// Contains either a `result` or an `error` (see [`JsonRpcResult`]). The `id`
/// is populated after method future completion. For notifications, no response
/// object is emitted upstream.
pub struct JsonRpcResponseSingle {
    jsonrpc: MustBe!("2.0"),
    #[serde(flatten)]
    result: JsonRpcResult,
    id: Value,
}

#[derive(Serialize)]
#[serde(untagged)]
/// Top-level response enumeration:
///
/// * `Single` – one response object
/// * `Bulk`   – batch of response objects, ordered in the input order with notifications skipped.
///
/// This mirrors JSON-RPC 2.0 batch semantics.
pub enum JsonRpcResponse {
    Single(JsonRpcResponseSingle),
    Bulk(Vec<JsonRpcResponseSingle>),
}

impl JsonRpcResponseSingle {
    /// Creates a new response with the given [`JsonRpcResult`].
    ///
    /// The `id` is initialized to `Null` and later replaced before emission.
    /// Prefer this constructor when building manual method dispatch implementations.
    pub fn new(result: JsonRpcResult) -> Self {
        Self {
            jsonrpc: MustBe!("2.0"),
            result,
            id: Value::Null,
        }
    }
}

impl From<JsonRpcResponseSingle> for JsonRpcResponse {
    /// Wraps a single response object into the `JsonRpcResponse::Single` variant.
    fn from(value: JsonRpcResponseSingle) -> Self {
        JsonRpcResponse::Single(value)
    }
}

#[derive(Serialize)]
/// Wrapper for JSON-RPC error codes (standard and custom).
///
/// Provided constructors cover the canonical JSON-RPC 2.0 codes:
///
/// | Helper                 | Code    | Meaning            |
/// |------------------------|---------|--------------------|
/// | `parse_error()`        | -32700  | Invalid JSON       |
/// | `invalid_request()`    | -32600  | Bad request object |
/// | `method_not_found()`   | -32601  | Unknown method     |
/// | `invalid_params()`     | -32602  | Invalid params     |
/// | `internal_error()`     | -32603  | Internal error     |
///
/// Additional helpers:
/// * `server_error(code)` – server-defined extension codes
/// * `custom_error(code)` – arbitrary application codes
pub struct JsonRpcErrorCode(i32);

impl JsonRpcErrorCode {
    /// Returns the underlying integer error code.
    pub const fn error_code(&self) -> i32 {
        self.0
    }

    /// Standard JSON-RPC `Parse error` (-32700).
    pub const fn parse_error() -> Self {
        Self(-32700)
    }

    /// Standard JSON-RPC `Invalid Request` (-32600).
    pub const fn invalid_request() -> Self {
        Self(-32600)
    }

    /// Standard JSON-RPC `Method not found` (-32601).
    pub const fn method_not_found() -> Self {
        Self(-32601)
    }

    /// Standard JSON-RPC `Invalid params` (-32602).
    pub const fn invalid_params() -> Self {
        Self(-32602)
    }

    /// Standard JSON-RPC `Internal error` (-32603).
    pub const fn internal_error() -> Self {
        Self(-32603)
    }

    /// Server error (implementation-defined). Consider using codes in the
    /// recommended server range (-32000..-32099) for interoperability.
    pub const fn server_error(error_code: i32) -> Self {
        Self(error_code)
    }

    /// Custom application error code. Make sure to avoid clashes with the
    /// official range for future compatibility.
    pub const fn custom_error(error_code: i32) -> Self {
        Self(error_code)
    }
}

/// Boxed future produced by a registered JSON-RPC method.
/// Resolves to a [`JsonRpcResponse`] (single or bulk).
pub type JsonRpcMethodCall = Box<dyn Future<Output = JsonRpcResponse> + 'static + Send>;

/// Low-level method handler signature used for manual registration.
///
/// Returns:
/// * `Ok(JsonRpcMethodCall)` – async evaluation producing a response
/// * `Err(JsonRpcResponseSingle)` – immediate error response
///
/// Manual handlers allow custom validation, metrics, tracing, etc.
pub type JsonRpcMethod =
    &'static dyn for<'server, 'request> Fn(
        &'server ServerState,
        &'request JsonRpcRequest<'request>,
    ) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle>;

/// Static pairing of JSON-RPC method name and its dispatch function.
///
/// Usually generated by the `#[jsonrpc]` macro, but can be constructed
/// manually using [`JsonRpcNamedMethod::new`].
pub struct JsonRpcNamedMethod {
    name: &'static str,
    method: JsonRpcMethod,
}

impl JsonRpcNamedMethod {
    /// Constructs a new statically named method wrapper.
    ///
    /// # Parameters
    /// * `name`   – JSON-RPC method string
    /// * `method` – handler function reference
    pub const fn new(name: &'static str, method: JsonRpcMethod) -> Self {
        Self { name, method }
    }
}

/// Builder for configuring and constructing a JSON-RPC server.
///
/// Key capabilities:
/// * Register methods (`register`, `register_named`)
/// * Inject shared state (`manage`)
/// * Constrain concurrency (`max_parallel_requests`)
/// * Wrap/transform method futures (`map_requests`)
/// * Finalize to a running [`Server`] with [`ServerBuilder::build`]
///
/// The builder is `Clone` to support patterns where partial configurations
/// are reused, or the config is re-used for several connections.
#[derive(Clone)]
pub struct ServerBuilder {
    registered_methods: HashMap<String, JsonRpcMethod>,
    managed_state: ServerState,
    mapper: Option<&'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>>,
    max_parallel_requests: usize,
}

impl ServerBuilder {
    /// Creates a new empty builder with:
    /// * No registered methods
    /// * No mapper
    /// * Fresh `ServerState`
    /// * Unlimited parallel requests (`usize::MAX`)
    pub fn new() -> Self {
        Self {
            registered_methods: HashMap::new(),
            mapper: None,
            managed_state: ServerState::new(),
            max_parallel_requests: usize::MAX,
        }
    }

    /// Registers a method under the provided `name`.
    ///
    /// Use this for manual registration. For macro-generated methods prefer
    /// [`ServerBuilder::register_named`].
    ///
    /// Returns the mutated builder for fluent chaining.
    pub fn register(mut self, name: String, method: JsonRpcMethod) -> Self {
        self.registered_methods.insert(name, method);
        self
    }

    /// Registers a macro-generated or manually constructed named method.
    ///
    /// Returns the mutated builder for fluent chaining.
    pub fn register_named(mut self, named: JsonRpcNamedMethod) -> Self {
        self.registered_methods
            .insert(named.name.to_string(), named.method);
        self
    }

    /// Sets the maximum number of concurrent in-flight method futures.
    ///
    /// Must be greater than zero; a panic occurs if set to zero and used.
    /// Useful for controlling resource usage or rate limiting. If the requests need
    /// to be strictly processed in order, this can be set to 1 to disable concurrent
    /// processing.
    pub fn max_parallel_requests(mut self, max_parallel_requests: usize) -> Self {
        self.max_parallel_requests = max_parallel_requests;
        self
    }

    /// Installs a mapping function that can wrap every method future that is spawned.
    ///
    /// Common use cases:
    /// * Spawning onto a runtime (e.g., `tokio::spawn`)
    /// * Adding tracing spans (especially for tracing lock-contention)
    /// * Injecting timeouts or cancellation
    pub fn map_requests(
        mut self,
        mapper: &'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>,
    ) -> Self {
        self.mapper = Some(mapper);
        self
    }

    /// Injects a managed state value accessible to methods via the macro
    /// (`#[jsonrpc(state = "...")]`) or manually through
    /// [`ServerState::get_arc_lock`].
    pub fn manage<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.managed_state.manage(state);
        self
    }

    /// Finalizes the builder into a running [`Server`].
    ///
    /// The `source` stream yields raw JSON bytes for single or batch requests.
    /// Each item must represent one full JSON-RPC entity (request or batch array).
    ///
    /// # Type Parameters
    /// * `I`: An item convertible to `Vec<u8>` (e.g. `String`, `Bytes`, `Vec<u8>`)
    /// * `Source`: A stream of `I` values
    pub fn build<I: Into<Vec<u8>>, Source: Stream<Item = I> + Unpin>(
        self,
        source: Source,
    ) -> Server<Source> {
        let dispatcher = Dispatcher {
            registered_methods: self.registered_methods,
            source: source,
            state: self.managed_state,
        };

        Server {
            source: StreamExt::fuse(dispatcher),
            mapper: self.mapper,
            max_parallel_requests: self.max_parallel_requests,
            running_futures: FuturesUnordered::new(),
        }
    }
}

/// Internal dispatcher driving request parsing and method invocation.
///
/// Not public; encapsulates JSON decoding, batch decomposition, notification
/// suppression, and error fallback logic.
struct Dispatcher<Source> {
    registered_methods: HashMap<String, JsonRpcMethod>,
    state: ServerState,
    source: Source,
}

/// Wrapper holding a future plus its optional request `id`.
///
/// Used to re-associate the `id` after the method future resolves.
struct JsonRpcMethodCallInner {
    id: Option<Value>,
    fut: Pin<JsonRpcMethodCall>,
}

impl Future for JsonRpcMethodCallInner {
    type Output = Option<JsonRpcResponse>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let fut = pin!(&mut self.fut);
        match ready!(fut.poll(cx)) {
            JsonRpcResponse::Single(mut single_response) => {
                let Some(id) = std::mem::take(&mut self.id) else {
                    return Poll::Ready(None);
                };
                single_response.id = id;
                Poll::Ready(Some(JsonRpcResponse::Single(single_response)))
            }
            JsonRpcResponse::Bulk(bulk) => {
                if bulk.is_empty() {
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(JsonRpcResponse::Bulk(bulk)))
            }
        }
    }
}

impl<Source> Dispatcher<Source> {
    fn process_single(
        &self,
        next: &[u8],
    ) -> Option<Result<JsonRpcMethodCallInner, JsonRpcResponseSingle>> {
        let request = match serde_json::from_slice::<JsonRpcRequest>(&next) {
            Ok(request) => request,
            Err(error) => return Some(Err(parse_error(error))),
        };

        match self.dispatch(&request) {
            Ok(fut) => Some(Ok(JsonRpcMethodCallInner {
                fut: Box::into_pin(fut),
                id: request.id,
            })),
            Err(mut err) => {
                if let Some(id) = request.id {
                    err.id = id;
                    Some(Err(err))
                } else {
                    None
                }
            }
        }
    }

    fn process_batch(&self, next: &[u8]) -> Result<JsonRpcMethodCallInner, JsonRpcResponseSingle> {
        let requests = match serde_json::from_slice::<Vec<&RawValue>>(next) {
            Ok(request) => request,
            Err(error) => return Err(parse_error(error)),
        };

        if requests.is_empty() {
            return Err(JsonRpcResponseSingle::new(JsonRpcResult::Err {
                code: JsonRpcErrorCode::invalid_request(),
                message: "Invalid Request".to_owned(),
                data: None,
            }));
        }

        let mut calls = Vec::with_capacity(requests.len());
        for req in requests {
            let request = match serde_json::from_str::<JsonRpcRequest>(&req.get()) {
                Ok(request) => request,
                Err(error) => {
                    calls.push(JsonRpcMethodCallInner {
                        id: Some(Value::Null),
                        fut: Box::pin(async { JsonRpcResponse::Single(parse_error(error)) }),
                    });
                    continue;
                }
            };

            let call_inner = match self.dispatch(&request) {
                Ok(call) => JsonRpcMethodCallInner {
                    id: request.id,
                    fut: Box::into_pin(call),
                },
                Err(error) => JsonRpcMethodCallInner {
                    id: request.id,
                    fut: Box::pin(async { JsonRpcResponse::Single(error) }),
                },
            };

            calls.push(call_inner);
        }

        let future = Box::pin(async move {
            let mut results = vec![];
            for next in calls {
                if let Some(JsonRpcResponse::Single(response)) = next.await {
                    results.push(response);
                }
            }
            JsonRpcResponse::Bulk(results)
        });

        Ok(JsonRpcMethodCallInner {
            id: None,
            fut: future,
        })
    }

    fn dispatch(&self, req: &JsonRpcRequest) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
        let Some(method_call) = self.registered_methods.get(&req.method) else {
            return Err(method_not_found(req.method.clone()));
        };

        method_call(&self.state, &req)
    }
}

impl<I: Into<Vec<u8>>, Source: Stream<Item = I> + Unpin> Stream for Dispatcher<Source> {
    type Item = Result<JsonRpcMethodCallInner, JsonRpcResponseSingle>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let f = pin!(&mut self.source);
        let Some(next) = ready!(f.poll_next(cx)) else {
            return Poll::Ready(None);
        };

        // ToDo: Consider using serde_path_to_error here.
        let next = next.into();
        if next.starts_with(b"[") {
            Poll::Ready(Some(self.process_batch(&next)))
        } else {
            match self.process_single(&next) {
                Some(single) => Poll::Ready(Some(single)),
                None => {
                    // There was an error in a notification, immediately poll again to continue
                    cx.waker().clone().wake();
                    Poll::Pending
                }
            }
        }
    }
}

#[derive(Clone)]
/// Internal container holding user managed state values registered via
/// [`ServerBuilder::manage`]. State is stored type‑erased and can be retrieved
/// as an `Arc<RwLock<T>>` using [`ServerState::get_arc_lock`] or through the
/// `jsonrpc` macro with the `state` field: `#[jsonrpc(state = "...")]`
pub struct ServerState {
    managed_state: Vec<Arc<dyn Any + Send + Sync + 'static>>,
}

impl Default for ServerState {
    /// Creates an empty `ServerState`.
    fn default() -> Self {
        Self {
            managed_state: Default::default(),
        }
    }
}

impl ServerState {
    /// Internal constructor for an empty state.
    fn new() -> Self {
        Default::default()
    }

    /// Adds a value of type `T` to the managed state list.
    ///
    /// Stored values are wrapped in `Arc<RwLock<T>>`. Duplicate types are
    /// allowed; retrieval returns the first matching instance.
    fn manage<T: Send + Sync + 'static>(&mut self, state: T) {
        // ToDo: Check if the type is already managed?
        let arc = Arc::new(RwLock::new(state));
        self.managed_state.push(arc);
    }

    /// Attempts to retrieve previously managed state of type `T`.
    ///
    /// Returns `Some(Arc<RwLock<T>>)` if found, otherwise `None`.
    ///
    /// # Example
    /// ```rust
    /// # use jsonrpc_rs::server::ServerState;
    /// # let mut st = ServerState::default();
    /// st.manage(42u32);
    /// let arc = st.get_arc_lock::<u32>().unwrap();
    /// ```
    pub fn get_arc_lock<T: Send + Sync + 'static>(&self) -> Option<Arc<RwLock<T>>> {
        for state in &self.managed_state {
            if let Ok(lock) = state.clone().downcast::<RwLock<T>>() {
                return Some(lock);
            }
        }
        return None;
    }
}

/// A running JSON-RPC server over an abstract `Stream` of raw JSON values.
///
/// Implements `Stream<Item = JsonRpcResponse>`. Polling it yields completed
/// responses (single or bulk). Notification requests (no `id`) are suppressed
/// and produce no items. Concurrency is bounded by the configured maximum in
/// [`ServerBuilder::max_parallel_requests`].
pub struct Server<Source> {
    // We will continue to poll the source while we are finishing the requests, so -
    source: Fuse<Dispatcher<Source>>,
    mapper: Option<&'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>>,
    max_parallel_requests: usize,
    // ToDo: Can we let the user choose between ordered and unordered?
    running_futures: FuturesUnordered<JsonRpcMethodCallInner>,
}

impl<I: Into<Vec<u8>>, Source: Stream<Item = I> + Unpin> Stream for Server<Source> {
    type Item = JsonRpcResponse;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.running_futures.len() >= self.max_parallel_requests {
            // We cannot accept any more elements from the stream atm. So we just ignore polling the stream
            // when woken. If we ignored a wake for the stream, we will resume after a future has finished.
            #[rustfmt::skip]
            assert!(self.max_parallel_requests > 0, "Cannot have 0 parallel futures.");
            let fut_running = pin!(&mut self.running_futures);
            let Some(response) = ready!(fut_running.poll_next(cx)) else {
                panic!("We had at least one future in the FuturesUnordered. This is a bug.")
            };

            // We need to poll the stream here again. While FuturesUnordered is "full", we might have ignored
            // a poll for the stream, so we check now. Since we know we just got one result out, the next
            // call can actually add it to the set.
            cx.waker().clone().wake();
            if response.is_some() {
                return Poll::Ready(response);
            } else {
                return Poll::Pending;
            }
        }

        let fut_stream = pin!(&mut self.source);

        match ready!(fut_stream.poll_next(cx)) {
            Some(Ok(mut next)) => {
                // Allow the user to add custom behaviour here (spawning, tracing, timeout, etc.).
                if let Some(mapper) = self.mapper {
                    next.fut = mapper(next.fut);
                }
                self.running_futures.push(next);
                // Continue to poll running futures
            }
            Some(Err(response)) => {
                return Poll::Ready(Some(JsonRpcResponse::Single(response)));
            }
            None => {
                // Make sure all started futures run to completion before ending stream.
                if self.running_futures.is_empty() {
                    return Poll::Ready(None);
                }
            }
        }

        let fut_running = pin!(&mut self.running_futures);
        match ready!(fut_running.poll_next(cx)) {
            Some(Some(response)) => Poll::Ready(Some(response)),
            Some(None) => {
                // If the last message is a notification, the output will never
                // be closed if we respond with pending
                cx.waker().clone().wake();
                Poll::Pending
            }
            None => Poll::Ready(None),
        }
    }
}

/// Deserializes the `params` segment of a [`JsonRpcRequest`] into a concrete type `T`.
///
/// Produces either:
/// * `Ok(T)` – successful typed parsing
/// * `Err(JsonRpcResponseSingle)` – error response using `Parse error` (-32700) or
///   `Invalid Request` (-32600), depending on serde classification.
///
/// # Example
/// ```rust
/// # use serde::Deserialize;
/// # use serde_json::from_str;
/// # use jsonrpc_rs::server::{JsonRpcRequest, parse_params};
/// #[derive(Deserialize)]
/// struct Args { a: i32, b: i32 }
///
/// let req: JsonRpcRequest = from_str(r#"{"jsonrpc":"2.0","method":"m","params":{"a":1,"b":2},"id":1}"#).unwrap();
/// let parsed: Args = parse_params(&req).unwrap();
/// assert_eq!(parsed.a + parsed.b, 3);
/// ```
pub fn parse_params<T: DeserializeOwned>(req: &JsonRpcRequest) -> Result<T, JsonRpcResponseSingle> {
    // ToDo: Consider using serde_path_to_error here.
    serde_json::from_str(req.param_str()).map_err(parse_error)
}

/// Converts a serde JSON error into a JSON-RPC error response.
///
/// Classification:
/// * `Data` → `Invalid Request` (-32600)
/// * Other → `Parse error` (-32700)
///
/// Private helper used by [`parse_params`].
fn parse_error(error: serde_json::Error) -> JsonRpcResponseSingle {
    match error.classify() {
        serde_json::error::Category::Data => JsonRpcResponseSingle {
            jsonrpc: MustBe!("2.0"),
            result: JsonRpcResult::Err {
                code: JsonRpcErrorCode::invalid_request(),
                message: "Invalid Request".to_string(),
                data: Some(Box::new(json!({
                    "error": error.to_string()
                }))),
            },
            id: Value::Null,
        },
        _ => JsonRpcResponseSingle {
            jsonrpc: MustBe!("2.0"),
            result: JsonRpcResult::Err {
                code: JsonRpcErrorCode::parse_error(),
                message: "Parse error".to_string(),
                data: Some(Box::new(json!({
                    "error": error.to_string()
                }))),
            },
            id: Value::Null,
        },
    }
}

/// Constructs a "Method not found" error response for the given method name.
///
/// Private helper used in dispatch logic.
fn method_not_found(method: String) -> JsonRpcResponseSingle {
    JsonRpcResponseSingle {
        jsonrpc: MustBe!("2.0"),
        result: JsonRpcResult::Err {
            code: JsonRpcErrorCode::method_not_found(),
            message: "Method not found".to_string(),
            data: Some(Box::new(json!({"method": method}))),
        },
        id: Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{StreamExt, stream};
    use monostate::MustBe;
    use serde::Deserialize;
    use serde_json::json;

    use super::{
        JsonRpcErrorCode, JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponseSingle, JsonRpcResult,
        ServerBuilder, ServerState, parse_params,
    };

    async fn rpc_test_method_subtract(lhs: i32, rhs: i32) -> i32 {
        lhs - rhs
    }

    fn rpc_test_method_subtract_dispatch<'a>(
        _: &ServerState,
        request: &JsonRpcRequest<'a>,
    ) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
        #[derive(Deserialize)]
        struct Params {
            lhs: i32,
            rhs: i32,
        }

        let params: Params = parse_params(&request)?;
        Ok(Box::new(async move {
            let result = rpc_test_method_subtract(params.lhs, params.rhs).await;
            JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(result))).into()
        }))
    }

    #[test]
    fn should_deserialize_basic_request() {
        let request_str =
            r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_str).unwrap();
        assert_eq!(request.method.as_str(), "subtract");
        assert_eq!(request.param_str(), "[42, 23]")
    }

    #[test]
    fn should_serialize_result() {
        let response = JsonRpcResponseSingle {
            jsonrpc: MustBe!("2.0"),
            result: JsonRpcResult::Ok(Box::new(19)),
            id: json!(1),
        };

        let response_str = serde_json::to_string(&response).unwrap();
        assert_eq!(
            response_str.as_str(),
            r#"{"jsonrpc":"2.0","result":19,"id":1}"#
        );
    }

    #[test]
    fn should_serialize_error() {
        let error_response = JsonRpcResponseSingle {
            jsonrpc: MustBe!("2.0"),
            result: JsonRpcResult::Err {
                code: JsonRpcErrorCode::method_not_found(),
                message: "Method not found".to_string(),
                data: None,
            },
            id: json!("1"),
        };
        let response_str = serde_json::to_string(&error_response).unwrap();
        assert_eq!(
            response_str.as_str(),
            r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":"1"}"#
        );
    }

    #[tokio::test]
    async fn should_run_method() {
        let source = [
            r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#.to_owned(),
            r#"{"jsonrpc": "2.0", "method": "subtract", "params": [23, 42], "id": 2}"#.to_owned(),
        ];

        let mut server = ServerBuilder::new()
            .register("subtract".to_string(), &rpc_test_method_subtract_dispatch)
            .max_parallel_requests(2)
            .map_requests(&|fut| Box::pin(async move { tokio::spawn(fut).await.unwrap() }))
            .build(stream::iter(source.into_iter()));

        let result1 = server.next().await.unwrap();
        let result2 = server.next().await.unwrap();
        let result3 = server.next().await;

        let result1_str = serde_json::to_string(&result1).unwrap();
        let result2_str = serde_json::to_string(&result2).unwrap();
        assert_eq!(result1_str, r#"{"jsonrpc":"2.0","result":19,"id":1}"#);
        assert_eq!(result2_str, r#"{"jsonrpc":"2.0","result":-19,"id":2}"#);
        assert!(result3.is_none());
    }

    #[tokio::test]
    async fn should_return_state() {
        let mut state = ServerState::new();
        state.manage(1u32);

        let Some(arc) = state.get_arc_lock::<u32>() else {
            unreachable!("Arc was just managed.");
        };
        std::mem::drop(state);
        let lock = arc.read().await;
        assert_eq!(*lock, 1);
    }
}
