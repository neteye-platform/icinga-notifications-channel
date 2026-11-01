//! Managed state taken by `&mut` in a `#[jsonrpc]` method is writable and the
//! change is visible to later requests.
use futures_util::stream;
use jsonrpc_rs::{jsonrpc, server::ServerBuilder};

#[derive(Default)]
struct Config {
    url: Option<String>,
}

#[jsonrpc(state = "config", name = "SetConfig")]
async fn set_config(config: &mut Config, url: String) {
    config.url = Some(url);
}

#[jsonrpc(state = "config", name = "GetUrl")]
async fn get_url(config: &Config) -> Option<String> {
    config.url.clone()
}

#[tokio::test]
async fn mutable_state_is_visible_to_later_requests() {
    let server = ServerBuilder::new()
        .manage(Config::default())
        .register_named(set_config)
        .register_named(get_url)
        // Sequential, so the response order matches the request order.
        .max_parallel_requests(1)
        .build();
    let requests = [
        r#"{"jsonrpc":"2.0","method":"GetUrl","id":1}"#,
        r#"{"jsonrpc":"2.0","method":"SetConfig","params":{"url":"http://x"},"id":2}"#,
        r#"{"jsonrpc":"2.0","method":"GetUrl","id":3}"#,
    ];

    let mut responses = Vec::new();
    server
        .serve(stream::iter(requests), &mut responses)
        .await
        .unwrap();

    let responses: Vec<_> = responses
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
    assert_eq!(
        responses,
        [
            r#"{"jsonrpc":"2.0","result":null,"id":1}"#,
            r#"{"jsonrpc":"2.0","result":null,"id":2}"#,
            r#"{"jsonrpc":"2.0","result":"http://x","id":3}"#,
        ]
    );
}
