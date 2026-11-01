//! A channel that logs each notification and returns its summary as a detail.
use std::sync::Arc;

use icinga_notifications_channel::{
    channel::{Channel, ChannelConfig},
    config::{ConfigOption, ConfigType, Localized},
    delivery::{DeliveryDetail, DeliveryResult},
    error::ChannelError,
    info::Info,
    notification::NotificationV1,
    run,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    #[serde(default = "default_prefix")]
    prefix: String,
}

fn default_prefix() -> String {
    "echo".into()
}

impl ChannelConfig for Config {
    fn config_attrs() -> Vec<ConfigOption> {
        vec![
            ConfigOption::new("prefix", ConfigType::String, Localized::en_us("Prefix"))
                .default(default_prefix()),
        ]
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
