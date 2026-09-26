//! `jevscript lsp` end to end: a real process, real `Content-Length` framing
//! on stdio, and a session over the shipped examples. Each step names the
//! editor feature and, where one applies, the spec rule behind it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

struct Session {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
    /// Notifications that arrived while waiting for a response.
    pending: Vec<Value>,
}

impl Session {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jevscript"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("starts the language server");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, incoming) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 {
                        return;
                    }
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(value) = header.strip_prefix("Content-Length: ") {
                        length = value.parse::<usize>().ok();
                    }
                }
                let mut body = vec![0; length.expect("a Content-Length header")];
                reader.read_exact(&mut body).expect("reads a body");
                let value: Value = serde_json::from_slice(&body).expect("a JSON body");
                if sender.send(value).is_err() {
                    return;
                }
            }
        });
        Session {
            child,
            stdin,
            incoming,
            next_id: 1,
            pending: Vec::new(),
        }
    }

    fn send(&mut self, value: &Value) {
        let body = value.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("writes");
        self.stdin.flush().expect("flushes");
    }

    fn receive(&mut self) -> Value {
        self.incoming
            .recv_timeout(Duration::from_secs(30))
            .expect("the server answers within 30s")
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.receive();
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
            self.pending.push(message);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// The next diagnostics published for `uri`.
    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        if let Some(at) = self.pending.iter().position(|m| {
            m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri
        }) {
            let message = self.pending.remove(at);
            return message["params"]["diagnostics"]
                .as_array()
                .cloned()
                .unwrap_or_default();
        }
        loop {
            let message = self.receive();
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
            }
            self.pending.push(message);
        }
    }
}

fn examples() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"))
        .expect("the examples exist")
}

fn uri(path: &Path) -> String {
    format!("file://{}", path.display()).replace(' ', "%20")
}

fn position_of(text: &str, needle: &str) -> Value {
    let offset = text.find(needle).unwrap_or_else(|| panic!("`{needle}`"));
    let line = text[..offset].matches('\n').count();
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let character: usize = text[line_start..offset].encode_utf16().count();
    json!({ "line": line, "character": character })
}

#[test]
fn a_session_over_the_shipped_examples() {
    let examples = examples();
    let mut session = Session::start();
    let init = session.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": uri(&examples),
            "workspaceFolders": [{ "uri": uri(&examples), "name": "examples" }],
            "capabilities": {},
        }),
    );
    let capabilities = &init["capabilities"];
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert_eq!(
        capabilities["textDocumentSync"]["change"], 2,
        "incremental sync"
    );
    assert_eq!(init["serverInfo"]["name"], "jevscript");
    session.notify("initialized", json!({}));

    // Diagnostics: every shipped example compiles without an error, the
    // library user linked against its library (spec 3.9).
    let names = [
        "fix_issue.jev",
        "lib/agent_loop.jev",
        "fix_issue_inline.jev",
        "review_loop.jev",
        "chief_of_staff.jev",
        "inbox_triage.jev",
    ];
    let mut texts = std::collections::BTreeMap::new();
    for name in names {
        let path = examples.join(name);
        let text = std::fs::read_to_string(&path).expect("reads");
        session.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri(&path), "languageId": "jevscript", "version": 1, "text": text,
            }}),
        );
        texts.insert(name, (uri(&path), text));
    }
    for name in names {
        let (file, _) = &texts[name];
        let found = session.diagnostics(file);
        let errors: Vec<&Value> = found.iter().filter(|d| d["severity"] == 1).collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
        for diagnostic in &found {
            assert_eq!(diagnostic["source"], "jevscript");
            assert!(
                diagnostic["codeDescription"]["href"]
                    .as_str()
                    .is_some_and(|href| href.contains("error-reference.md#"))
            );
        }
    }

    let (root, root_text) = texts["fix_issue.jev"].clone();
    let (inline, inline_text) = texts["fix_issue_inline.jev"].clone();

    // Go-to-definition across `use`.
    let found = session.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": root }, "position": position_of(&root_text, "watch(dev") }),
    );
    let target = found[0]["uri"].as_str().expect("a location");
    assert!(target.ends_with("lib/agent_loop.jev"), "{found}");

    // Find-references from the library back into its importer.
    let (library, library_text) = texts["lib/agent_loop.jev"].clone();
    let found = session.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": library },
            "position": position_of(&library_text, "watch(dev"),
            "context": { "includeDeclaration": true },
        }),
    );
    let files: Vec<&str> = found
        .as_array()
        .expect("locations")
        .iter()
        .filter_map(|l| l["uri"].as_str())
        .collect();
    assert!(
        files.iter().any(|f| f.ends_with("/fix_issue.jev")),
        "{files:?}"
    );

    // Hover: a keyword documented from the spec, and a unit's signature.
    let hover = session.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": inline }, "position": position_of(&inline_text, "feels") }),
    );
    let value = hover["contents"]["value"].as_str().expect("markdown");
    assert!(value.contains("Spec section 6.2"), "{value}");
    let hover = session.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": inline }, "position": position_of(&inline_text, "read_agent obs") }),
    );
    let value = hover["contents"]["value"].as_str().expect("markdown");
    assert!(
        value.contains("judgment read_agent(summary, files, tests, recent)"),
        "{value}"
    );

    // The outline, folding and semantic tokens.
    let symbols = session.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": inline } }),
    );
    let names: Vec<&str> = symbols
        .as_array()
        .expect("symbols")
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(
        names.contains(&"read_agent") && names.contains(&"main"),
        "{names:?}"
    );
    let folds = session.request(
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": inline } }),
    );
    assert!(folds.as_array().is_some_and(|f| f.len() > 5), "{folds}");
    let tokens = session.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": inline } }),
    );
    let data = tokens["data"].as_array().expect("token data");
    assert!(!data.is_empty() && data.len().is_multiple_of(5));

    // Mid-edit: an incremental change that breaks one line. The server
    // reports the syntax error, and completion still knows the scope.
    let at = position_of(&inline_text, "    if stuck(obs.recent)");
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": inline, "version": 2 },
            "contentChanges": [{
                "range": { "start": at, "end": at },
                "text": "    if j.\n",
            }],
        }),
    );
    let broken = session.diagnostics(&inline);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0]["code"], "syntax");
    let cursor = json!({ "line": at["line"], "character": 9 });
    let completion = session.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": inline }, "position": cursor }),
    );
    let labels: Vec<&str> = completion
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert_eq!(labels.len(), 3, "{labels:?}");
    assert!(
        labels.contains(&"claims_done") && labels.contains(&"next"),
        "{labels:?}"
    );
    // The outline survives the broken line: the judgment still parses.
    let symbols = session.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": inline } }),
    );
    assert!(symbols.to_string().contains("read_agent"));

    // Undo the edit: clean again.
    let end = json!({ "line": at["line"].as_u64().expect("line") + 1, "character": 0 });
    session.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": inline, "version": 3 },
            "contentChanges": [{ "range": { "start": at, "end": end }, "text": "" }],
        }),
    );
    let fixed = session.diagnostics(&inline);
    assert!(fixed.iter().all(|d| d["severity"] != 1), "{fixed:?}");

    session.request("shutdown", Value::Null);
    session.notify("exit", Value::Null);
    let status = session.child.wait().expect("the server exits");
    assert!(status.success(), "{status}");
}

#[test]
fn an_unsaved_edit_to_an_open_library_reaches_its_importer() {
    // Spec 3.9: a library's errors are reported against the library. The
    // importer is linked against the library's open buffer, saved or not, and
    // shows the library's error on its `use` line with the library location.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lsp_unsaved_library");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch dir");
    let dir = std::fs::canonicalize(&dir).expect("canonical");
    let saved = "program lib\n\ndef helper(x):\n  return x\n";
    let importer_text =
        "program main\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = lib.helper(1)\n";
    std::fs::write(dir.join("lib.jev"), saved).expect("writes");
    std::fs::write(dir.join("main.jev"), importer_text).expect("writes");
    let library = uri(&dir.join("lib.jev"));
    let importer = uri(&dir.join("main.jev"));

    let mut session = Session::start();
    session.request(
        "initialize",
        json!({ "processId": null, "rootUri": uri(&dir), "capabilities": {} }),
    );
    session.notify("initialized", json!({}));
    for (file, text) in [(&library, saved), (&importer, importer_text)] {
        session.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": file, "languageId": "jevscript", "version": 1, "text": text } }),
        );
    }
    assert_eq!(session.diagnostics(&library), Vec::<Value>::new());
    assert_eq!(session.diagnostics(&importer), Vec::<Value>::new());

    let edit = |session: &mut Session, version: i32, body: &str| {
        session.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": library, "version": version },
                "contentChanges": [{
                    "range": { "start": { "line": 3, "character": 9 }, "end": { "line": 3, "character": 10 } },
                    "text": body,
                }],
            }),
        );
    };

    // Break the library without saving it.
    edit(&mut session, 2, "y");
    let own = session.diagnostics(&library);
    assert_eq!(own.len(), 1, "{own:?}");
    assert_eq!(own[0]["code"], "unassigned_read");
    let seen = session.diagnostics(&importer);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["code"], "unassigned_read");
    assert_eq!(seen[0]["range"]["start"]["line"], 2, "on the `use` line");
    let related = &seen[0]["relatedInformation"][0]["location"];
    assert_eq!(related["uri"], library.as_str());
    assert_eq!(
        related["range"]["start"],
        json!({ "line": 3, "character": 9 })
    );

    // Fix it, still unsaved: both are clean again.
    edit(&mut session, 3, "x");
    assert_eq!(session.diagnostics(&library), Vec::<Value>::new());
    assert_eq!(session.diagnostics(&importer), Vec::<Value>::new());

    // Break it again and close it unsaved: the importer goes back to the
    // saved file, which is fine.
    edit(&mut session, 4, "y");
    assert_eq!(session.diagnostics(&library).len(), 1);
    assert_eq!(session.diagnostics(&importer).len(), 1);
    session.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": library } }),
    );
    assert_eq!(session.diagnostics(&library), Vec::<Value>::new());
    assert_eq!(session.diagnostics(&importer), Vec::<Value>::new());

    session.request("shutdown", Value::Null);
    session.notify("exit", Value::Null);
    assert!(session.child.wait().expect("exits").success());
}

#[test]
fn opening_an_unsaved_new_library_reaches_its_open_importer() {
    // Spec 3.9: the importer is linked against a library that exists only as
    // an open buffer, and is republished when that buffer opens, changes and
    // closes.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lsp_new_library");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch dir");
    let dir = std::fs::canonicalize(&dir).expect("canonical");
    let importer_text =
        "program main\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = lib.helper(1)\n";
    std::fs::write(dir.join("main.jev"), importer_text).expect("writes");
    let library = uri(&dir.join("lib.jev"));
    let importer = uri(&dir.join("main.jev"));

    let mut session = Session::start();
    session.request(
        "initialize",
        json!({ "processId": null, "rootUri": uri(&dir), "capabilities": {} }),
    );
    session.notify("initialized", json!({}));
    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": importer, "languageId": "jevscript", "version": 1, "text": importer_text } }),
    );
    let missing = session.diagnostics(&importer);
    assert_eq!(missing[0]["code"], "use_not_found", "{missing:?}");

    session.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": library, "languageId": "jevscript", "version": 1,
            "text": "program lib\n\ndef helper(x):\n  return x\n" } }),
    );
    assert_eq!(session.diagnostics(&library), Vec::<Value>::new());
    assert_eq!(session.diagnostics(&importer), Vec::<Value>::new());

    session.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": library, "version": 2 }, "contentChanges": [{
            "range": { "start": { "line": 3, "character": 9 }, "end": { "line": 3, "character": 10 } },
            "text": "y" }] }),
    );
    assert_eq!(session.diagnostics(&library).len(), 1);
    let seen = session.diagnostics(&importer);
    assert_eq!(seen[0]["code"], "unassigned_read", "{seen:?}");

    session.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": library } }),
    );
    assert_eq!(session.diagnostics(&library), Vec::<Value>::new());
    let back = session.diagnostics(&importer);
    assert_eq!(back[0]["code"], "use_not_found", "{back:?}");

    session.request("shutdown", Value::Null);
    session.notify("exit", Value::Null);
    assert!(session.child.wait().expect("exits").success());
}
