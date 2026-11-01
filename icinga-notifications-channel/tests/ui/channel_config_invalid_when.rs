use icinga_notifications_channel::{ChannelConfig, ConfigEnum};
use serde::Deserialize;

#[derive(Deserialize, ConfigEnum)]
enum M {
    A,
}

#[derive(Deserialize, ChannelConfig)]
#[config(locales = "../../../../icinga-notifications-channel/tests/locales/m_a")]
struct C {
    m: M,
    #[config(parent = "m", when = ["B"])]
    a: Option<String>,
}

fn main() {}
