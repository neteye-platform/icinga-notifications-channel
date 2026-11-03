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
};

use async_lock::RwLock;
use erased_serde::Serialize as DynSerialize;
use futures_util::{Sink, SinkExt, Stream, StreamExt, ready, select_biased};
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
    /// }"#).unwrap();
    /// assert_eq!(req.param_str(), r#"{ "a": 1 }"#);
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
pub type JsonRpcMethod = &'static (
             dyn for<'server, 'request> Fn(
    &'server ServerState,
    &'request JsonRpcRequest<'request>,
) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle>
                 + Sync
         );

/// Wraps a method future, see [`ServerBuilder::map_requests`].
pub type RequestMapper = &'static (dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall> + Sync);

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
/// are reused, or the config is reused for several connections.
#[derive(Clone)]
pub struct ServerBuilder {
    registered_methods: HashMap<String, JsonRpcMethod>,
    managed_state: ServerState,
    mappers: Vec<RequestMapper>,
    max_parallel_requests: usize,
}

impl ServerBuilder {
    /// Creates a new empty builder with:
    /// * No registered methods
    /// * No mappers
    /// * Fresh `ServerState`
    /// * Unlimited parallel requests (`usize::MAX`)
    pub fn new() -> Self {
        Self {
            registered_methods: HashMap::new(),
            mappers: Vec::new(),
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
    /// processing. To keep parallel execution but send responses in request order,
    /// enable the `ordered` feature instead.
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
    ///
    /// Can be called several times. Mappers are applied in call order, so the
    /// last one registered is the outermost wrapper.
    pub fn map_requests(mut self, mapper: RequestMapper) -> Self {
        self.mappers.push(mapper);
        self
    }

    /// Injects a managed state value accessible to methods via the macro
    /// (`#[jsonrpc(state = "...")]`) or manually through
    /// [`ServerState::get_arc_lock`].
    pub fn manage<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.managed_state.manage(state);
        self
    }

    /// Finalizes the builder into a [`Server`].
    ///
    /// The server holds only configuration; drive a connection with
    /// [`Server::serve`]. A single `Server` can serve several connections.
    pub fn build(self) -> Server {
        Server {
            registered_methods: self.registered_methods,
            state: self.managed_state,
            mappers: self.mappers,
            max_parallel_requests: self.max_parallel_requests,
        }
    }
}

/// Wrapper holding a future plus its optional request `id`.
///
/// Used to re-associate the `id` after the method future resolves.
struct JsonRpcMethodCallInner {
    id: Option<Value>,
    fut: Pin<JsonRpcMethodCall>,
}

impl JsonRpcMethodCallInner {
    /// A call that immediately resolves to `error`, answered with a `null` id.
    fn error(error: JsonRpcResponseSingle) -> Self {
        Self {
            id: Some(Value::Null),
            fut: Box::pin(async { JsonRpcResponse::Single(error) }),
        }
    }
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

/// Set of in-flight requests. Response order follows the `ordered` feature:
/// completion order by default, request order when enabled.
#[cfg(not(feature = "ordered"))]
type RunningCalls = futures_util::stream::FuturesUnordered<JsonRpcMethodCallInner>;
#[cfg(feature = "ordered")]
type RunningCalls = futures_util::stream::FuturesOrdered<JsonRpcMethodCallInner>;

/// A JSON-RPC server: registered methods, managed state and execution settings.
///
/// Created by [`ServerBuilder::build`]. Call [`Server::serve`] to process one
/// connection.
#[derive(Clone)]
pub struct Server {
    registered_methods: HashMap<String, JsonRpcMethod>,
    state: ServerState,
    mappers: Vec<RequestMapper>,
    max_parallel_requests: usize,
}

impl Server {
    /// Reads requests from `source`, runs them, and writes the responses to `sink`.
    ///
    /// Each item of `source` must be one complete JSON-RPC entity (request or
    /// batch array). Notifications produce no response. At most
    /// [`ServerBuilder::max_parallel_requests`] requests run at the same time.
    /// Responses are sent in the order they complete, or in request order
    /// with the `ordered` feature.
    ///
    /// Returns when `source` has ended and every request it started has been
    /// answered. The sink is then closed. Returns early if the sink fails.
    pub async fn serve<I, Source, Output>(
        &self,
        source: Source,
        sink: Output,
    ) -> Result<(), Output::Error>
    where
        I: Into<Vec<u8>>,
        Source: Stream<Item = I>,
        Output: Sink<JsonRpcResponse>,
    {
        assert!(
            self.max_parallel_requests > 0,
            "Cannot have 0 parallel futures."
        );
        let mut source = pin!(source.fuse());
        let mut sink = pin!(sink);
        let mut running = RunningCalls::new();

        loop {
            // When full, stop reading from the source until a request finishes.
            let response = if running.len() >= self.max_parallel_requests {
                running.next().await.flatten()
            } else {
                select_biased! {
                    response = running.select_next_some() => response,
                    request = source.next() => match request {
                        Some(request) => {
                            running.extend(self.handle(&request.into()));
                            None
                        }
                        None => break,
                    },
                }
            };
            if let Some(response) = response {
                sink.send(response).await?;
            }
        }

        // The source has ended: finish all requests that are still running.
        while let Some(response) = running.next().await {
            if let Some(response) = response {
                sink.send(response).await?;
            }
        }
        sink.close().await
    }

    /// Turns one raw source item into a call to run.
    ///
    /// Errors that are known immediately become calls that resolve to the
    /// error response, so every response goes through the same path.
    /// Returns `None` for a failed notification, which has no response.
    fn handle(&self, raw: &[u8]) -> Option<JsonRpcMethodCallInner> {
        // ToDo: Consider using serde_path_to_error here.
        let mut call = if raw.starts_with(b"[") {
            self.process_batch(raw)
        } else {
            self.process_single(raw)?
        };
        // Allow the user to add custom behaviour here (spawning, tracing, timeout, etc.).
        for mapper in &self.mappers {
            call.fut = mapper(call.fut);
        }
        Some(call)
    }

    fn process_single(&self, raw: &[u8]) -> Option<JsonRpcMethodCallInner> {
        let request = match serde_json::from_slice::<JsonRpcRequest>(raw) {
            Ok(request) => request,
            Err(error) => return Some(JsonRpcMethodCallInner::error(parse_error(error))),
        };

        match self.dispatch(&request) {
            Ok(fut) => Some(JsonRpcMethodCallInner {
                fut: Box::into_pin(fut),
                id: request.id,
            }),
            Err(err) => request.id.map(|id| JsonRpcMethodCallInner {
                id: Some(id),
                fut: Box::pin(async { JsonRpcResponse::Single(err) }),
            }),
        }
    }

    fn process_batch(&self, raw: &[u8]) -> JsonRpcMethodCallInner {
        let requests = match serde_json::from_slice::<Vec<&RawValue>>(raw) {
            Ok(request) => request,
            Err(error) => return JsonRpcMethodCallInner::error(parse_error(error)),
        };

        if requests.is_empty() {
            return JsonRpcMethodCallInner::error(JsonRpcResponseSingle::new(JsonRpcResult::Err {
                code: JsonRpcErrorCode::invalid_request(),
                message: "Invalid Request".to_owned(),
                data: None,
            }));
        }

        let mut calls = Vec::with_capacity(requests.len());
        for req in requests {
            let request = match serde_json::from_str::<JsonRpcRequest>(req.get()) {
                Ok(request) => request,
                Err(error) => {
                    calls.push(JsonRpcMethodCallInner::error(parse_error(error)));
                    continue;
                }
            };

            let fut = match self.dispatch(&request) {
                Ok(call) => Box::into_pin(call),
                Err(error) => Box::pin(async { JsonRpcResponse::Single(error) }),
            };
            calls.push(JsonRpcMethodCallInner {
                id: request.id,
                fut,
            });
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

        JsonRpcMethodCallInner {
            id: None,
            fut: future,
        }
    }

    fn dispatch(&self, req: &JsonRpcRequest) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
        let Some(method_call) = self.registered_methods.get(&req.method) else {
            return Err(method_not_found(req.method.clone()));
        };

        method_call(&self.state, req)
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
    pub fn manage<T: Send + Sync + 'static>(&mut self, state: T) {
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
/// let Ok(parsed) = parse_params::<Args>(&req) else {
///     panic!();
/// };
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
    use futures_util::stream;
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

        let server = ServerBuilder::new()
            .register("subtract".to_string(), &rpc_test_method_subtract_dispatch)
            .max_parallel_requests(2)
            .map_requests(&|fut| Box::pin(async move { tokio::spawn(fut).await.unwrap() }))
            .build();

        let mut responses = Vec::new();
        server
            .serve(stream::iter(source), &mut responses)
            .await
            .unwrap();

        let [result1, result2] = <[_; 2]>::try_from(responses).ok().unwrap();

        let result1_str = serde_json::to_string(&result1).unwrap();
        let result2_str = serde_json::to_string(&result2).unwrap();
        assert_eq!(result1_str, r#"{"jsonrpc":"2.0","result":19,"id":1}"#);
        assert_eq!(result2_str, r#"{"jsonrpc":"2.0","result":-19,"id":2}"#);
    }

    #[tokio::test]
    async fn should_chain_mappers() {
        fn ok(value: i32) -> super::JsonRpcResponse {
            JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(value))).into()
        }
        let source = [r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#];

        // Each mapper replaces the result; the last registered wraps outermost, so it wins.
        let server = ServerBuilder::new()
            .register("subtract".to_string(), &rpc_test_method_subtract_dispatch)
            .map_requests(&|fut| {
                Box::pin(async move {
                    fut.await;
                    ok(1)
                })
            })
            .map_requests(&|fut| {
                Box::pin(async move {
                    fut.await;
                    ok(2)
                })
            })
            .build();

        let mut responses = Vec::new();
        server
            .serve(stream::iter(source), &mut responses)
            .await
            .unwrap();

        let response = serde_json::to_string(&responses[0]).unwrap();
        assert_eq!(response, r#"{"jsonrpc":"2.0","result":2,"id":1}"#);
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
