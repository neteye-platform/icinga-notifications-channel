use icinga_notifications_channel::{ConfigEnum, config::ConfigEnum as _};
use serde::Deserialize;

#[derive(Deserialize, ConfigEnum)]
enum Plain {
    GetRequest,
    #[serde(rename = "custom")]
    Renamed,
}

#[derive(Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
enum Snake {
    GetRequest,
    Post,
    #[serde(rename = "x")]
    Other,
}

#[test]
fn variants_without_rename_all() {
    assert_eq!(Plain::VARIANTS, ["GetRequest", "custom"]);
}

#[test]
fn variants_with_rename_all() {
    assert_eq!(Snake::VARIANTS, ["get_request", "post", "x"]);
}

#[test]
fn compile_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/config_enum_*.rs");
}
