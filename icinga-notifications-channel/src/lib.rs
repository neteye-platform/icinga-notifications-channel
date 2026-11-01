//! Write [Icinga Notifications channel plugins](https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md)
//! on top of `jsonrpc-rs`.

pub mod channel;
pub mod config;
pub mod delivery;
pub mod error;
pub mod info;
pub mod log;
pub mod notification;
mod run;
pub mod state;

mod methods;
mod output;

pub use run::{RunOptions, run, run_with, run_with_subscriber};

#[cfg(feature = "derive")]
pub use icinga_notifications_channel_macros::{ChannelConfig, ConfigEnum};

#[doc(hidden)]
pub mod __private {
    use serde::Serialize;
    use serde_json::Value;

    /// A default as the string or bool `GetInfo` expects.
    pub fn default_value(default: &impl Serialize) -> Value {
        match serde_json::to_value(default).expect("defaults are serializable") {
            Value::Number(n) => Value::String(n.to_string()),
            other => other,
        }
    }
}
