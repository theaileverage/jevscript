//! The installed CLI selects local System One profiles and parses typed answers (spec sections 10.6 and 11.6).

use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/custom-decision-model")
        .join(name)
}

fn request(stream: &mut TcpStream) -> (String, String, Value) {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("request line");
    let mut authorization = String::new();
    let mut content_length = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).expect("header");
        if header == "\r\n" {
            break;
        }
        let (name, value) = header.split_once(':').expect("header name and value");
        if name.eq_ignore_ascii_case("authorization") {
            authorization = value.trim().to_string();
        }
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(value.trim().parse::<usize>().expect("content length"));
        }
    }
    let mut body = vec![0; content_length.expect("request body length")];
    reader.read_exact(&mut body).expect("request body");
    (
        request_line.trim().to_string(),
        authorization,
        serde_json::from_slice(&body).expect("request JSON"),
    )
}

fn answer(question: &Value) -> Value {
    match question["type"].as_str().expect("question type") {
        "noul" => json!({ "noul": 0.73 }),
        other => panic!("unknown question type {other}"),
    }
}

fn serve(listener: TcpListener) -> (String, String, Value) {
    let (mut stream, _) = listener.accept().expect("CLI request");
    let captured = request(&mut stream);
    let questions = captured.2["questions"].as_object().expect("questions");
    let answers: Map<String, Value> = questions
        .iter()
        .map(|(id, question)| (id.clone(), answer(question)))
        .collect();
    let response = json!({
        "model": "fixture served configuration",
        "answers": answers,
        "usage": { "input_tokens": 20, "output_tokens": 0 }
    })
    .to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    )
    .expect("response");
    captured
}

fn judge(source: &Path, judgment: &str, model: &str, profiles: &Path) -> Value {
    let binary =
        std::env::var_os("JEVSCRIPT_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_jevscript").into());
    let output = Command::new(binary)
        .arg("judge")
        .arg(source)
        .arg(judgment)
        .arg("--state")
        .arg(r#"{"message":"I need help before my meeting starts in an hour"}"#)
        .arg("--model")
        .arg(model)
        .arg("--profiles")
        .arg(profiles)
        .env("TYPESAFE_API_KEY", "local")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("CLI runs");
    assert!(
        output.status.success(),
        "{}: {}",
        model,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI answer JSON")
}

#[test]
fn local_profiles_select_the_server_and_decode_system_one_answers() {
    let mut profiles: Vec<Value> =
        serde_json::from_slice(&std::fs::read(example("profiles.json")).expect("example profiles"))
            .expect("valid profiles");
    let models = ["english", "kev-latest", "openjev", "openjev-latest"];
    assert_eq!(profiles.len(), models.len());
    let mut servers = Vec::new();
    for profile in &mut profiles {
        let model = profile["model"]
            .as_str()
            .expect("profile model")
            .to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback server");
        let port = listener.local_addr().expect("server address").port();
        profile["endpoint"] = json!(format!("http://127.0.0.1:{port}/v1/systemone"));
        servers.push((model, std::thread::spawn(move || serve(listener))));
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("custom-decision-model");
    std::fs::create_dir_all(&dir).expect("test directory");
    let local_profiles = dir.join("profiles.json");
    std::fs::write(&local_profiles, serde_json::to_vec(&profiles).unwrap())
        .expect("local profiles");

    let urgent = example("urgent.jev");
    for model in models {
        let result = judge(&urgent, "classify", model, &local_profiles);
        assert_eq!(result["urgent"], json!({ "$jev": "prob", "value": 0.73 }));
    }

    for (model, server) in servers {
        let (line, bearer, body) = server.join().expect("server completed");
        assert_eq!(line, "POST /v1/systemone HTTP/1.1");
        assert_eq!(bearer, "Bearer local");
        assert_eq!(body["model"], model);
        assert_eq!(
            body["state"],
            json!({ "message": "I need help before my meeting starts in an hour" })
        );
        assert_eq!(body["questions"].as_object().unwrap().len(), 1);
    }
}
