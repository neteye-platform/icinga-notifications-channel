//! Channel meta information for the `GetInfo` method.

use serde::Serialize;

use crate::config::ConfigOption;

/// The request versions this library understands.
pub const SUPPORTED_REQUEST_VERSIONS: [u32; 1] = [1];

/// Meta information about a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The display name of the channel.
    pub name: String,
    /// The version of the channel.
    pub version: String,
    /// The author of the channel.
    pub author: String,
}

/// The result of `GetInfo`: [`Info`] plus the supported request versions and options.
#[derive(Debug, Serialize)]
pub struct GetInfoResponse {
    name: String,
    version: String,
    author: String,
    supported_request_versions: [u32; 1],
    config_attrs: Vec<ConfigOption>,
}

impl GetInfoResponse {
    pub fn new(info: Info, config_attrs: Vec<ConfigOption>) -> Self {
        Self {
            name: info.name,
            version: info.version,
            author: info.author,
            supported_request_versions: SUPPORTED_REQUEST_VERSIONS,
            config_attrs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigType, Localized};
    use serde_json::json;

    #[test]
    fn serializes_doc_example() {
        let info = Info {
            name: "Minified Webhook".into(),
            version: "0.0.0-gf369a11-dirty".into(),
            author: "Icinga GmbH".into(),
        };
        let attrs = vec![
            ConfigOption::new(
                "url_template",
                ConfigType::String,
                Localized::en_us("URL Template")
                    .with("de_DE", "URL-Template")
                    .unwrap(),
            )
            .required(),
        ];
        assert_eq!(
            serde_json::to_value(GetInfoResponse::new(info, attrs)).unwrap(),
            json!({
                "name": "Minified Webhook",
                "version": "0.0.0-gf369a11-dirty",
                "author": "Icinga GmbH",
                "supported_request_versions": [1],
                "config_attrs": [{
                    "name": "url_template", "type": "string",
                    "label": {"de_DE": "URL-Template", "en_US": "URL Template"},
                    "required": true, "min": null, "max": null
                }]
            })
        );
    }
}
