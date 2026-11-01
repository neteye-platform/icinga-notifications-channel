use icinga_notifications_channel::ConfigEnum;

#[derive(ConfigEnum)]
enum Mode {
    Fine,
    Data(String),
}

fn main() {}
