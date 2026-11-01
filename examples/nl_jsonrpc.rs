use futures_util::StreamExt;
use jsonrpc_rs::server::{
    JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponseSingle, JsonRpcResult, ServerBuilder,
};
use serde::Deserialize;
use tokio_util::codec::{FramedRead, FramedWrite};

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();

    let framed_reader = FramedRead::new(tokio::io::stdin(), codec.clone()).map(|f| f.unwrap());
    let framed_writer = FramedWrite::new(tokio::io::stdout(), codec);

    let future = ServerBuilder::new()
        .register("subtract".to_string(), &rpc_test_method_subtract_dispatch)
        .register("sum".to_string(), &rpc_test_method_sum_dispatch)
        .register("get_data".to_string(), &rpc_test_method_get_data_dispatch)
        .build(framed_reader)
        .map(|r| Ok(serde_json::to_string(&r).unwrap()))
        .forward(framed_writer);

    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
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
