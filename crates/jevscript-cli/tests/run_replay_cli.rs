//! Offline `run` and `replay` acceptance (spec sections 10.3 and 11.6).

use std::process::Command;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("jevscript-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("creates temp directory");
    path
}

fn fixture(path: &std::path::Path) {
    std::fs::write(
        path,
        "program offline\n\nneeds tree: tool\n\ntask main:\n  tree.touch\n",
    )
    .expect("writes fixture");
}

#[test]
fn explicit_stub_records_and_replays_without_source_or_ambient_services() {
    let dir = temp_dir("record-replay");
    let source = dir.join("offline.jev");
    let recording = dir.join("run.jsonl");
    fixture(&source);

    let live = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args([
            "run",
            source.to_str().unwrap(),
            "--stub",
            "tree",
            "--record",
        ])
        .arg(&recording)
        .arg("--redact")
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("runs CLI");
    assert!(
        live.status.success(),
        "{}",
        String::from_utf8_lossy(&live.stderr)
    );
    let stderr = String::from_utf8_lossy(&live.stderr);
    assert!(stderr.contains("[stub]"), "{stderr}");
    assert!(stderr.contains("sensitive replay companion"), "{stderr}");
    assert!(recording.exists());
    assert!(
        jevscript_runtime::record::companion_path(&recording).exists(),
        "redacted recordings include their private replay companion"
    );

    std::fs::remove_file(&source).expect("removes source");
    let replay = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["replay", recording.to_str().unwrap()])
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("replays CLI recording");
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert!(String::from_utf8_lossy(&replay.stdout).contains("\"kind\":\"done\""));
}

#[test]
fn unbound_real_capabilities_do_not_silently_use_stubs() {
    let dir = temp_dir("unbound");
    let source = dir.join("offline.jev");
    fixture(&source);
    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["run", source.to_str().unwrap()])
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .expect("runs CLI");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("was not bound"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_existing_primary_is_refused_before_the_stub_runs() {
    let dir = temp_dir("existing-primary");
    let source = dir.join("offline.jev");
    let recording = dir.join("run.jsonl");
    fixture(&source);
    std::fs::write(&recording, "keep me\n").expect("writes sentinel recording");

    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args([
            "run",
            source.to_str().unwrap(),
            "--stub",
            "tree",
            "--record",
        ])
        .arg(&recording)
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("runs CLI");

    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(&recording).expect("reads sentinel recording"),
        "keep me\n"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("[stub:tree]"),
        "the external effect ran before the destination was refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_existing_redaction_companion_is_refused_without_creating_the_primary() {
    let dir = temp_dir("existing-companion");
    let source = dir.join("offline.jev");
    let recording = dir.join("run.jsonl");
    let companion = jevscript_runtime::record::companion_path(&recording);
    fixture(&source);
    std::fs::write(&companion, "keep me private\n").expect("writes companion sentinel");

    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args([
            "run",
            source.to_str().unwrap(),
            "--stub",
            "tree",
            "--record",
        ])
        .arg(&recording)
        .arg("--redact")
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("runs CLI");

    assert!(!output.status.success());
    assert!(!recording.exists(), "a partial primary was left behind");
    assert_eq!(
        std::fs::read_to_string(&companion).expect("reads companion sentinel"),
        "keep me private\n"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("[stub:tree]"),
        "the external effect ran before the destination was refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn terminal_pauses_reap_subprocess_adapters_before_exit() {
    for (name, body, reply, exit_code) in [
        (
            "stopped",
            "  tree.touch\n  stop \"done\"\n",
            r#"{"result":null}"#,
            3,
        ),
        (
            "escalated",
            "  tree.touch\n  escalate \"help\"\n",
            r#"{"result":null}"#,
            3,
        ),
        (
            "adapter-error",
            "  tree.touch\n",
            r#"{"error":{"message":"no","retryable":false}}"#,
            1,
        ),
        (
            "declined-retry",
            "  tree.touch\n",
            r#"{"error":{"message":"later","retryable":true}}"#,
            1,
        ),
    ] {
        let dir = temp_dir(name);
        let source = dir.join("cleanup.jev");
        let pid_file = dir.join("adapter.pid");
        std::fs::write(
            &source,
            format!("program cleanup\n\nneeds tree: tool\n\ntask main:\n{body}"),
        )
        .expect("writes cleanup fixture");
        let adapter = format!(
            "trap '' TERM; printf '%s\\n' \"$$\" > {}; IFS= read -r line; printf '%s\\n' '{}'; exec sleep 30",
            pid_file.display(),
            reply
        );

        let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
            .args(["run", source.to_str().unwrap(), "--bind"])
            .arg(format!("tree={adapter}"))
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("JEVSCRIPT_PROFILES")
            .output()
            .expect("runs CLI");
        assert_eq!(
            output.status.code(),
            Some(exit_code),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let pid = std::fs::read_to_string(&pid_file)
            .expect("adapter wrote its pid")
            .trim()
            .to_string();
        let mut exited = false;
        for _ in 0..20 {
            if !process_exists(&pid) {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        if !exited {
            let _ = Command::new("kill").args(["-KILL", &pid]).status();
        }
        assert!(exited, "{name}: adapter process {pid} survived");
    }
}

#[cfg(unix)]
fn process_exists(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("checks adapter pid")
        .success()
}

#[cfg(unix)]
#[test]
fn replayed_runtime_errors_exit_one() {
    let dir = temp_dir("replayed-error");
    let source = dir.join("error.jev");
    let recording = dir.join("error.jsonl");
    std::fs::write(
        &source,
        "program replay_error\n\nneeds tree: tool\n\ntask main:\n  tree.touch\n",
    )
    .expect("writes error fixture");
    let adapter = r#"while IFS= read -r line; do printf '%s\n' '{"error":{"message":"no","retryable":false}}'; done"#;

    let live = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["run", source.to_str().unwrap(), "--bind"])
        .arg(format!("tree={adapter}"))
        .arg("--record")
        .arg(&recording)
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("records runtime error");
    assert_eq!(live.status.code(), Some(1));

    let replay = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["replay", recording.to_str().unwrap()])
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("replays runtime error");
    assert_eq!(
        replay.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
}

#[test]
fn replay_consumes_a_recorded_escalation_resume_before_done() {
    let dir = temp_dir("resumed-escalation");
    let recording = dir.join("escalation.jsonl");
    let ir = jevscript_compiler::compile_source(
        "program resumed\n\nout continued\n\ntask main:\n  escalate \"help\"\n  continued = true\n",
    )
    .expect("compiles fixture");
    let mut run = jevscript_runtime::Run::create(
        ir,
        "main",
        jevscript_runtime::RunOptions {
            record: Some(recording.clone()),
            ..jevscript_runtime::RunOptions::default()
        },
        jevscript_runtime::capability::Bindings::new(),
    )
    .expect("creates recorded run");
    let escalated = run.next().expect("reaches escalation");
    assert!(matches!(
        escalated,
        jevscript_runtime::Pause::Escalate { .. }
    ));
    run.resume(jevscript_runtime::Resume::Continue { resume: true })
        .expect("records continuation");
    let done = run.next().expect("finishes after continuation");
    assert!(matches!(done, jevscript_runtime::Pause::Done { .. }));
    drop(run);

    let replay = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["replay", recording.to_str().unwrap()])
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("replays resumed escalation");
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let expected = format!(
        "{}\n{}\n",
        serde_json::to_string(&escalated).unwrap(),
        serde_json::to_string(&done).unwrap()
    );
    assert_eq!(String::from_utf8_lossy(&replay.stdout), expected);
}

#[test]
fn run_prints_logs_and_replay_reproduces_them_from_the_recording() {
    // Spec 5.8 and 11.6: `run` prints each log on stderr as it happens;
    // `replay` reproduces the recorded lines, redacted ones as markers.
    let dir = temp_dir("logs");
    let source = dir.join("logs.jev");
    std::fs::write(
        &source,
        "program logs\n\nin message: text\nout n\n\ntask main:\n  log info \"starting\" { message }\n  n = log debug len(message)\n",
    )
    .expect("writes fixture");
    let run = |recording: &std::path::Path, redact: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jevscript"));
        command
            .args(["run", source.to_str().unwrap(), "--input"])
            .arg(r#"{"message":"hello"}"#)
            .arg("--record")
            .arg(recording)
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("JEVSCRIPT_PROFILES");
        if redact {
            command.arg("--redact");
        }
        command.output().expect("runs CLI")
    };
    let replay = |recording: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_jevscript"))
            .args(["replay", recording.to_str().unwrap()])
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("JEVSCRIPT_PROFILES")
            .output()
            .expect("replays CLI recording")
    };

    let recording = dir.join("full.jsonl");
    let live = run(&recording, false);
    let stderr = String::from_utf8_lossy(&live.stderr);
    assert!(live.status.success(), "{stderr}");
    assert!(
        stderr.contains("log info main 7:3: starting {\"message\":\"hello\"}"),
        "{stderr}"
    );
    assert!(stderr.contains("log debug main 8:7: 5"), "{stderr}");
    let replayed = replay(&recording);
    let replay_stderr = String::from_utf8_lossy(&replayed.stderr);
    assert!(replayed.status.success(), "{replay_stderr}");
    assert_eq!(
        replay_stderr
            .lines()
            .filter(|l| l.starts_with("log "))
            .collect::<Vec<_>>(),
        stderr
            .lines()
            .filter(|l| l.starts_with("log "))
            .collect::<Vec<_>>(),
        "replay reproduces the same lines"
    );

    let redacted = dir.join("redacted.jsonl");
    assert!(run(&redacted, true).status.success());
    let replayed = replay(&redacted);
    let replay_stderr = String::from_utf8_lossy(&replayed.stderr);
    assert!(replayed.status.success(), "{replay_stderr}");
    assert!(
        replay_stderr.contains("log info main 7:3: {\"redacted\":"),
        "{replay_stderr}"
    );
    assert!(!replay_stderr.contains("hello"), "{replay_stderr}");
}

#[test]
fn replay_prints_only_log_lines_it_validated() {
    // Spec 5.8, 10.4 and 11.6: a log that fails its identity check was never
    // replayed, so neither it nor anything after it is printed.
    let dir = temp_dir("tampered-log");
    let source = dir.join("logs.jev");
    std::fs::write(
        &source,
        "program p\n\ntask main:\n  log info \"first\"\n  log info \"original\"\n  log info \"third\"\n",
    )
    .expect("writes fixture");
    let recording = dir.join("run.jsonl");
    let live = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["run", source.to_str().unwrap(), "--record"])
        .arg(&recording)
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("runs CLI");
    assert!(
        live.status.success(),
        "{}",
        String::from_utf8_lossy(&live.stderr)
    );
    let text = std::fs::read_to_string(&recording).expect("reads");
    // Tamper only the log event, not the program embedded in `start`.
    let tampered: Vec<String> = text
        .lines()
        .map(|line| {
            if line.contains(r#""event":"log""#) {
                line.replace("\"original\"", "\"tampered\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    std::fs::write(&recording, tampered.join("\n") + "\n").expect("writes");

    let replay = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["replay", recording.to_str().unwrap()])
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("JEVSCRIPT_PROFILES")
        .output()
        .expect("replays CLI recording");
    let stdout = String::from_utf8_lossy(&replay.stdout);
    let stderr = String::from_utf8_lossy(&replay.stderr);
    assert_eq!(replay.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.contains("replay_diverged"), "{stdout}");
    let logs: Vec<&str> = stderr.lines().filter(|l| l.starts_with("log ")).collect();
    assert_eq!(logs, vec!["log info main 4:3: first"], "{stderr}");
}
