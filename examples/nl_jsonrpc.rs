use futures_util::StreamExt;
use jsonrpc_rs::server::{
    JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponse, JsonRpcResult, ServerBuilder,
};
use serde::Deserialize;
use tokio_util::codec::{FramedRead, FramedWrite};

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();

    let framed_reader = FramedRead::new(tokio::io::stdin(), codec.clone()).map(|f| f.unwrap());
    let framed_writer = FramedWrite::new(tokio::io::stdout(), codec);

    let future = ServerBuilder::new()
        .register("subtract".to_string(), &rpc_test_method_subtract_dispatch)
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
) -> Result<JsonRpcMethodCall, JsonRpcResponse> {
    #[derive(Deserialize)]
    struct Params {
        lhs: i32,
        rhs: i32,
    }

    let params: Params = jsonrpc_rs::server::parse_params(&request)?;
    Ok(Box::new(async move {
        let result = subtract(params.lhs, params.rhs).await;
        JsonRpcResponse::new(JsonRpcResult::Ok(Box::new(result)))
    }))
}
