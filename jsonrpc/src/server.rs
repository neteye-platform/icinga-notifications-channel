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
pub struct JsonRpcRequest<'request> {
    #[allow(dead_code)]
    jsonrpc: MustBe!("2.0"),
    method: String,
    #[serde(borrow)]
    params: Option<&'request RawValue>,
    id: Option<Value>,
}

impl JsonRpcRequest<'_> {
    pub fn param_str(&self) -> &str {
        // If no params are given, default to an empty object
        self.params.map(|r| r.get()).unwrap_or("{}")
    }

    pub fn id(&self) -> Option<&Value> {
        self.id.as_ref()
    }
}

#[derive(Serialize)]
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
pub struct JsonRpcResponseSingle {
    jsonrpc: MustBe!("2.0"),
    #[serde(flatten)]
    result: JsonRpcResult,
    id: Value,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Single(JsonRpcResponseSingle),
    Bulk(Vec<JsonRpcResponseSingle>),
}

impl JsonRpcResponseSingle {
    pub fn new(result: JsonRpcResult) -> Self {
        Self {
            jsonrpc: MustBe!("2.0"),
            result,
            id: Value::Null,
        }
    }
}

impl From<JsonRpcResponseSingle> for JsonRpcResponse {
    fn from(value: JsonRpcResponseSingle) -> Self {
        JsonRpcResponse::Single(value)
    }
}

#[derive(Serialize)]
pub struct JsonRpcErrorCode(i32);

impl JsonRpcErrorCode {
    pub const fn error_code(&self) -> i32 {
        self.0
    }

    pub const fn parse_error() -> Self {
        Self(-32700)
    }

    pub const fn invalid_request() -> Self {
        Self(-32600)
    }

    pub const fn method_not_found() -> Self {
        Self(-32601)
    }

    pub const fn invalid_params() -> Self {
        Self(-32602)
    }

    pub const fn internal_error() -> Self {
        Self(-32603)
    }

    pub const fn server_error(error_code: i32) -> Self {
        Self(error_code)
    }

    pub const fn custom_error(error_code: i32) -> Self {
        Self(error_code)
    }
}

pub type JsonRpcMethodCall = Box<dyn Future<Output = JsonRpcResponse> + 'static + Send>;
pub type JsonRpcMethod =
    &'static dyn for<'server, 'request> Fn(
        &'server ServerState,
        &'request JsonRpcRequest<'request>,
    ) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle>;

pub struct JsonRpcNamedMethod {
    name: &'static str,
    method: JsonRpcMethod,
}

impl JsonRpcNamedMethod {
    pub const fn new(name: &'static str, method: JsonRpcMethod) -> Self {
        Self { name, method }
    }
}

#[derive(Clone)]
pub struct ServerBuilder {
    registered_methods: HashMap<String, JsonRpcMethod>,
    managed_state: ServerState,
    mapper: Option<&'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>>,
    max_parallel_requests: usize,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self {
            registered_methods: HashMap::new(),
            mapper: None,
            managed_state: ServerState::new(),
            max_parallel_requests: usize::MAX,
        }
    }

    pub fn register(mut self, name: String, method: JsonRpcMethod) -> Self {
        self.registered_methods.insert(name, method);
        self
    }

    pub fn register_named(mut self, named: JsonRpcNamedMethod) -> Self {
        self.registered_methods
            .insert(named.name.to_string(), named.method);
        self
    }

    pub fn max_parallel_requests(mut self, max_parallel_requests: usize) -> Self {
        self.max_parallel_requests = max_parallel_requests;
        self
    }

    pub fn map_requests(
        mut self,
        mapper: &'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>,
    ) -> Self {
        self.mapper = Some(mapper);
        self
    }

    pub fn manage<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.managed_state.manage(state);
        self
    }

    pub fn build<Source: Stream<Item = String> + Unpin>(self, source: Source) -> Server<Source> {
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

struct Dispatcher<Source> {
    registered_methods: HashMap<String, JsonRpcMethod>,
    state: ServerState,
    source: Source,
}

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
        next: &str,
    ) -> Option<Result<JsonRpcMethodCallInner, JsonRpcResponseSingle>> {
        let request = match serde_json::from_str::<JsonRpcRequest>(&next) {
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

    fn process_batch(&self, next: &str) -> Result<JsonRpcMethodCallInner, JsonRpcResponseSingle> {
        let requests = match serde_json::from_str::<Vec<&RawValue>>(next) {
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

impl<Source: Stream<Item = String> + Unpin> Stream for Dispatcher<Source> {
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
        if next.starts_with("[") {
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
pub struct ServerState {
    managed_state: Vec<Arc<dyn Any + Send + Sync + 'static>>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            managed_state: Default::default(),
        }
    }
}

impl ServerState {
    fn new() -> Self {
        Default::default()
    }

    fn manage<T: Send + Sync + 'static>(&mut self, state: T) {
        // ToDo: Check if the type is already managed?
        let arc = Arc::new(RwLock::new(state));
        self.managed_state.push(arc);
    }

    pub fn get_arc_lock<T: Send + Sync + 'static>(&self) -> Option<Arc<RwLock<T>>> {
        for state in &self.managed_state {
            if let Ok(lock) = state.clone().downcast::<RwLock<T>>() {
                return Some(lock);
            }
        }
        return None;
    }
}

pub struct Server<Source> {
    // We will continue to poll the source while we are finishing the requests, so -
    source: Fuse<Dispatcher<Source>>,
    mapper: Option<&'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>>,
    max_parallel_requests: usize,
    // ToDo: Can we let the user choose between ordered and unordered?
    running_futures: FuturesUnordered<JsonRpcMethodCallInner>,
}

impl<Source: Stream<Item = String> + Unpin> Stream for Server<Source> {
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
                // Allow the user to add custom behaviour here, e.g. spawning the future as a task for
                // parallelism, or adding tracing, etc.
                if let Some(mapper) = self.mapper {
                    next.fut = mapper(next.fut);
                }
                self.running_futures.push(next);
                // Make sure to call poll_next on the running futures
            }
            Some(Err(response)) => {
                return Poll::Ready(Some(JsonRpcResponse::Single(response)));
            }
            None => {
                // Make sure all started futures run to their end to avoid cancel issues.
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

pub fn parse_params<T: DeserializeOwned>(req: &JsonRpcRequest) -> Result<T, JsonRpcResponseSingle> {
    // ToDo: Consider using serde_path_to_error here.
    serde_json::from_str(req.param_str()).map_err(parse_error)
}

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

    use crate::server::{
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
    pub fn should_deserialize_basic_request() {
        let request_str =
            r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_str).unwrap();
        assert_eq!(request.method.as_str(), "subtract");
        assert_eq!(request.param_str(), "[42, 23]")
    }

    #[test]
    pub fn should_serialize_result() {
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
    pub fn should_serialize_error() {
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
    pub async fn should_run_method() {
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
