//! The JSON-RPC methods `GetInfo`, `SetConfig` and `SendNotification`.

use std::sync::Arc;

use async_lock::RwLock;
use jsonrpc_rs::{
    server::{
        JsonRpcMethodCall, JsonRpcRequest, JsonRpcResponse, JsonRpcResponseSingle, JsonRpcResult,
        Server, ServerBuilder, ServerState, parse_params,
    },
    tokio::tokio_parallel,
};

use crate::{
    channel::{Channel, ChannelConfig},
    error::ChannelError,
    info::{GetInfoResponse, SUPPORTED_REQUEST_VERSIONS},
    notification::{NotificationRequest, NotificationV1},
};

/// The channel and its active configuration, managed by the server.
///
/// The lock around it is only held to read or swap the `Arc`, never while
/// a notification is sent.
struct Shared<C: Channel> {
    channel: Arc<C>,
    config: Option<Arc<C::Config>>,
}

/// Builds the server answering the channel protocol for `channel`.
pub(crate) fn server<C: Channel>(channel: C, max_parallel_requests: usize) -> Server {
    ServerBuilder::new()
        .manage(Shared {
            channel: Arc::new(channel),
            config: None::<Arc<C::Config>>,
        })
        .register("GetInfo".into(), &get_info::<C>)
        .register("SetConfig".into(), &set_config::<C>)
        .register("SendNotification".into(), &send_notification::<C>)
        .max_parallel_requests(max_parallel_requests)
        .map_requests(&tokio_parallel)
        .build()
}

type Handler = Result<JsonRpcMethodCall, JsonRpcResponseSingle>;

fn shared<C: Channel>(state: &ServerState) -> Arc<RwLock<Shared<C>>> {
    state
        .get_arc_lock::<Shared<C>>()
        .expect("the channel state is always managed")
}

fn respond(
    result: Result<impl serde::Serialize + Send + 'static, ChannelError>,
) -> JsonRpcResponse {
    let result = match result {
        Ok(value) => JsonRpcResult::Ok(Box::new(value)),
        Err(error) => error.into(),
    };
    JsonRpcResponse::Single(JsonRpcResponseSingle::new(result))
}

fn get_info<C: Channel>(state: &ServerState, _: &JsonRpcRequest) -> Handler {
    let shared = shared::<C>(state);
    Ok(Box::new(async move {
        let channel = shared.read().await.channel.clone();
        respond(Ok(GetInfoResponse::new(
            channel.info(),
            C::Config::config_attrs(),
        )))
    }))
}

fn set_config<C: Channel>(state: &ServerState, request: &JsonRpcRequest) -> Handler {
    let config: C::Config = parse_params(request)?;
    let shared = shared::<C>(state);
    Ok(Box::new(async move {
        let channel = shared.read().await.channel.clone();
        let result = channel.validate(&config);
        if result.is_ok() {
            // Swap under a short lock; the old config stays on error.
            shared.write().await.config = Some(Arc::new(config));
        }
        respond(result.map(|()| None::<()>))
    }))
}

fn send_notification<C: Channel>(state: &ServerState, request: &JsonRpcRequest) -> Handler {
    let NotificationRequest { version, payload } = parse_params(request)?;
    if !SUPPORTED_REQUEST_VERSIONS.contains(&version) {
        return Err(error_response(ChannelError::new(format!(
            "unsupported request version {version}, supported: {SUPPORTED_REQUEST_VERSIONS:?}"
        ))));
    }
    let notification: NotificationV1 = serde_json::from_str(payload.get())
        .map_err(|e| error_response(ChannelError::with_code(-32602, e.to_string())))?;
    let shared = shared::<C>(state);
    Ok(Box::new(async move {
        let (channel, config) = {
            let shared = shared.read().await;
            (shared.channel.clone(), shared.config.clone())
        };
        let Some(config) = config else {
            return respond(Err::<(), _>(ChannelError::new("channel not configured")));
        };
        respond(channel.send_notification(config, notification).await)
    }))
}

fn error_response(error: ChannelError) -> JsonRpcResponseSingle {
    JsonRpcResponseSingle::new(error.into())
}

#[cfg(test)]
mod tests {
    //! Drive the server like `jsonrpc/tests/spec.rs`: requests in, a `Vec`
    //! of responses out.
    use super::*;
    use crate::{
        config::{ConfigOption, ConfigType, Localized},
        delivery::{DeliveryDetail, DeliveryResult},
        info::Info,
    };
    use futures_util::stream;
    use serde::Deserialize;
    use serde_json::{Value, json};

    #[derive(Deserialize)]
    struct Config {
        greeting: String,
    }

    impl ChannelConfig for Config {
        fn config_attrs() -> Vec<ConfigOption> {
            vec![
                ConfigOption::new("greeting", ConfigType::String, Localized::en_us("Greeting"))
                    .required(),
            ]
        }
    }

    struct Test;

    impl Channel for Test {
        type Config = Config;

        fn info(&self) -> Info {
            Info {
                name: "Test".into(),
                version: "1".into(),
                author: "me".into(),
            }
        }

        fn validate(&self, config: &Config) -> Result<(), ChannelError> {
            if config.greeting == "bad" {
                return Err(ChannelError::new("bad greeting"));
            }
            Ok(())
        }

        async fn send_notification(
            &self,
            config: Arc<Config>,
            notification: NotificationV1,
        ) -> Result<Option<DeliveryResult>, ChannelError> {
            if notification.summary == "panic" {
                panic!("intentional");
            }
            Ok(Some(DeliveryResult {
                state: None,
                details: vec![DeliveryDetail::Text {
                    label: "Greeting".into(),
                    value: format!("{} {}", config.greeting, notification.contact.full_name),
                }],
            }))
        }
    }

    fn notification(summary: &str) -> Value {
        json!({"version": 1, "payload": {
            "contact": {"full_name": "admin", "addresses": []},
            "object": {"name": "o", "url": "http://x", "sources": [], "tags": {}},
            "incident": null,
            "event": {"time": "2024-07-12T10:47:30Z", "summary": "s", "message": "m"},
            "summary": summary, "body": "b"
        }})
    }

    /// Sends each request on its own (so a `SetConfig` is applied before the
    /// next call) to one server, and returns the responses.
    async fn session(requests: Vec<(&str, Value)>) -> Vec<Value> {
        let server = server(Test, usize::MAX);
        let mut responses = Vec::new();
        for (id, (method, params)) in requests.into_iter().enumerate() {
            let line = json!({"jsonrpc": "2.0", "method": method, "params": params, "id": id});
            let mut out = Vec::new();
            server
                .serve(stream::iter([line.to_string()]), &mut out)
                .await
                .unwrap();
            responses.extend(out.iter().map(|r| serde_json::to_value(r).unwrap()));
        }
        responses
    }

    fn configure(greeting: &str) -> (&'static str, Value) {
        ("SetConfig", json!({"greeting": greeting}))
    }

    fn send(summary: &str) -> (&'static str, Value) {
        ("SendNotification", notification(summary))
    }

    #[tokio::test]
    async fn get_info() {
        let r = session(vec![("GetInfo", json!({}))]).await;
        assert_eq!(
            r[0]["result"],
            json!({
                "name": "Test", "version": "1", "author": "me",
                "supported_request_versions": [1],
                "config_attrs": [{"name": "greeting", "type": "string",
                    "label": {"en_US": "Greeting"}, "required": true,
                    "min": null, "max": null}]
            })
        );
    }

    #[tokio::test]
    async fn send_requires_config() {
        let r = session(vec![send("s"), configure("hi"), send("s")]).await;
        assert_eq!(r[0]["error"]["code"], -32000);
        assert_eq!(r[0]["error"]["message"], "channel not configured");
        assert_eq!(r[1]["result"], Value::Null);
        assert_eq!(r[2]["result"]["details"][0]["value"], "hi admin");
    }

    #[tokio::test]
    async fn set_config_replaces_and_invalid_keeps_old() {
        let r = session(vec![
            configure("a"),
            configure("b"),
            send("s"),
            configure("bad"),
            send("s"),
        ])
        .await;
        assert_eq!(r[2]["result"]["details"][0]["value"], "b admin");
        assert_eq!(r[3]["error"]["message"], "bad greeting");
        assert_eq!(r[4]["result"]["details"][0]["value"], "b admin");
    }

    #[tokio::test]
    async fn rejects_unsupported_version() {
        let mut request = notification("s");
        request["version"] = json!(2);
        let r = session(vec![configure("a"), ("SendNotification", request)]).await;
        assert_eq!(
            r[1]["error"]["message"],
            "unsupported request version 2, supported: [1]"
        );
    }

    #[tokio::test]
    async fn panic_is_internal_error() {
        let r = session(vec![configure("a"), send("panic")]).await;
        assert_eq!(r[1]["error"]["code"], -32603);
    }
}
