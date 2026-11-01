#![allow(dead_code)]
use icinga_notifications_channel::{ConfigEnum, channel::ChannelConfig};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
enum Method {
    Get,
    Post,
}

#[derive(Deserialize, icinga_notifications_channel::ChannelConfig)]
#[config(locales = "tests/locales/translated")]
struct Translated {
    method: Method,
    #[serde(default)]
    retries: u8,
}

#[test]
fn labels_help_and_options_come_from_the_locale_files() {
    let attrs = serde_json::to_value(Translated::config_attrs()).unwrap();
    assert_eq!(
        attrs[0],
        json!({
            "name": "method", "type": "option", "required": true,
            "label": { "en_US": "Method", "de_DE": "Methode" },
            "help": { "en_US": "HTTP method", "de_DE": "HTTP-Methode" },
            // Choices without a translation fall back to the variant name.
            "options": { "get": "GET request", "post": "post" },
            "min": null, "max": null
        })
    );
    // No help in any file: the key is omitted.
    assert!(attrs[1].get("help").is_none());
}

#[test]
fn compile_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/locales_*.rs");
}
