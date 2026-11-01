use jsonrpc_rs::{
    jsonrpc,
    server::ServerBuilder,
    tokio::{BuilderTokioServerExt, TokioServer},
};

struct ManagedState {
    state: (String, i32),
}

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();
    let server: TokioServer<_, String, _, _, _> = ServerBuilder::new()
        .manage(ManagedState {
            state: ("hello".to_string(), 5),
        })
        .register_named(subtract)
        .register_named(sum)
        .register_named(state_managed)
        .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());

    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(server)
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
