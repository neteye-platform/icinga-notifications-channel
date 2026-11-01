//! The traits a channel implements.

use std::{future::Future, sync::Arc};

use serde::de::DeserializeOwned;

use crate::{
    config::ConfigOption, delivery::DeliveryResult, error::ChannelError, info::Info,
    notification::NotificationV1,
};

/// The configuration of a channel, deserialized from `SetConfig`.
///
/// Missing keys must fall back to the defaults advertised in
/// [`config_attrs`](ChannelConfig::config_attrs), e.g. with
/// `#[serde(default)]`.
pub trait ChannelConfig: DeserializeOwned + Send + Sync + 'static {
    /// The options advertised in `GetInfo`.
    fn config_attrs() -> Vec<ConfigOption>;
}

/// A channel plugin.
pub trait Channel: Send + Sync + 'static {
    /// The channel's configuration.
    type Config: ChannelConfig;

    /// Meta information returned by `GetInfo`.
    fn info(&self) -> Info;

    /// Checks a new configuration before it replaces the current one.
    ///
    /// On error the previous configuration stays active.
    fn validate(&self, _config: &Self::Config) -> Result<(), ChannelError> {
        Ok(())
    }

    /// Sends one notification using the configuration active at the call.
    fn send_notification(
        &self,
        config: Arc<Self::Config>,
        notification: NotificationV1,
    ) -> impl Future<Output = Result<Option<DeliveryResult>, ChannelError>> + Send;
}

/// Fails a test unless every label and help of `C` exists in `locale`.
///
/// Missing translations are not a compile error, since only `en_US` is
/// required. Call this in a test to keep a locale complete:
///
/// ```ignore
/// #[test]
/// fn german_is_complete() {
///     icinga_notifications_channel::channel::assert_complete_locale::<Config>("de_DE");
/// }
/// ```
///
/// A help is only expected where `en_US` has one. Choices of select options
/// are not localized by Icinga Notifications and are not checked.
///
/// # Panics
///
/// Lists every missing text, or an invalid `locale`.
pub fn assert_complete_locale<C: ChannelConfig>(locale: &str) {
    if let Err(e) = crate::config::Locale::new(locale) {
        panic!("{e}");
    }
    let missing: Vec<String> = C::config_attrs()
        .iter()
        .flat_map(|option| option.missing_translations(locale))
        .collect();
    assert!(
        missing.is_empty(),
        "the locale {locale} is incomplete:\n  {}",
        missing.join("\n  ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Config {}

    impl ChannelConfig for Config {
        fn config_attrs() -> Vec<ConfigOption> {
            Vec::new()
        }
    }

    struct Null;

    impl Channel for Null {
        type Config = Config;

        fn info(&self) -> Info {
            Info {
                name: "null".into(),
                version: "0".into(),
                author: "me".into(),
            }
        }

        async fn send_notification(
            &self,
            _config: Arc<Config>,
            _notification: NotificationV1,
        ) -> Result<Option<DeliveryResult>, ChannelError> {
            Ok(None)
        }
    }

    #[test]
    fn hand_written_channel_compiles() {
        assert_eq!(Null.info().name, "null");
        assert!(Null.validate(&Config {}).is_ok());
    }
}
