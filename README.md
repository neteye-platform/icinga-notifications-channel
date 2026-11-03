# icinga-notifications-channel

Write [Icinga Notifications channel plugins](https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md)
on top of [`jsonrpc-rs`](crates/jsonrpc). A channel is a process that talks
JSON-RPC over stdin and stdout; this crate handles the protocol, the
configuration form, logging and shutdown, so you only write the delivery.

## Quick start

```rust
use std::sync::Arc;

use icinga_notifications_channel::{
    ChannelConfig, channel::Channel, delivery::DeliveryResult, error::ChannelError,
    info::Info, notification::NotificationV1, run,
};
use serde::Deserialize;

#[derive(Deserialize, ChannelConfig)]
#[config(locales = "locales")]
struct Config {
    #[serde(default = "default_prefix")]
    prefix: String,
}

fn default_prefix() -> String {
    "echo".into()
}

struct Echo;

impl Channel for Echo {
    type Config = Config;

    fn info(&self) -> Info {
        Info {
            name: "Echo".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            author: "me".into(),
        }
    }

    async fn send_notification(
        &self,
        config: Arc<Config>,
        notification: NotificationV1,
    ) -> Result<Option<DeliveryResult>, ChannelError> {
        tracing::info!(contact = notification.contact.full_name, "sending");
        Ok(None)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    run(Echo).await
}
```

A complete channel is in [`examples/echo.rs`](examples/echo.rs).

## Configuration

`#[derive(ChannelConfig)]` builds the `config_attrs` of `GetInfo` from a
`Deserialize` struct. Names, defaults and optionality follow serde.

| Field type | Form element |
|---|---|
| `String` | single-line input; `#[config(text)]` multi-line |
| `Secret` | password input, redacted in `Debug` |
| numbers | number input; `#[config(min = 1, max = 10)]` |
| `bool` | checkbox |
| `#[derive(ConfigEnum)]` enum | select |
| `Vec`/`BTreeSet` of such an enum | multi-select |

A field is required unless it is an `Option` or has `#[serde(default)]`.
`#[serde(flatten)]` and generic structs are rejected at compile time.

A field can be shown only for some values of another field:
`#[config(parent = "method", when = ["post"])]`, or `when = [true]` for a
checkbox. A child cannot be a parent.

## Translations

`#[config(locales = "dir")]` names a directory relative to the crate root
with one file per locale, named `language_REGION.toml`. Each table is an
option by its serialized name:

```toml
[method]
label = "Method"
help = "The HTTP method."   # optional

[method.options]            # optional, en_US.toml only
get = "GET request"         # defaults to the variant name
```

`en_US.toml` is required and needs a `label` for every option. Unknown
options and keys are compile errors. The files are embedded in the binary and
editing one rebuilds the crate. Other locales may be partial; keep one
complete with a test:

```rust,ignore
#[test]
fn german_is_complete() {
    icinga_notifications_channel::channel::assert_complete_locale::<Config>("de_DE");
}
```

## Running

- **Never write to stdout.** It carries the protocol. Use `tracing`; events
  are sent to Icinga Notifications as `channel::Log` notifications.
  `run_with_subscriber` adds your own layers.
- `run` stops on end of stdin, SIGINT or SIGTERM, then waits up to
  `RunOptions::drain_timeout` (5 s) for running requests before abandoning
  them.
- `RunOptions::max_parallel_requests` bounds the requests running at once
  (unlimited by default). At the limit, input is no longer read, which
  pushes back on the sender instead of queueing work in memory.
- `SetConfig` swaps the configuration without a restart; an invalid one
  (`Channel::validate`) keeps the previous one. `SendNotification` fails
  until the first `SetConfig`.
