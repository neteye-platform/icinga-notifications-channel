//! Running a channel on a pair of byte streams.

use std::{future::Future, io, time::Duration};

use futures_util::{
    FutureExt, StreamExt,
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

/// Settings of a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// How long running requests may take to finish after a shutdown
    /// request. Requests still running afterwards are abandoned.
    pub drain_timeout: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            drain_timeout: Duration::from_secs(5),
        }
    }
}

/// Runs `channel` on stdin and stdout until stdin ends, SIGINT or SIGTERM
/// (on Windows: Ctrl-C) arrives.
///
/// Running requests are then answered, for at most
/// [`RunOptions::drain_timeout`]. `tracing` events are sent to Icinga
/// Notifications as `channel::Log`; **nothing else may write to stdout**.
pub async fn run<C: Channel>(channel: C) -> io::Result<()> {
    run_with_subscriber(channel, RunOptions::default(), |layer| {
        Registry::default().with(layer)
    })
    .await
}

/// Like [`run`], with a custom `tracing` subscriber built around the layer
/// that forwards events to Icinga Notifications.
///
/// The subscriber becomes the global default.
pub async fn run_with_subscriber<C, S>(
    channel: C,
    options: RunOptions,
    subscriber: impl FnOnce(ChannelLogLayer) -> S,
) -> io::Result<()>
where
    C: Channel,
    S: tracing::Subscriber + Send + Sync + 'static,
{
    session(
        channel,
        tokio::io::stdin(),
        tokio::io::stdout(),
        termination_signal(),
        options,
        |layer| tracing::subscriber::set_global_default(subscriber(layer)),
    )
    .await
}

/// Runs `channel` on `read` and `write` until `read` ends or `shutdown`
/// completes, then answers every running request within
/// [`RunOptions::drain_timeout`].
///
/// `tracing` events of the current thread are forwarded to Icinga
/// Notifications for the duration of the call. Events of tasks on other
/// threads are not, so use a current-thread runtime for tests.
pub async fn run_with<C: Channel>(
    channel: C,
    read: impl AsyncRead,
    write: impl AsyncWrite,
    shutdown: impl Future<Output = ()>,
    options: RunOptions,
) -> io::Result<()> {
    session(channel, read, write, shutdown, options, |layer| {
        tracing::subscriber::set_default(Registry::default().with(layer))
    })
    .await
}

/// Completes on SIGINT or SIGTERM (on Windows: Ctrl-C).
async fn termination_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            // Without signal handlers only EOF can stop the channel.
            return std::future::pending().await;
        };
        tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Serves one session. `install` makes the log layer active and returns a
/// guard that is held until the session ends.
async fn session<C: Channel, Guard>(
    channel: C,
    read: impl AsyncRead,
    write: impl AsyncWrite,
    shutdown: impl Future<Output = ()>,
    options: RunOptions,
    install: impl FnOnce(ChannelLogLayer) -> Guard,
) -> io::Result<()> {
    let (tx, rx) = queue();
    let _guard = install(ChannelLogLayer::new(tx.clone()));

    let shutdown = shutdown.shared();
    let source = FramedRead::new(read, LinesCodec::new())
        .filter_map(|line| {
            ready(
                line.inspect_err(|error| tracing::warn!(%error, "Skipping unreadable line"))
                    .ok(),
            )
        })
        .take_until(shutdown.clone());
    let server = server(channel);
    let serve = server.serve(source, response_sink(tx.clone()));
    // Starts counting once shutdown was requested.
    let deadline = async {
        shutdown.await;
        tokio::time::sleep(options.drain_timeout).await;
    };
    let serve = async {
        let result = tokio::select! {
            result = serve => result,
            () = deadline => {
                tracing::warn!("Abandoning requests still running after the drain timeout");
                Ok(())
            }
        };
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
            notification: NotificationV1,
        ) -> Result<Option<DeliveryResult>, ChannelError> {
            tracing::info!(id = 7, "sending");
            if notification.summary == "slow" {
                tokio::time::sleep(Duration::from_secs(1)).await;
            } else if notification.summary == "stuck" {
                std::future::pending::<()>().await;
            }
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
        let channel = tokio::spawn(run_with(
            Logging,
            read,
            write,
            std::future::pending(),
            RunOptions::default(),
        ));
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

    fn request(id: u32, summary: &str) -> String {
        json!({"jsonrpc": "2.0", "method": "SendNotification", "id": id,
            "params": {"version": 1, "payload": {
                "contact": {"full_name": "a", "addresses": []},
                "object": {"name": "o", "url": "http://x", "sources": [], "tags": {}},
                "event": {"time": "2024-07-12T10:47:30Z", "summary": "s", "message": "m"},
                "summary": summary, "body": "b"}}})
        .to_string()
    }

    /// Starts a session with a configured channel and one request in flight,
    /// then triggers the shutdown. Returns the output lines.
    async fn shutdown_during(summary: &str, drain_timeout: Duration) -> Vec<Value> {
        let (mut client_write, read) = duplex(64 * 1024);
        let (write, mut client_read) = duplex(64 * 1024);
        let (trigger, triggered) = tokio::sync::oneshot::channel::<()>();
        let options = RunOptions { drain_timeout };
        let channel = tokio::spawn(run_with(
            Logging,
            read,
            write,
            async {
                let _ = triggered.await;
            },
            options,
        ));
        let config = json!({"jsonrpc": "2.0", "method": "SetConfig", "params": {}, "id": 1});
        for line in [config.to_string(), request(2, summary)] {
            client_write
                .write_all(format!("{line}\n").as_bytes())
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        // stdin stays open: only the shutdown can end the session.
        trigger.send(()).unwrap();
        channel.await.unwrap().unwrap();
        drop(client_write);
        let mut out = String::new();
        client_read.read_to_string(&mut out).await.unwrap();
        out.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_still_delivers_slow_response() {
        let lines = shutdown_during("slow", Duration::from_secs(5)).await;
        assert!(
            lines
                .iter()
                .any(|l| l["id"] == 2 && l.get("result").is_some())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_request_is_abandoned_after_drain_timeout() {
        let start = tokio::time::Instant::now();
        let lines = shutdown_during("stuck", Duration::from_secs(5)).await;
        assert!(lines.iter().all(|l| l["id"] != 2));
        assert!(lines.iter().any(|l| l["id"] == 1));
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_secs(5) && waited < Duration::from_secs(6),
            "{waited:?}"
        );
    }
}
