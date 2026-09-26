//! Protocol sessions run in process over `lsp-server`'s memory connection:
//! position-encoding negotiation, and responsiveness on a file far larger
//! than any in the repository, typed into while broken.

use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};

struct Client {
    connection: Connection,
    server: Option<std::thread::JoinHandle<()>>,
    next_id: i32,
}

impl Client {
    fn start(capabilities: Value) -> (Self, Value) {
        let (server, connection) = Connection::memory();
        let handle = std::thread::spawn(move || {
            jevscript_lsp::run(server).expect("the session ends cleanly");
        });
        let mut client = Client {
            connection,
            server: Some(handle),
            next_id: 1,
        };
        let init = client.request(
            "initialize",
            json!({ "processId": null, "rootUri": null, "capabilities": capabilities }),
        );
        client.notify("initialized", json!({}));
        (client, init)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.connection
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(id),
                method.to_string(),
                params,
            )))
            .expect("sends");
        loop {
            match self.receive() {
                Message::Response(response) if response.id == RequestId::from(id) => {
                    return response
                        .response_result
                        .unwrap_or_else(|error| panic!("{method}: {error:?}"));
                }
                _ => {}
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.connection
            .sender
            .send(Message::Notification(Notification::new(
                method.to_string(),
                params,
            )))
            .expect("sends");
    }

    fn receive(&mut self) -> Message {
        self.connection
            .receiver
            .recv_timeout(Duration::from_secs(60))
            .expect("the server answers")
    }

    fn diagnostics(&mut self) -> Vec<Value> {
        loop {
            if let Message::Notification(n) = self.receive()
                && n.method == "textDocument/publishDiagnostics"
            {
                return n.params["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
            }
        }
    }

    fn stop(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        if let Some(server) = self.server.take() {
            server.join().expect("the server thread ends");
        }
    }
}

/// A program of `copies` judgments, defs and tasks: every construct of the
/// inline coding harness, repeated.
fn large_program(copies: usize) -> String {
    let mut out = String::from(
        "program large\n\nin  issue: { title, body, branch }\nout pr_url\n\nneeds claude: agent\nneeds tree:   tool\nneeds me:     person\n\n",
    );
    for i in 0..copies {
        out.push_str(&format!(
            r#"# Reads the agent, copy {i}.
judgment read_agent_{i}(summary, files, tests, recent):
  claims_done = summary feels "states the work is complete":
    focus "An explicit statement that the work is finished, not a plan."
  off_scope   = each files feels "is unrelated to the issue: {{issue.title}}"
  next        = summary pick:
    keep_working  "making progress and needs nothing"
    stuck         "looping or unsure how to proceed"
    needs_me      "asks a question only a person can answer"
    other

def score_{i}(values, floor = 0.5):
  kept = [v for v in values if v > floor]
  if len(kept) == 0:
    return 0
  return sum(kept) / len(kept)

task step_{i}(dev) budget calls 40, minutes 30 thresholds risk_confirm 0.2, min_confidence 0.5:
  loop max 3:
    obs = shape:
      summary  focus dev.observe.last_message on "what the agent says it did", max 2k
      files    tree.diff.files, max 500
      tests    tree.test_summary, max 300
      recent   trail 6
    j = read_agent_{i} obs
    if stuck(obs.recent) or j.next is stuck:
      dev.send "Stop. In three lines, what is blocking you?"
      continue
    gate risk max(j.off_scope), confidence j.next.confidence:
      confirm  -> me.ask "Agent touched {{count(j.off_scope, above 0.6)}} unrelated files. Continue?"
      escalate -> me.take_over
      proceed  ->
        if j.claims_done > score_{i}([0.8]):
          dev.send "Tests fail:\n{{obs.tests}}"

"#
        ));
    }
    out.push_str("task main:\n  tree.create issue.branch\n  dev = claude.spawn in tree, prompt issue.body\n  step_0(dev)\n  dev.stop\n  pr_url = tree.open_pr\n");
    out
}

#[test]
fn utf8_positions_when_the_client_offers_them() {
    let (client, init) =
        Client::start(json!({ "general": { "positionEncodings": ["utf-8", "utf-16"] } }));
    assert_eq!(init["capabilities"]["positionEncoding"], "utf-8");
    client.stop();
}

#[test]
fn a_large_file_stays_responsive_while_broken() {
    let source = large_program(150);
    let lines = source.lines().count();
    assert!(lines > 5000, "{lines} lines");
    let (mut client, _) = Client::start(json!({}));
    let uri = "file:///virtual/large.jev";

    let started = Instant::now();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "jevscript", "version": 1, "text": source } }),
    );
    let clean = client.diagnostics();
    let open = started.elapsed();
    assert!(
        clean.iter().all(|d| d["severity"] != 1),
        "{:?}",
        clean.iter().find(|d| d["severity"] == 1)
    );

    // Break a line in the middle of the file, as typing does.
    let line = source
        .lines()
        .position(|l| l == "    j = read_agent_75 obs")
        .expect("the middle copy");
    let started = Instant::now();
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{
                "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 0 } },
                "text": "    x = j.\n",
            }],
        }),
    );
    let broken = client.diagnostics();
    let change = started.elapsed();
    assert_eq!(broken.len(), 1, "{broken:?}");

    let started = Instant::now();
    let completion = client.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": 10 } }),
    );
    let tokens = client.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": uri } }),
    );
    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let requests = started.elapsed();
    assert!(!completion.as_array().expect("items").is_empty());
    assert!(tokens["data"].as_array().is_some_and(|d| d.len() > 10_000));
    // Every other unit is still in the outline while one is broken.
    assert!(symbols.as_array().expect("symbols").len() > 450);

    // Debug builds on a shared CI runner are slow; the bound is generous and
    // still catches anything quadratic in the file size.
    let budget = Duration::from_secs(10);
    assert!(open < budget, "open took {open:?}");
    assert!(change < budget, "a keystroke took {change:?}");
    assert!(requests < budget, "requests took {requests:?}");
    eprintln!("{lines} lines: open {open:?}, keystroke {change:?}, requests {requests:?}");
    client.stop();
}
