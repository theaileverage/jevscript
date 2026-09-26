//! JSON-RPC framing acceptance for `jevscript serve` (spec section 11.5).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn server() -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("starts server")
}

fn exchange(child: &mut std::process::Child, line: &str) -> serde_json::Value {
    writeln!(child.stdin.as_mut().expect("stdin"), "{line}").expect("writes request");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .flush()
        .expect("flushes");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().expect("stdout"))
        .read_line(&mut line)
        .expect("reads response");
    serde_json::from_str(&line).expect("response is JSON")
}

fn write_line(child: &mut std::process::Child, value: &serde_json::Value) {
    writeln!(child.stdin.as_mut().expect("stdin"), "{value}").expect("writes message");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .flush()
        .expect("flushes");
}

fn read_value(reader: &mut BufReader<std::process::ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("reads message");
    serde_json::from_str(&line).expect("message is JSON")
}

#[test]
fn malformed_json_and_invalid_valid_json_have_distinct_errors() {
    let mut child = server();
    let malformed = exchange(&mut child, "{");
    assert_eq!(malformed["error"]["code"], -32700);
    let invalid = exchange(&mut child, "[]");
    assert_eq!(invalid["error"]["code"], -32600);
    drop(child.stdin.take());
    child.wait().expect("server exits");
}

#[test]
fn notifications_are_dispatched_without_a_reply() {
    let mut child = server();
    let stdin = child.stdin.as_mut().expect("stdin");
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"method\":\"program.destroy\",\"params\":{{}}}}"
    )
    .expect("writes notification");
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"program.destroy\",\"params\":{{}}}}"
    )
    .expect("writes request");
    stdin.flush().expect("flushes");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().expect("stdout"))
        .read_line(&mut line)
        .expect("reads response");
    let response: serde_json::Value = serde_json::from_str(&line).expect("response is JSON");
    assert_eq!(response["id"], 9);
    assert_eq!(response["error"]["code"], -32601);
    drop(child.stdin.take());
    child.wait().expect("server exits");
}

#[test]
fn explicit_null_id_is_answered_but_invalid_id_types_are_rejected_with_null() {
    let mut child = server();

    let explicit_null = exchange(
        &mut child,
        r#"{"jsonrpc":"2.0","id":null,"method":"program.destroy","params":{}}"#,
    );
    assert!(explicit_null["id"].is_null());
    assert_eq!(explicit_null["error"]["code"], -32601);

    for invalid_id in ["{}", "[]", "true"] {
        let response = exchange(
            &mut child,
            &format!(
                r#"{{"jsonrpc":"2.0","id":{invalid_id},"method":"program.destroy","params":{{}}}}"#
            ),
        );
        assert!(response["id"].is_null(), "invalid id {invalid_id}");
        assert_eq!(response["error"]["code"], -32600, "invalid id {invalid_id}");
    }

    drop(child.stdin.take());
    child.wait().expect("server exits");
}

#[test]
fn program_load_requires_exactly_one_source_form() {
    let mut child = server();
    for params in [
        r#"{}"#,
        r#"{"path":"example.jev","source":"program example"}"#,
    ] {
        let response = exchange(
            &mut child,
            &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"program.load","params":{params}}}"#),
        );
        assert_eq!(response["error"]["code"], -32602, "params {params}");
    }
    drop(child.stdin.take());
    child.wait().expect("server exits");
}

#[test]
fn malformed_inflight_abort_ids_do_not_cancel_the_run() {
    let mut child = server();
    let loaded = exchange(
        &mut child,
        r#"{"jsonrpc":"2.0","id":1,"method":"program.load","params":{"source":"program ids\n\nneeds tree: tool\n\ntask main:\n  tree.one\n  tree.two\n"}}"#,
    );
    let program_id = loaded["result"]["program_id"]
        .as_str()
        .expect("program id")
        .to_string();
    let started = exchange(
        &mut child,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "task.start",
            "params": {
                "program_id": program_id,
                "name": "main",
                "inputs": {},
                "bindings": [{ "name": "tree", "kind": "tool" }],
            },
        })
        .to_string(),
    );
    let run_id = started["result"]["run_id"]
        .as_str()
        .expect("run id")
        .to_string();

    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    write_line(
        &mut child,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "run.next",
            "params": { "run_id": run_id },
        }),
    );
    let first_call = loop {
        let message = read_value(&mut reader);
        if message["method"] == "capability.call" {
            break message;
        }
    };
    assert_eq!(first_call["params"]["verb"], "one");

    for invalid_id in [
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!(true),
    ] {
        write_line(
            &mut child,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": invalid_id,
                "method": "run.abort",
                "params": { "run_id": run_id },
            }),
        );
        let response = read_value(&mut reader);
        assert!(response["id"].is_null());
        assert_eq!(response["error"]["code"], -32600);
    }

    write_line(
        &mut child,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": first_call["id"],
            "result": { "result": null },
        }),
    );
    let second_call = loop {
        let message = read_value(&mut reader);
        if message["method"] == "capability.call" {
            break message;
        }
    };
    assert_eq!(
        second_call["params"]["verb"], "two",
        "invalid aborts did not cancel"
    );
    write_line(
        &mut child,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": second_call["id"],
            "result": { "result": null },
        }),
    );
    let done = loop {
        let message = read_value(&mut reader);
        if message["id"] == 3 {
            break message;
        }
    };
    assert_eq!(done["result"]["kind"], "done");

    drop(child.stdin.take());
    child.wait().expect("server exits");
}
