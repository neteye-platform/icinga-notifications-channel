use icinga_notifications_channel::ChannelConfig;
use serde::Deserialize;

#[derive(Deserialize, ChannelConfig)]
struct C { #[config(text)] a: u8 }

fn main() {}
