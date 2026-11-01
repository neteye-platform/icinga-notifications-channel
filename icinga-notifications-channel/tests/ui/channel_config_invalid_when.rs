use icinga_notifications_channel::{ChannelConfig, ConfigEnum};
use serde::Deserialize;

#[derive(Deserialize, ConfigEnum)]
enum M {
    A,
}

#[derive(Deserialize, ChannelConfig)]
struct C {
    m: M,
    #[config(parent = "m", when = ["B"])]
    a: Option<String>,
}

fn main() {}
