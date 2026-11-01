use icinga_notifications_channel::ChannelConfig;
use serde::Deserialize;

#[derive(Deserialize, ChannelConfig)]
#[config(locales = "../../../../tests/locales/bad_help_only")]
struct C {
    a: String,
}

fn main() {}
