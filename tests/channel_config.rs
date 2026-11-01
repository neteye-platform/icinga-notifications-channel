#![allow(dead_code)]
use std::collections::BTreeSet;

use icinga_notifications_channel::{ConfigEnum, channel::ChannelConfig};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, ConfigEnum, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum Method {
    Get,
    Post,
}

fn default_user() -> String {
    "admin".into()
}

#[derive(Deserialize, icinga_notifications_channel::ChannelConfig)]
#[serde(rename_all = "camelCase")]
#[config(locales = "tests/locales/everything")]
struct Everything {
    url_template: String,
    #[serde(default = "default_user")]
    user: String,
    password: Option<icinga_notifications_channel::config::Secret>,
    #[config(text)]
    body: Option<String>,
    #[serde(default)]
    #[config(min = 1, max = 10)]
    retries: u8,
    #[serde(default)]
    verify: bool,
    method: Method,
    #[serde(rename = "methods")]
    allowed: BTreeSet<Method>,
    #[serde(skip)]
    _internal: u8,
}

#[test]
fn maps_every_field() {
    let attrs = serde_json::to_value(Everything::config_attrs()).unwrap();
    let label = |name: &str| json!({ "en_US": format!("L {name}") });
    assert_eq!(
        attrs,
        json!([
            { "name": "urlTemplate", "type": "string", "label": label("urlTemplate"),
              "required": true, "min": null, "max": null },
            { "name": "user", "type": "string", "label": label("user"),
              "default": "admin", "min": null, "max": null },
            { "name": "password", "type": "secret", "label": label("password"),
              "min": null, "max": null },
            { "name": "body", "type": "text", "label": label("body"),
              "min": null, "max": null },
            { "name": "retries", "type": "number", "label": label("retries"),
              "default": "0", "min": 1, "max": 10 },
            { "name": "verify", "type": "bool", "label": label("verify"),
              "default": false, "min": null, "max": null },
            { "name": "method", "type": "option", "label": label("method"),
              "required": true, "options": { "get": "get", "post": "post" },
              "min": null, "max": null },
            { "name": "methods", "type": "options", "label": label("methods"),
              "required": true, "options": { "get": "get", "post": "post" },
              "min": null, "max": null },
        ])
    );
}

#[derive(Deserialize, icinga_notifications_channel::ChannelConfig)]
#[serde(default)]
#[config(locales = "tests/locales/whole_default")]
struct WholeDefault {
    port: u16,
}

impl Default for WholeDefault {
    fn default() -> Self {
        Self { port: 8080 }
    }
}

#[test]
fn struct_default_is_used_per_field() {
    let attrs = serde_json::to_value(WholeDefault::config_attrs()).unwrap();
    assert_eq!(attrs[0]["default"], "8080");
    assert!(attrs[0].get("required").is_none());
}

#[test]
fn compile_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/channel_config_*.rs");
}

#[derive(Deserialize, icinga_notifications_channel::ChannelConfig)]
#[config(locales = "tests/locales/children")]
struct WithChildren {
    method: Method,
    #[config(parent = "method", when = ["post"])]
    body: Option<String>,
    #[serde(default)]
    auth: bool,
    #[config(parent = "auth", when = [true])]
    user: Option<String>,
}

#[test]
fn children_are_nested_under_their_parent() {
    let attrs = serde_json::to_value(WithChildren::config_attrs()).unwrap();
    let attrs = attrs.as_array().unwrap();
    assert_eq!(attrs.len(), 2);
    assert_eq!(attrs[0]["name"], "method");
    assert_eq!(
        attrs[0]["children"],
        json!([{
            "name": "body", "type": "string", "label": { "en_US": "L body" },
            "parent_values": ["post"], "min": null, "max": null
        }])
    );
    assert_eq!(attrs[1]["children"][0]["name"], "user");
    assert_eq!(attrs[1]["children"][0]["parent_values"], json!([true]));
}
