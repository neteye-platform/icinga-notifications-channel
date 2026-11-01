use icinga_notifications_channel::{ChannelConfig, ConfigEnum};
use serde::Deserialize;

#[derive(Deserialize, ConfigEnum)]
enum M {
    A,
}

#[derive(Deserialize, ChannelConfig)]
#[config(locales = "../../../../tests/locales/m_mid_a")]
struct C {
    m: M,
    #[config(parent = "m", when = ["A"])]
    mid: Option<M>,
    #[config(parent = "mid", when = ["A"])]
    a: Option<String>,
}

fn main() {}
