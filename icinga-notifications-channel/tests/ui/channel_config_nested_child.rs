use icinga_notifications_channel::{ChannelConfig, ConfigEnum};
use serde::Deserialize;

#[derive(Deserialize, ConfigEnum)]
enum M {
    A,
}

#[derive(Deserialize, ChannelConfig)]
struct C {
    m: M,
    #[config(parent = "m", when = ["A"])]
    mid: Option<M>,
    #[config(parent = "mid", when = ["A"])]
    a: Option<String>,
}

fn main() {}
