//! The `SendNotification` request, version 1.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::state::State;

/// The `params` of `SendNotification`. The payload depends on `version`.
#[derive(Debug, Deserialize)]
pub struct NotificationRequest {
    /// The request version.
    pub version: u32,
    /// The version specific payload.
    pub payload: Box<RawValue>,
}

/// The payload of request version 1.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NotificationV1 {
    /// The contact to notify.
    pub contact: Contact,
    /// The object the notification is about.
    pub object: Object,
    /// The incident; `None` if the notification does not relate to one.
    #[serde(default)]
    pub incident: Option<Incident>,
    /// The event that triggered the notification.
    pub event: Event,
    /// A short rendering of the notification.
    pub summary: String,
    /// A long rendering of the notification.
    pub body: String,
    /// Present if there is an incident and the contact has a matching address.
    #[serde(default)]
    pub state: Option<State>,
}

/// A contact.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Contact {
    /// The contact's full name.
    pub full_name: String,
    /// The contact's addresses whose type matches the channel.
    pub addresses: Vec<Address>,
}

/// An address of a contact.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Address {
    /// The address type, e.g. `email`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The address.
    pub address: String,
}

/// The object a notification is about.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Object {
    /// The object's name.
    pub name: String,
    /// A link to the object.
    pub url: String,
    /// The sources that reported the object.
    pub sources: Vec<String>,
    /// The object's tags.
    pub tags: BTreeMap<String, String>,
}

/// The severity of an incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Everything is fine.
    Ok,
    /// Warning.
    Warning,
    /// Error.
    Error,
    /// Critical.
    Critical,
}

/// An incident.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Incident {
    /// The current severity.
    pub severity: Severity,
    /// Whether the incident is muted.
    pub muted: bool,
    /// Whether the incident has recovered.
    pub recovered: bool,
    /// The message of the latest event.
    pub message: String,
    /// The summary of the latest event.
    pub summary: String,
    /// When the incident was opened.
    pub started_at: Timestamp,
    /// When the incident recovered.
    #[serde(default)]
    pub recovered_at: Option<Timestamp>,
    /// Why the incident is muted.
    #[serde(default)]
    pub muted_reason: Option<String>,
    /// Whether the incident is managed.
    pub managed: bool,
    /// Who manages the incident.
    #[serde(default)]
    pub managed_by: Option<String>,
    /// The source that opened the incident.
    pub opened_by_source: String,
}

/// An event.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Event {
    /// The event type, e.g. `downtime_start`.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// When the event happened.
    pub time: Timestamp,
    /// The event summary.
    pub summary: String,
    /// The event message.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> serde_json::Value {
        json!({
            "contact": {"full_name": "icingaadmin",
                "addresses": [{"type": "email", "address": "a@example.com"}]},
            "object": {"name": "example-809!disk /", "url": "http://localhost/x",
                "sources": ["icinga2-master"],
                "tags": {"host": "example-809", "service": "disk /"}},
            "incident": {"severity": "critical", "muted": false, "recovered": false,
                "message": "m", "summary": "s", "started_at": "2024-07-12T10:40:00Z",
                "managed": false, "opened_by_source": "icinga2-master"},
            "event": {"time": "2024-07-12T10:47:30.445439055Z", "message": "m", "summary": "s"},
            "summary": "sum", "body": "body",
            "state": {"state_key": "0d6a5bd1-3c1e-5f0e-9d43-6f1f6c2b7a10",
                "value": "{\"last_message_id\": \"x\"}"}
        })
    }

    #[test]
    fn deserializes_doc_example() {
        let request: NotificationRequest =
            serde_json::from_value(json!({"version": 1, "payload": payload()})).unwrap();
        assert_eq!(request.version, 1);
        let n: NotificationV1 = serde_json::from_str(request.payload.get()).unwrap();
        assert_eq!(n.contact.addresses[0].kind, "email");
        assert_eq!(n.object.tags["service"], "disk /");
        let incident = n.incident.unwrap();
        assert_eq!(incident.severity, Severity::Critical);
        assert_eq!(incident.started_at.to_string(), "2024-07-12T10:40:00Z");
        assert_eq!(n.event.time.subsec_nanosecond(), 445439055);
        assert_eq!(n.state.unwrap().value, "{\"last_message_id\": \"x\"}");
    }

    #[test]
    fn deserializes_null_incident_and_missing_state() {
        let mut p = payload();
        p["incident"] = json!(null);
        p.as_object_mut().unwrap().remove("state");
        let n: NotificationV1 = serde_json::from_value(p).unwrap();
        assert!(n.incident.is_none());
        assert!(n.state.is_none());
    }
}
