//! The single owner of the output stream.
//!
//! Responses and `channel::Log` notifications are produced concurrently but
//! must reach the stream as whole lines. Both go through one queue that one
//! writer drains.

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_util::{SinkExt, StreamExt, future::ready};
use jsonrpc_rs::server::JsonRpcResponse;
use serde_json::Value;
use tokio::io::AsyncWrite;
use tokio_util::codec::{FramedWrite, LinesCodec, LinesCodecError};

/// Something to write to the output.
pub(crate) enum Outgoing {
    /// A response of the JSON-RPC server.
    Response(JsonRpcResponse),
    /// A complete notification, e.g. `channel::Log`.
    Log(Value),
}

pub(crate) type OutgoingSender = UnboundedSender<Outgoing>;

/// A queue for [`Outgoing`] items and the receiving end for [`write_all`].
pub(crate) fn queue() -> (OutgoingSender, UnboundedReceiver<Outgoing>) {
    unbounded()
}

/// A sink for the server that queues its responses.
pub(crate) fn response_sink(
    sender: OutgoingSender,
) -> impl futures_util::Sink<JsonRpcResponse, Error = futures_channel::mpsc::SendError> {
    sender.with(|response| ready(Ok(Outgoing::Response(response))))
}

/// Writes queued items as lines until every sender is gone or dropped.
pub(crate) async fn write_all(
    mut queue: UnboundedReceiver<Outgoing>,
    write: impl AsyncWrite,
) -> Result<(), LinesCodecError> {
    let mut write = Box::pin(FramedWrite::new(write, LinesCodec::new()));
    while let Some(item) = queue.next().await {
        // Serializing JSON values and responses cannot fail: all keys are strings.
        let line = match item {
            Outgoing::Response(response) => serde_json::to_string(&response),
            Outgoing::Log(value) => serde_json::to_string(&value),
        }
        .unwrap();
        write.send(line).await?;
    }
    SinkExt::<String>::close(&mut write).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpc_rs::server::{JsonRpcResponseSingle, JsonRpcResult};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, duplex};

    fn response() -> JsonRpcResponse {
        JsonRpcResponse::Single(JsonRpcResponseSingle::new(JsonRpcResult::Ok(Box::new(
            "x".repeat(1000),
        ))))
    }

    async fn read_to_end(mut read: impl AsyncReadExt + Unpin) -> String {
        let mut out = String::new();
        read.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn lines_are_never_interleaved() {
        // A tiny buffer forces partial writes.
        let (write, read) = duplex(16);
        let (tx, rx) = queue();
        let writer = tokio::spawn(write_all(rx, write));
        let mut tasks = Vec::new();
        for i in 0..20 {
            let mut sink = Box::pin(response_sink(tx.clone()));
            let log = tx.clone();
            tasks.push(tokio::spawn(async move {
                sink.send(response()).await.unwrap();
                log.unbounded_send(Outgoing::Log(json!({"log": i, "pad": "y".repeat(500)})))
                    .unwrap();
            }));
        }
        drop(tx);
        for task in tasks {
            task.await.unwrap();
        }
        let out = read_to_end(read).await;
        writer.await.unwrap().unwrap();
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines.len(), 40);
        for line in lines {
            serde_json::from_str::<Value>(line).expect("each line is complete JSON");
        }
    }

    #[tokio::test]
    async fn closing_one_sender_keeps_the_others_working() {
        let (write, read) = duplex(1024);
        let (tx, rx) = queue();
        let writer = tokio::spawn(write_all(rx, write));
        let mut sink = Box::pin(response_sink(tx.clone()));
        sink.send(response()).await.unwrap();
        sink.close().await.unwrap();
        tx.unbounded_send(Outgoing::Log(json!({"after": "close"})))
            .unwrap();
        drop((tx, sink));
        let out = read_to_end(read).await;
        writer.await.unwrap().unwrap();
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(out.lines().last().unwrap().contains("after"));
    }
}
