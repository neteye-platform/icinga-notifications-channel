# jsonrpc-rs: JSON-RPC 2.0 for Rust

An implementation of the [JSON-RPC 2.0](https://www.jsonrpc.org/specification) protocol for Rust,
designed for ergonomics and transport agnosticism.

## Highlights

- Transport agnostic (works over any `Stream<Item = impl Into<Vec<u8>>>`).
- Async method execution with bounded parallelism.
- Managed shared state injection (thread-safe, async accessible).
- Optional Tokio integration (via `tokio_util` codecs).
- Procedural macro (`#[jsonrpc]`) removing boilerplate for parameter parsing and state access.
- Fine-grained control over concurrency and future mapping (spawning, tracing, timeouts).

JSON-RPC 2.0 defines message structure and behavior without dictating the transport. This crate lets
you plug in stdin/stdout, TCP, WebSockets, in-memory channels, or any other framed text interface.

---

## Installation

Add the crate (and optional features) with:

```bash
cargo add jsonrpc-rs
```

Or in `Cargo.toml`:

```toml
[dependencies]
jsonrpc-rs = { version = "0.x", features = ["macros", "tokio_util"] }
```

Feature flags:

| Feature     | Purpose |
|-------------|---------|
| `macros`    | Enables the `#[jsonrpc]` procedural macro for auto-registration & param parsing. |
| `tokio_util`| Enables `tokio` integration (`build_with_codec`) for framed async IO. |

---

## Quick Start (Macro-Based)

```rust
use jsonrpc_rs::{
    jsonrpc,
    server::ServerBuilder,
    tokio::BuilderTokioServerExt,
    tokio::TokioServer,
};

struct Greetings { greeting: String }

#[jsonrpc]
async fn add(a: i64, b: i64) -> i64 { a + b }

#[jsonrpc(name = "sum", params = "params")]
async fn calculate_sum(params: Vec<i32>) -> i32 {
    params.into_iter().sum()
}

#[jsonrpc(state = "app")]
async fn greet(app: &Greetings, name: String) -> String {
    format!("{}, {name}!", app.greeting)
}

fn main() {
    let codec = tokio_util::codec::LinesCodec::new();
    let server: TokioServer<_, String, _, _, _> = ServerBuilder::new()
        .manage(Greetings { greeting: "Hello".into() })
        .register_named(add)
        .register_named(calculate_sum)
        .register_named(greet)
        .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());

    tokio::runtime::Builder::new_current_thread()
        .build().unwrap()
        .block_on(server).unwrap();
}
```

Client messages (one per line):

```text
{"jsonrpc":"2.0","method":"add","params":{"a":7,"b":35},"id":1}
{"jsonrpc":"2.0","method":"sum","params":[1,2,3,4],"id":2}
{"jsonrpc":"2.0","method":"greet","params":{"name":"World"},"id":3}
```

Responses:

```text
{"jsonrpc":"2.0","result":42,"id":1}
{"jsonrpc":"2.0","result":10,"id":2}
{"jsonrpc":"2.0","result":"Hello, World!","id":3}
```

---

## Managed State

Register arbitrary shared values with `.manage(value)`:

```rust,ignore
ServerBuilder::new()
    .manage(MyConfig { ... })
    .register_named(my_method);
```

In macro-decorated RPC functions, mark a parameter as managed state:

```rust
use jsonrpc_rs::jsonrpc;
struct MyConfig {
    version: String,
}


#[jsonrpc(state = "cfg")]
async fn version(cfg: &MyConfig) -> String {
    cfg.version.clone()
}
```

The state can also be manually accessed (without macro) in the dispatch function:

```rust,ignore
let arc_lock = server_state.get_arc_lock::<MyConfig>().unwrap();
let cfg = arc_lock.read().await;
```

---

## Notifications

Requests without an `id` field (or with `"id": null`) are treated as notifications and MUST NOT
produce a response:

```text
{"jsonrpc":"2.0","method":"add","params":[1,2]}
```

---

## Batch Requests

Provide a JSON array. Requests in a batch requests are executed sequentially,
while single messages are processed concurrently, limited my `max_parallel_requests`.

```text
[
  {"jsonrpc":"2.0","method":"add","params":[4,5],"id":1},
  {"jsonrpc":"2.0","method":"ping","id":2},
  {"jsonrpc":"2.0","method":"no_such","id":3},
  {"jsonrpc":"2.0","method":"add","params":[10,7]}  // notification
]
```

Response (notification omitted):

```text
[
  {"jsonrpc":"2.0","result":9,"id":1},
  {"jsonrpc":"2.0","result":"pong","id":2},
  {"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":3}
]
```

---

## Manual Registration (Without Macros)

Use this path for custom validation or dynamic method generation.

```rust
use jsonrpc_rs::server::{
    ServerBuilder, JsonRpcRequest, JsonRpcMethodCall,
    JsonRpcResponseSingle, JsonRpcResult, JsonRpcErrorCode,
    parse_params, ServerState
};
use futures_util::{stream, StreamExt};
use serde::Deserialize;

#[derive(Deserialize)]
struct DivParams { numerator: i64, denominator: i64 }

fn div_dispatch(_: &ServerState, req: &JsonRpcRequest)
    -> Result<JsonRpcMethodCall, JsonRpcResponseSingle>
{
    let params: DivParams = parse_params(req)?;
    if params.denominator == 0 {
        return Err(JsonRpcResponseSingle::new(JsonRpcResult::Err {
            code: JsonRpcErrorCode::invalid_params(),
            message: "Division by zero".into(),
            data: None,
        }));
    }
    Ok(Box::new(async move {
        JsonRpcResponseSingle::new(
            JsonRpcResult::Ok(Box::new(params.numerator / params.denominator))
        ).into()
    }))
}

#[tokio::main(flavor="current_thread")]
async fn main() {
    let inputs = vec![
        r#"{"jsonrpc":"2.0","method":"div","params":{"numerator":10,"denominator":2},"id":1}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"div","params":{"numerator":5,"denominator":0},"id":2}"#.to_string(),
    ];
    let mut server = ServerBuilder::new()
        .register("div".to_string(), &div_dispatch)
        .build(stream::iter(inputs));

    while let Some(resp) = server.next().await {
        println!("{}", serde_json::to_string(&resp).unwrap());
    }
}
```

---

## Concurrency & Future Mapping

Limit simultaneous method execution:

```rust
use jsonrpc_rs::server::ServerBuilder;
ServerBuilder::new().max_parallel_requests(32);
```

Wrap method futures (e.g., spawn, trace, timeout):

```rust
use jsonrpc_rs::{jsonrpc, server::ServerBuilder};

ServerBuilder::new()
    .map_requests(&|fut| Box::pin(async move {
        tokio::time::timeout(std::time::Duration::from_secs(2), fut)
            .await
            .unwrap_or_else(|_| {
                jsonrpc_rs::server::JsonRpcResponseSingle::new(
                    jsonrpc_rs::server::JsonRpcResult::Err {
                        code: jsonrpc_rs::server::JsonRpcErrorCode::internal_error(),
                        message: "Timeout".into(),
                        data: None,
                    }
                ).into()
            })
    }));
```

Or use the helper:

```rust
use jsonrpc_rs::server::ServerBuilder;
use jsonrpc_rs::tokio::tokio_parallel;

ServerBuilder::new().map_requests(&tokio_parallel);
```

---

## Tokio Integration

Under the `tokio_util` feature:

```rust
use jsonrpc_rs::{server::ServerBuilder, jsonrpc};
use crate::jsonrpc_rs::tokio::{BuilderTokioServerExt, TokioServer};

#[jsonrpc]
fn add(lhs: i32, rhs: i32) -> i32 {
    lhs + rhs
}

let codec = tokio_util::codec::LinesCodec::new();
let server: TokioServer<_, String, _, _, _> = ServerBuilder::new()
    .register_named(add)
    .build_with_codec(codec, tokio::io::stdin(), tokio::io::stdout());

tokio::runtime::Builder::new_current_thread()
    .build().unwrap()
    .block_on(server).unwrap();
```

You can supply any codec implementing `Decoder<Item = I>` and `Encoder<O>` where `I: Into<Vec<u8>>`
and `O: From<String>`.

---

## Error Codes

Implemented helpers:

| Helper                 | Code    | Meaning            |
|------------------------|---------|--------------------|
| `parse_error()`        | -32700  | Invalid JSON       |
| `invalid_request()`    | -32600  | Bad request object |
| `method_not_found()`   | -32601  | Unknown method     |
| `invalid_params()`     | -32602  | Params validation  |
| `internal_error()`     | -32603  | Internal failure   |
| `server_error(code)`   | (custom)| Server extension   |
| `custom_error(code)`   | (custom)| App-specific code  |

---

## Best Practices

- Return `invalid_params` for semantically wrong but structurally valid input.
- Use notifications for fire-and-forget operations to reduce client/server chatter.
- Keep method futures lean; offload heavy work with `.map_requests` or internal task spawning.
- Populate `error.data` with structured JSON for actionable client diagnostics.
- Version your managed state types if you expect evolution (e.g., wrap in enum).

---

## Spec Mapping

| Spec Concept        | API Surface                                        |
|---------------------|----------------------------------------------------|
| Request Object      | `JsonRpcRequest`                                   |
| Response Object     | `JsonRpcResponseSingle`, `JsonRpcResult`           |
| Batch               | `JsonRpcResponse::Bulk`                            |
| Error Object        | `JsonRpcResult::Err`, `JsonRpcErrorCode`           |
| Notification        | `id: None` (response suppressed)                   |
| Transport Agnostic  | `ServerBuilder::build(Stream<Item = impl Into<Vec<u8>>)` |

---

## Extending

You can layer HTTP, WebSocket, or other transports by adapting inbound frames into
a `Stream<Item = Vec<u8>>` (or `String`) and forwarding serialized responses to the client.
