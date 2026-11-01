use icinga_notifications_channel::ChannelConfig;

#[derive(ChannelConfig)]
struct C { #[config(min = 1)] a: String }

fn main() {}
