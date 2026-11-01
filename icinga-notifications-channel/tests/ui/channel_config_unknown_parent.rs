use icinga_notifications_channel::{ChannelConfig, ConfigEnum};
use serde::Deserialize;

#[derive(Deserialize, ConfigEnum)]
enum M {
    A,
}

#[derive(Deserialize, ChannelConfig)]
struct C {
    #[config(parent = "nope", when = ["A"])]
    a: Option<String>,
}

fn main() {}
