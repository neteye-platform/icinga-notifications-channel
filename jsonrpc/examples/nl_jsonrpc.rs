use jsonrpc_rs::{
    jsonrpc,
    server::{
        JsonRpcErrorCode, JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponseSingle, JsonRpcResult,
        ServerBuilder, ServerState,
    },
    tokio::BuilderTokioServerExt,
};

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();
    let server = ServerBuilder::new()
        .manage(("hello".to_string(), 5))
        .register_named(subtract)
        .register_named(sum)
        .register("get_data".to_string(), &rpc_test_method_get_data_dispatch)
        .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());

    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(server)
        .unwrap();
}

#[jsonrpc]
async fn subtract(minuend: i32, subtrahend: i32) -> i32 {
    minuend - subtrahend
}

#[jsonrpc]
async fn sum(a: i32, b: i32, c: i32) -> i32 {
    a + b + c
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
