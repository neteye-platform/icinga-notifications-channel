//! The examples from the JSON-RPC 2.0 specification
//! (<https://www.jsonrpc.org/specification#examples>), plus a few custom cases.
//!
//! Each request is served on its own, like one connection per request.
//! `error.data` is implementation-specific and removed before comparing.
use futures_util::stream;
use jsonrpc_rs::{
    jsonrpc,
    server::{JsonRpcErrorCode, JsonRpcResult, Server, ServerBuilder},
    tokio::tokio_parallel,
};
use serde_json::{Value, json};

struct ManagedState {
    state: (String, i32),
}

#[jsonrpc]
fn subtract(minuend: i32, subtrahend: i32) -> i32 {
    minuend - subtrahend
}

#[jsonrpc(params = "params")]
fn sum(params: Vec<i32>) -> i32 {
    params.into_iter().sum()
}

#[jsonrpc(state = "state", name = "get_data")]
async fn state_managed(state: &ManagedState) -> (String, i32) {
    state.state.clone()
}

struct DivByZeroError;

impl From<DivByZeroError> for JsonRpcResult {
    fn from(_: DivByZeroError) -> Self {
        JsonRpcResult::Err {
            code: JsonRpcErrorCode::custom_error(-32001),
            message: "Cannot divide by zero!".to_owned(),
            data: None,
        }
    }
}

#[jsonrpc]
fn div(divident: i32, divisor: i32) -> Result<f32, DivByZeroError> {
    if divisor == 0 {
        return Err(DivByZeroError);
    }
    Ok(divident as f32 / divisor as f32)
}

#[jsonrpc]
fn internal_server_error() {
    panic!("intentional panic")
}

fn server() -> Server {
    ServerBuilder::new()
        .manage(ManagedState {
            state: ("hello".to_string(), 5),
        })
        .register_named(subtract)
        .register_named(sum)
        .register_named(div)
        .register_named(state_managed)
        .register_named(internal_server_error)
        .map_requests(&tokio_parallel)
        .build()
}

/// Serves `request` and returns the response, or `None` if there was none.
async fn call(request: &str) -> Option<Value> {
    let mut responses = Vec::new();
    server()
        .serve(stream::iter([request]), &mut responses)
        .await
        .unwrap();
    assert!(responses.len() <= 1, "one request, several responses");

    let mut response = serde_json::to_value(responses.pop()?).unwrap();
    match &mut response {
        Value::Array(batch) => batch.iter_mut().for_each(strip_error_data),
        single => strip_error_data(single),
    }
    Some(response)
}

fn strip_error_data(response: &mut Value) {
    if let Some(error) = response.get_mut("error").and_then(Value::as_object_mut) {
        error.remove("data");
    }
}

fn invalid_request() -> Value {
    json!({"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null})
}

fn parse_error() -> Value {
    json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}, "id": null})
}

#[tokio::test]
async fn positional_parameters() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#).await,
        Some(json!({"jsonrpc": "2.0", "result": 19, "id": 1}))
    );
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "subtract", "params": [23, 42], "id": 2}"#).await,
        Some(json!({"jsonrpc": "2.0", "result": -19, "id": 2}))
    );
}

#[tokio::test]
async fn named_parameters() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "subtract", "params": {"subtrahend": 23, "minuend": 42}, "id": 3}"#).await,
        Some(json!({"jsonrpc": "2.0", "result": 19, "id": 3}))
    );
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "subtract", "params": {"minuend": 42, "subtrahend": 23}, "id": 4}"#).await,
        Some(json!({"jsonrpc": "2.0", "result": 19, "id": 4}))
    );
}

#[tokio::test]
async fn notification() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "update", "params": [1,2,3,4,5]}"#).await,
        None
    );
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "foobar"}"#).await,
        None
    );
}

#[tokio::test]
async fn non_existent_method() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "foobar", "id": "1"}"#).await,
        Some(
            json!({"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "1"})
        )
    );
}

#[tokio::test]
async fn invalid_json() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "foobar, "params": "bar", "baz]"#).await,
        Some(parse_error())
    );
}

#[tokio::test]
async fn invalid_request_object() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": 1, "params": "bar"}"#).await,
        Some(invalid_request())
    );
}

#[tokio::test]
async fn batch_invalid_json() {
    assert_eq!(
        call(r#"[{"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},{"jsonrpc": "2.0", "method"]"#).await,
        Some(parse_error())
    );
}

#[tokio::test]
async fn empty_batch() {
    assert_eq!(call("[]").await, Some(invalid_request()));
}

#[tokio::test]
async fn invalid_batch() {
    assert_eq!(call("[1]").await, Some(json!([invalid_request()])));
    assert_eq!(
        call("[1,2,3]").await,
        Some(json!([
            invalid_request(),
            invalid_request(),
            invalid_request()
        ]))
    );
}

#[tokio::test]
async fn batch() {
    let request = r#"[
        {"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},
        {"jsonrpc": "2.0", "method": "notify_hello", "params": [7]},
        {"jsonrpc": "2.0", "method": "subtract", "params": [42,23], "id": "2"},
        {"foo": "boo"},
        {"jsonrpc": "2.0", "method": "foo.get", "params": {"name": "myself"}, "id": "5"},
        {"jsonrpc": "2.0", "method": "get_data", "id": "9"}
    ]"#;
    assert_eq!(
        call(request).await,
        Some(json!([
            {"jsonrpc": "2.0", "result": 7, "id": "1"},
            {"jsonrpc": "2.0", "result": 19, "id": "2"},
            invalid_request(),
            {"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "5"},
            {"jsonrpc": "2.0", "result": ["hello", 5], "id": "9"}
        ]))
    );
}

#[tokio::test]
async fn batch_all_notifications() {
    assert_eq!(
        call(r#"[{"jsonrpc": "2.0", "method": "notify_sum", "params": [1,2,4]},{"jsonrpc": "2.0", "method": "notify_hello", "params": [7]}]"#).await,
        None
    );
}

#[tokio::test]
async fn custom_error() {
    assert_eq!(
        call(r#"[{"jsonrpc": "2.0", "method": "div", "params": [1,2], "id":1},{"jsonrpc": "2.0", "method": "div", "params": [1,0], "id":2}]"#).await,
        Some(json!([
            {"jsonrpc": "2.0", "result": 0.5, "id": 1},
            {"jsonrpc": "2.0", "error": {"code": -32001, "message": "Cannot divide by zero!"}, "id": 2}
        ]))
    );
}

#[tokio::test]
async fn panic_becomes_internal_error() {
    assert_eq!(
        call(r#"{"jsonrpc": "2.0", "method": "internal_server_error", "id": 1}"#).await,
        Some(
            json!({"jsonrpc": "2.0", "error": {"code": -32603, "message": "Internal server error"}, "id": 1})
        )
    );
}
