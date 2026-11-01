use icinga_notifications_channel::ChannelConfig;
use serde::Deserialize;

#[derive(Deserialize, ChannelConfig)]
#[config(locales = "../../../../icinga-notifications-channel/tests/locales/bad_unknown_key")]
struct C {
    a: String,
}

fn main() {}
