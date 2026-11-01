use jsonrpc_rs::{
    server::{
        JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponseSingle, JsonRpcResult, ServerBuilder,
    },
    tokio::BuilderTokioServerExt,
};
use serde::Deserialize;

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();
    let server = ServerBuilder::new()
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
    request: &JsonRpcRequest<'a>,
) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
    let params: Vec<i32> = jsonrpc_rs::server::parse_params(&request)?;
    Ok(Box::new(async move {
        let result = sum(&params).await;
        JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(result))).into()
    }))
}

async fn get_data() -> (String, i32) {
    ("hello".to_string(), 5)
}

fn rpc_test_method_get_data_dispatch<'a>(
    _: &JsonRpcRequest<'a>,
) -> Result<JsonRpcMethodCall, JsonRpcResponseSingle> {
    Ok(Box::new(async move {
        let result = get_data().await;
        JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(result))).into()
    }))
}
