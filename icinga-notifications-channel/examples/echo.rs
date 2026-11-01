//! A channel that logs each notification and returns its summary as a detail.
use std::sync::Arc;

use icinga_notifications_channel::{
    ChannelConfig,
    channel::Channel,
    delivery::{DeliveryDetail, DeliveryResult},
    error::ChannelError,
    info::Info,
    notification::NotificationV1,
    run,
};
use serde::Deserialize;

/// Labels and help texts live in `examples/echo_locales/<locale>.toml`.
#[derive(Deserialize, ChannelConfig)]
#[config(locales = "examples/echo_locales")]
struct Config {
    #[serde(default = "default_prefix")]
    prefix: String,
}

fn default_prefix() -> String {
    "echo".into()
}

#[cfg(test)]
mod tests {
    use icinga_notifications_channel::channel::assert_complete_locale;

    use super::Config;

    #[test]
    fn german_is_complete() {
        assert_complete_locale::<Config>("de_DE");
    }
}

struct Echo;

impl Channel for Echo {
    type Config = Config;

    fn info(&self) -> Info {
        Info {
            name: "Echo".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            author: "example".into(),
        }
    }

    async fn send_notification(
        &self,
        config: Arc<Config>,
        notification: NotificationV1,
    ) -> Result<Option<DeliveryResult>, ChannelError> {
        tracing::info!(contact = notification.contact.full_name, "echoing");
        Ok(Some(DeliveryResult {
            state: None,
            details: vec![DeliveryDetail::Text {
                label: "Echo".into(),
                value: format!("{}: {}", config.prefix, notification.summary),
            }],
        }))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    run(Echo).await
}
