use jsonrpc_rs::{
    server::{
        JsonRpcErrorCode, JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponseSingle, JsonRpcResult,
        ServerBuilder, ServerState,
    },
    tokio::BuilderTokioServerExt,
};
use serde::Deserialize;

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();
    let server = ServerBuilder::new()
        .manage(("hello".to_string(), 5))
        .register("subtract".to_string(), &rpc_test_method_subtract_dispatch)
        .register("sum".to_string(), &rpc_test_method_sum_dispatch)
        .register("get_data".to_string(), &rpc_test_method_get_data_dispatch)
        .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());

    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(server)
        .unwrap();
}

async fn subtract(lhs: i32, rhs: i32) -> i32 {
    lhs - rhs
}

fn rpc_test_method_subtract_dispatch<'a>(
    _: &ServerState,
    request: &JsonRpcRequest<'a>,
) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
    #[derive(Deserialize)]
    struct Params {
        minuend: i32,
        subtrahend: i32,
    }

    let params: Params = jsonrpc_rs::server::parse_params(&request)?;
    Ok(Box::new(async move {
        let result = subtract(params.minuend, params.subtrahend).await;
        JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(result))).into()
    }))
}

async fn sum(l: &[i32]) -> i32 {
    l.iter().sum()
}

fn rpc_test_method_sum_dispatch<'a>(
    _: &ServerState,
    request: &JsonRpcRequest<'a>,
) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
    let params: Vec<i32> = jsonrpc_rs::server::parse_params(&request)?;
    Ok(Box::new(async move {
        let result = sum(&params).await;
        JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(result))).into()
    }))
}

async fn get_data(state: &(String, i32)) -> (String, i32) {
    state.clone()
}

fn rpc_test_method_get_data_dispatch<'a>(
    state: &ServerState,
    _: &JsonRpcRequest<'a>,
) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
    let Some(arc) = state.get_arc_lock::<(String, i32)>() else {
        return Err(JsonRpcResponseSingle::new(JsonRpcResult::Err {
            code: JsonRpcErrorCode::internal_error(),
            message: "Unmanaged state".to_string(),
            data: None,
        }));
    };
    Ok(Box::new(async move {
        let state = arc.read().await;
        let result = get_data(&state).await;
        JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(result))).into()
    }))
}
