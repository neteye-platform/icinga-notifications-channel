//! Forwards `tracing` events to Icinga Notifications as `channel::Log`.
//!
//! Stdout belongs to the protocol, so nothing else may print to it.

use std::fmt::Debug;

use serde_json::{Value, json};
use tracing::{Event, Level, Subscriber, field::Field};
use tracing_subscriber::{Layer, layer::Context};

use crate::output::{Outgoing, OutgoingSender};

/// A [`Layer`] sending each event as a `channel::Log` notification.
///
/// `TRACE` and `DEBUG` become `debug`. The `message` field is the log
/// message; all other fields are sent as a flat `key, value, ...` array.
pub struct ChannelLogLayer {
    sender: OutgoingSender,
}

impl ChannelLogLayer {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(sender: OutgoingSender) -> Self {
        Self { sender }
    }
}

fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::TRACE | Level::DEBUG => "debug",
        Level::INFO => "info",
        Level::WARN => "warn",
        Level::ERROR => "error",
    }
}

#[derive(Default)]
struct Fields {
    message: String,
    fields: Vec<Value>,
}

impl Fields {
    fn field(&mut self, field: &Field, value: Value) {
        self.fields.push(field.name().into());
        self.fields.push(value);
    }
}

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.field(field, format!("{value:?}").into());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.field(field, value.into());
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.field(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.field(field, value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.field(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // NaN and infinity have no JSON representation.
        self.field(field, value.into());
    }
}

impl<S: Subscriber> Layer<S> for ChannelLogLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut params = json!({
            "level": level_name(event.metadata().level()),
            "message": fields.message,
        });
        if !fields.fields.is_empty() {
            params["fields"] = fields.fields.into();
        }
        // The writer is gone during shutdown; there is nowhere left to log to.
        let _ = self.sender.unbounded_send(Outgoing::Log(
            json!({"jsonrpc": "2.0", "method": "channel::Log", "params": params}),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::queue;
    use tracing_subscriber::layer::SubscriberExt;

    fn logged(log: impl FnOnce()) -> Vec<Value> {
        let (tx, mut rx) = queue();
        let subscriber = tracing_subscriber::registry().with(ChannelLogLayer::new(tx));
        tracing::subscriber::with_default(subscriber, log);
        let mut out = Vec::new();
        while let Ok(Some(Outgoing::Log(value))) = rx.try_next() {
            out.push(value);
        }
        out
    }

    #[test]
    fn info_with_field() {
        let out = logged(|| tracing::info!(id = 5, "hi"));
        assert_eq!(
            out,
            [json!({"jsonrpc": "2.0", "method": "channel::Log",
                "params": {"level": "info", "message": "hi", "fields": ["id", 5]}})]
        );
    }

    #[test]
    fn levels_and_field_types() {
        let out = logged(|| {
            tracing::trace!("t");
            tracing::debug!("d");
            tracing::warn!("w");
            tracing::error!(name = "x", ok = true, err = ?"e", "e");
        });
        let levels: Vec<_> = out.iter().map(|v| v["params"]["level"].clone()).collect();
        assert_eq!(levels, ["debug", "debug", "warn", "error"]);
        assert!(out[0]["params"].get("fields").is_none());
        assert_eq!(
            out[3]["params"]["fields"],
            json!(["name", "x", "ok", true, "err", "\"e\""])
        );
        assert!(out[0].get("id").is_none());
    }
}
