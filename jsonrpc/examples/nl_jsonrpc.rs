use jsonrpc_rs::{jsonrpc, server::ServerBuilder, tokio::BuilderTokioServerExt};

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
        .register_named(state_managed)
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

#[jsonrpc(state = "state", name = "get_data")]
async fn state_managed(state: &ManagedState) -> (String, i32) {
    state.state.clone()
}
