use icinga_notifications_channel::ChannelConfig;
use serde::Deserialize;

#[derive(Deserialize, ChannelConfig)]
struct C {
    #[serde(flatten)]
    a: Inner,
}

#[derive(Deserialize)]
struct Inner {}

fn main() {}
