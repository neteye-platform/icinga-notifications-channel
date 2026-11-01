use std::{
    collections::HashMap,
    pin::{Pin, pin},
    task::Poll,
    usize,
};

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
    params: &'request RawValue,
    id: Option<Value>,
}

impl JsonRpcRequest<'_> {
    pub fn param_str(&self) -> &str {
        self.params.get()
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
pub struct JsonRpcResponse {
    jsonrpc: MustBe!("2.0"),
    #[serde(flatten)]
    result: JsonRpcResult,
    id: Value,
}

impl JsonRpcResponse {
    pub fn new(result: JsonRpcResult) -> Self {
        Self {
            jsonrpc: MustBe!("2.0"),
            result,
            id: Value::Null,
        }
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
    &'static dyn Fn(&JsonRpcRequest<'_>) -> Result<JsonRpcMethodCall, JsonRpcResponse>;

pub struct ServerBuilder {
    registered_methods: HashMap<String, JsonRpcMethod>,
    mapper: Option<&'static dyn Fn(Pin<JsonRpcMethodCall>) -> Pin<JsonRpcMethodCall>>,
    max_parallel_requests: usize,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self {
            registered_methods: HashMap::new(),
            mapper: None,
            max_parallel_requests: usize::MAX,
        }
    }

    pub fn register(mut self, name: String, method: JsonRpcMethod) -> Self {
        self.registered_methods.insert(name, method);
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

    pub fn build<Source: Stream<Item = String> + Unpin>(self, source: Source) -> Server<Source> {
        let dispatcher = Dispatcher {
            registered_methods: self.registered_methods,
            source: source,
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
        let mut res = ready!(fut.poll(cx));
        match self.id.as_mut() {
            Some(id) => {
                res.id = std::mem::take(id);
                Poll::Ready(Some(res))
            }
            None => Poll::Ready(None),
        }
    }
}

impl<Source> Dispatcher<Source> {
    fn dispatch(&self, req: &JsonRpcRequest) -> Result<JsonRpcMethodCall, JsonRpcResponse> {
        let Some(method_call) = self.registered_methods.get(&req.method) else {
            return Err(method_not_found(req.method.clone()));
        };

        method_call(&req)
    }
}

impl<Source: Stream<Item = String> + Unpin> Stream for Dispatcher<Source> {
    type Item = Result<JsonRpcMethodCallInner, JsonRpcResponse>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let f = pin!(&mut self.source);
        let Some(next) = ready!(f.poll_next(cx)) else {
            return Poll::Ready(None);
        };

        // ToDo: Consider using serde_path_to_error here.
        let request = match serde_json::from_str::<JsonRpcRequest>(&next) {
            Ok(request) => request,
            Err(error) => return Poll::Ready(Some(Err(parse_error(error)))),
        };

        match self.dispatch(&request) {
            Ok(fut) => Poll::Ready(Some(Ok(JsonRpcMethodCallInner {
                fut: Box::into_pin(fut),
                id: request.id,
            }))),
            Err(mut err) => {
                if let Some(id) = request.id {
                    err.id = id;
                    Poll::Ready(Some(Err(err)))
                } else {
                    Poll::Pending
                }
            }
        }
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
                return Poll::Ready(Some(response));
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

pub fn parse_params<T: DeserializeOwned>(req: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    // ToDo: Consider using serde_path_to_error here.
    serde_json::from_str(req.params.get()).map_err(parse_error)
}

fn parse_error(error: serde_json::Error) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: MustBe!("2.0"),
        result: JsonRpcResult::Err {
            code: JsonRpcErrorCode::parse_error(),
            message: error.to_string(),
            data: None,
        },
        id: Value::Null,
    }
}

fn method_not_found(method: String) -> JsonRpcResponse {
    JsonRpcResponse {
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
        JsonRpcErrorCode, JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponse, JsonRpcResult,
        ServerBuilder, parse_params,
    };

    async fn rpc_test_method_subtract(lhs: i32, rhs: i32) -> i32 {
        lhs - rhs
    }

    fn rpc_test_method_subtract_dispatch<'a>(
        request: &JsonRpcRequest<'a>,
    ) -> Result<JsonRpcMethodCall, JsonRpcResponse> {
        #[derive(Deserialize)]
        struct Params {
            lhs: i32,
            rhs: i32,
        }

        let params: Params = parse_params(&request)?;
        // ToDo: Find a better way to handle Notifications
        Ok(Box::new(async move {
            let result = rpc_test_method_subtract(params.lhs, params.rhs).await;
            JsonRpcResponse::new(JsonRpcResult::Ok(Box::new(result)))
        }))
    }

    #[test]
    pub fn should_deserialize_basic_request() {
        let request_str =
            r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_str).unwrap();
        assert_eq!(request.method.as_str(), "subtract");
        assert_eq!(request.params.get(), "[42, 23]")
    }

    #[test]
    pub fn should_serialize_result() {
        let response = JsonRpcResponse {
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
        let error_response = JsonRpcResponse {
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
}
