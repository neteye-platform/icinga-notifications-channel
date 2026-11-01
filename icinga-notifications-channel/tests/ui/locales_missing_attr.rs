use icinga_notifications_channel::ChannelConfig;
use serde::Deserialize;

#[derive(Deserialize, ChannelConfig)]
struct C {
    a: String,
}

fn main() {}
