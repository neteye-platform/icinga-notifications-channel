use icinga_notifications_channel::ChannelConfig;

#[derive(ChannelConfig)]
struct C { #[config(text)] a: u8 }

fn main() {}
