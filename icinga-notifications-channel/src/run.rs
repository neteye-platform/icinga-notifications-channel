//! Running a channel on a pair of byte streams.

use std::io;

use futures_util::{
    StreamExt,
    future::{join, ready},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};
use tracing_subscriber::{Registry, layer::SubscriberExt};

use crate::{
    channel::Channel,
    log::ChannelLogLayer,
    methods::server,
    output::{queue, response_sink, write_all},
};

/// Runs `channel` on `read` and `write` until `read` ends, then answers every
/// running request.
///
/// `tracing` events of the current thread are forwarded to Icinga
/// Notifications for the duration of the call. Events of tasks on other
/// threads are not, so use a current-thread runtime for tests.
pub async fn run_with<C: Channel>(
    channel: C,
    read: impl AsyncRead,
    write: impl AsyncWrite,
) -> io::Result<()> {
    session(channel, read, write, |layer| {
        tracing::subscriber::set_default(Registry::default().with(layer))
    })
    .await
}

/// Serves one session. `install` makes the log layer active and returns a
/// guard that is held until the session ends.
async fn session<C: Channel, Guard>(
    channel: C,
    read: impl AsyncRead,
    write: impl AsyncWrite,
    install: impl FnOnce(ChannelLogLayer) -> Guard,
) -> io::Result<()> {
    let (tx, rx) = queue();
    let _guard = install(ChannelLogLayer::new(tx.clone()));

    let source = FramedRead::new(read, LinesCodec::new()).filter_map(|line| {
        ready(
            line.inspect_err(|error| tracing::warn!(%error, "Skipping unreadable line"))
                .ok(),
        )
    });
    let serve = async {
        let result = server(channel)
            .serve(source, response_sink(tx.clone()))
            .await;
        // Other senders (the log layer) may outlive the session: stop the
        // writer once everything queued so far is written.
        tx.close_channel();
        result
    };
    let (served, written) = join(serve, write_all(rx, write)).await;
    served.map_err(io::Error::other)?;
    written.map_err(|error| match error {
        LinesCodecError::Io(error) => error,
        other => io::Error::other(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        channel::ChannelConfig, config::ConfigOption, delivery::DeliveryResult,
        error::ChannelError, info::Info, notification::NotificationV1,
    };
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    #[derive(Deserialize)]
    struct Config {}

    impl ChannelConfig for Config {
        fn config_attrs() -> Vec<ConfigOption> {
            Vec::new()
        }
    }

    struct Logging;

    impl Channel for Logging {
        type Config = Config;

        fn info(&self) -> Info {
            Info {
                name: "L".into(),
                version: "1".into(),
                author: "me".into(),
            }
        }

        async fn send_notification(
            &self,
            _: Arc<Config>,
            _: NotificationV1,
        ) -> Result<Option<DeliveryResult>, ChannelError> {
            tracing::info!(id = 7, "sending");
            Ok(None)
        }
    }

    #[tokio::test]
    async fn full_session_until_eof() {
        let notification = json!({"version": 1, "payload": {
            "contact": {"full_name": "a", "addresses": []},
            "object": {"name": "o", "url": "http://x", "sources": [], "tags": {}},
            "event": {"time": "2024-07-12T10:47:30Z", "summary": "s", "message": "m"},
            "summary": "s", "body": "b"}});
        let requests = [
            json!({"jsonrpc": "2.0", "method": "GetInfo", "id": 1}),
            json!({"jsonrpc": "2.0", "method": "SetConfig", "params": {}, "id": 2}),
            json!({"jsonrpc": "2.0", "method": "SendNotification", "params": notification, "id": 3}),
        ];
        let (mut client_write, read) = duplex(64 * 1024);
        let (write, mut client_read) = duplex(64 * 1024);
        let channel = tokio::spawn(run_with(Logging, read, write));
        for request in requests {
            client_write
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
        }
        drop(client_write);
        let mut out = String::new();
        client_read.read_to_string(&mut out).await.unwrap();
        channel.await.unwrap().unwrap();

        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4, "{out}");
        let log = lines
            .iter()
            .find(|l| l["method"] == "channel::Log")
            .unwrap();
        assert_eq!(
            log["params"],
            json!({"level": "info", "message": "sending", "fields": ["id", 7]})
        );
        for id in 1..=3 {
            assert!(
                lines
                    .iter()
                    .any(|l| l["id"] == id && l.get("error").is_none())
            );
        }
    }
}
