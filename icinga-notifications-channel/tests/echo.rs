//! Runs the `echo` example as a separate process and speaks the protocol
//! over its stdin and stdout.
use std::{
    io::Write,
    process::{Command, Stdio},
};

use serde_json::{Value, json};

#[test]
fn echo_session() {
    let notification = json!({"version": 1, "payload": {
        "contact": {"full_name": "admin", "addresses": []},
        "object": {"name": "o", "url": "http://x", "sources": [], "tags": {}},
        "event": {"time": "2024-07-12T10:47:30Z", "summary": "s", "message": "m"},
        "summary": "hello", "body": "b"}});
    let requests = [
        json!({"jsonrpc": "2.0", "method": "GetInfo", "id": 1}),
        json!({"jsonrpc": "2.0", "method": "SetConfig", "params": {"prefix": "p"}, "id": 2}),
        json!({"jsonrpc": "2.0", "method": "SendNotification", "params": notification, "id": 3}),
    ];
    // Cargo only exposes binaries, not examples, to integration tests.
    let mut child = Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--example", "echo"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());

    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let by_id = |id: u64| lines.iter().find(|l| l["id"] == id).unwrap();
    assert_eq!(by_id(1)["result"]["name"], "Echo");
    assert_eq!(by_id(2)["result"], Value::Null);
    assert_eq!(by_id(3)["result"]["details"][0]["value"], "p: hello");
    let log = lines
        .iter()
        .find(|l| l["method"] == "channel::Log")
        .unwrap();
    assert_eq!(log["params"]["message"], "echoing");
    assert_eq!(log["params"]["fields"], json!(["contact", "admin"]));
    assert_eq!(lines.len(), 4);
}
