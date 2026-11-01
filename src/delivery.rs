//! The result of a successful `SendNotification`.

use std::fmt;

use jiff::Timestamp;
use serde::{Serialize, ser::SerializeMap};
use serde_json::Number;

use crate::{notification::Severity, state::StateUpdate};

/// An absolute `http` or `https` URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HttpUrl(String);

/// The string is not an absolute `http` or `https` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHttpUrl(String);

impl fmt::Display for InvalidHttpUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not an absolute http or https URL", self.0)
    }
}

impl std::error::Error for InvalidHttpUrl {}

impl HttpUrl {
    /// Accepts `http://host...` and `https://host...` only.
    pub fn new(url: impl Into<String>) -> Result<Self, InvalidHttpUrl> {
        let url = url.into();
        let rest = url
            .strip_prefix("http://")
            .or_else(|| url.strip_prefix("https://"));
        match rest {
            Some(rest) if !rest.is_empty() && !rest.starts_with(['/', '?', '#']) => Ok(Self(url)),
            _ => Err(InvalidHttpUrl(url)),
        }
    }
}

/// One piece of information about a delivery, shown in the notification history.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryDetail {
    /// A number.
    Number {
        /// What the value describes.
        label: String,
        /// The number.
        value: Number,
    },
    /// A boolean.
    Boolean {
        /// What the value describes.
        label: String,
        /// The boolean.
        value: bool,
    },
    /// A severity, shown with its color.
    Severity {
        /// What the value describes.
        label: String,
        /// The severity.
        value: Severity,
    },
    /// Plain text.
    Text {
        /// What the value describes.
        label: String,
        /// The text.
        value: String,
    },
    /// A link.
    Url {
        /// What the link points to.
        label: String,
        /// The target.
        value: HttpUrl,
        /// The link text; defaults to the URL.
        anchor_text: Option<String>,
    },
    /// A point in time.
    Datetime {
        /// What the value describes.
        label: String,
        /// The time, serialized as ISO 8601.
        value: Timestamp,
    },
}

impl Serialize for DeliveryDetail {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (kind, label, anchor_text) = match self {
            Self::Number { label, .. } => ("number", label, None),
            Self::Boolean { label, .. } => ("boolean", label, None),
            Self::Severity { label, .. } => ("severity", label, None),
            Self::Text { label, .. } => ("text", label, None),
            Self::Url {
                label, anchor_text, ..
            } => ("url", label, anchor_text.as_ref()),
            Self::Datetime { label, .. } => ("datetime", label, None),
        };
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", kind)?;
        map.serialize_entry("label", label)?;
        match self {
            Self::Number { value, .. } => map.serialize_entry("value", value)?,
            Self::Boolean { value, .. } => map.serialize_entry("value", value)?,
            Self::Severity { value, .. } => map.serialize_entry("value", value)?,
            Self::Text { value, .. } => map.serialize_entry("value", value)?,
            Self::Url { value, .. } => map.serialize_entry("value", value)?,
            Self::Datetime { value, .. } => map.serialize_entry("value", value)?,
        }
        if let Some(anchor_text) = anchor_text {
            map.serialize_entry("anchor_text", anchor_text)?;
        }
        map.end()
    }
}

/// What a channel reports after a successful delivery.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DeliveryResult {
    /// The new state to store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<StateUpdate>,
    /// Details for the notification history; at most 1 MiB serialized.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<DeliveryDetail>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use serde_json::json;

    #[test]
    fn serializes_doc_example() {
        let state: State = serde_json::from_value(json!({"state_key": "k", "value": ""})).unwrap();
        let result = DeliveryResult {
            state: Some(state.update("v").unwrap()),
            details: vec![DeliveryDetail::Text {
                label: "Message-ID".into(),
                value: "<x@example.com>".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "state": {"state_key": "k", "value": "v"},
                "details": [{"type": "text", "label": "Message-ID", "value": "<x@example.com>"}]
            })
        );
    }

    #[test]
    fn serializes_every_detail_type() {
        let url = HttpUrl::new("https://rt.example.com/Ticket/Display.html?id=123").unwrap();
        let details = [
            DeliveryDetail::Number {
                label: "n".into(),
                value: 5.into(),
            },
            DeliveryDetail::Boolean {
                label: "b".into(),
                value: true,
            },
            DeliveryDetail::Severity {
                label: "s".into(),
                value: Severity::Warning,
            },
            DeliveryDetail::Url {
                label: "Ticket".into(),
                value: url,
                anchor_text: Some("#123".into()),
            },
            DeliveryDetail::Datetime {
                label: "d".into(),
                value: "2024-07-12T10:40:00Z".parse().unwrap(),
            },
        ];
        assert_eq!(
            serde_json::to_value(details).unwrap(),
            json!([
                {"type": "number", "label": "n", "value": 5},
                {"type": "boolean", "label": "b", "value": true},
                {"type": "severity", "label": "s", "value": "warning"},
                {"type": "url", "label": "Ticket",
                 "value": "https://rt.example.com/Ticket/Display.html?id=123",
                 "anchor_text": "#123"},
                {"type": "datetime", "label": "d", "value": "2024-07-12T10:40:00Z"}
            ])
        );
    }

    #[test]
    fn empty_result_is_empty_object() {
        assert_eq!(
            serde_json::to_value(DeliveryResult::default()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn http_url_accepts_only_absolute_http() {
        assert!(HttpUrl::new("http://x").is_ok());
        assert!(HttpUrl::new("https://x/y").is_ok());
        for bad in ["ftp://x", "/relative", "http://", "http:///x", "x.com", ""] {
            assert!(HttpUrl::new(bad).is_err(), "{bad}");
        }
    }
}
