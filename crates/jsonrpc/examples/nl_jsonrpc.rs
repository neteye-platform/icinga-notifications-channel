use jsonrpc_rs::{
    jsonrpc,
    server::{JsonRpcErrorCode, JsonRpcResult, ServerBuilder},
    tokio::tokio_parallel,
};

struct ManagedState {
    state: (String, i32),
}

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();
    let server = ServerBuilder::new()
        .manage(ManagedState {
            state: ("hello".to_string(), 5),
        })
        .register_named(subtract)
        .register_named(sum)
        .register_named(div)
        .register_named(state_managed)
        .register_named(internal_server_error)
        .map_requests(&tokio_parallel)
        .build();

    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(server.serve_with_codec(codec, tokio::io::stdin(), tokio::io::stdout()))
        .unwrap();
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
    panic!()
}
