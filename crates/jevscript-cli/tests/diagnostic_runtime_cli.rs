//! CLI runtime diagnostics retain pause JSON and exit status (spec sections 10.2 and 15.8).

use std::process::Command;

#[test]
fn runtime_error_pause_has_source_cause_repair_and_link() {
    let path = std::env::temp_dir().join(format!(
        "jevscript-runtime-diagnostic-{}.jev",
        std::process::id()
    ));
    std::fs::write(&path, "program demo\n\ntask main:\n  x = 1 + \"a\"\n").expect("source");
    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("runs");
    std::fs::remove_file(&path).expect("cleanup");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains(":4:6: type_error:"), "{stderr}");
    assert!(
        stderr.contains("  4 |   x = 1 + \"a\"\n    |       ^^^^^^^"),
        "{stderr}"
    );
    assert!(
        stderr.contains("cause: A runtime value has the wrong type"),
        "{stderr}"
    );
    assert!(
        stderr.contains("help: Inspect the marked expression"),
        "{stderr}"
    );
    assert!(
        stderr.contains("docs/error-reference.md#type_error"),
        "{stderr}"
    );
    let pause: serde_json::Value = serde_json::from_slice(&output.stdout).expect("pause JSON");
    assert_eq!(pause["kind"], "error");
    assert_eq!(pause["code"], "type_error");
}

#[test]
fn startup_error_marks_the_capability_declaration() {
    let path = std::env::temp_dir().join(format!(
        "jevscript-missing-binding-{}.jev",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "program demo\n\nneeds tree: tool\n\ntask main:\n  tree.touch\n",
    )
    .expect("source");
    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("runs");
    std::fs::remove_file(&path).expect("cleanup");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("binding_missing:"), "{stderr}");
    assert!(stderr.contains("  3 | needs tree: tool"), "{stderr}");
    assert!(
        stderr.contains("docs/error-reference.md#binding_missing"),
        "{stderr}"
    );
}

#[test]
fn replay_without_source_uses_an_honest_excerpt_fallback() {
    let dir = std::env::temp_dir().join(format!(
        "jevscript-replay-diagnostic-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("directory");
    let source = dir.join("source.jev");
    let recording = dir.join("run.jsonl");
    std::fs::write(&source, "program demo\n\ntask main:\n  x = 1 + \"a\"\n").expect("source");
    let live = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("run")
        .arg(&source)
        .arg("--record")
        .arg(&recording)
        .output()
        .expect("records");
    assert_eq!(
        live.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&live.stderr)
    );
    std::fs::remove_file(&source).expect("remove source");
    let replay = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("replay")
        .arg(&recording)
        .output()
        .expect("replays");
    std::fs::remove_file(&recording).expect("remove recording");
    std::fs::remove_dir(&dir).expect("remove directory");
    let stderr = String::from_utf8_lossy(&replay.stderr);
    assert_eq!(replay.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("source excerpt unavailable"), "{stderr}");
    assert!(
        stderr.contains("docs/error-reference.md#type_error"),
        "{stderr}"
    );
}

#[test]
fn profile_failure_without_a_source_span_has_a_truthful_fallback() {
    let dir = std::env::temp_dir().join(format!(
        "jevscript-profile-diagnostic-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("directory");
    let source = dir.join("source.jev");
    let absent_profile = dir.join("absent.json");
    std::fs::write(&source, "program demo\n\ntask main:\n  x = 1\n").expect("source");
    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("run")
        .arg(&source)
        .arg("--profiles")
        .arg(&absent_profile)
        .output()
        .expect("runs");
    std::fs::remove_file(&source).expect("remove source");
    std::fs::remove_dir(&dir).expect("remove directory");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("profile_missing:"), "{stderr}");
    assert!(stderr.contains("source location unavailable"), "{stderr}");
    assert!(
        stderr.contains("docs/error-reference.md#profile_missing"),
        "{stderr}"
    );
}

#[test]
fn imported_runtime_error_never_claims_the_root_source() {
    // Spec 12: a pause has a span but no file; a linked unit must not be
    // attributed to the root just because its line number happens to fit.
    let dir = std::env::temp_dir().join(format!(
        "jevscript-imported-runtime-diagnostic-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("directory");
    let root = dir.join("root.jev");
    let library = dir.join("lib.jev");
    let recording = dir.join("run.jsonl");
    std::fs::write(
        &root,
        "program demo\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = lib.f()\n",
    )
    .expect("root");
    std::fs::write(&library, "program lib\n\ndef f():\n  return 1 + \"a\"\n").expect("library");
    let live = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("run")
        .arg(&root)
        .arg("--record")
        .arg(&recording)
        .output()
        .expect("runs");
    let replay = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("replay")
        .arg(&recording)
        .output()
        .expect("replays");
    for output in [&live, &replay] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("<source unavailable>:4:"), "{stderr}");
        assert!(
            stderr.contains("source file unavailable for linked program"),
            "{stderr}"
        );
        assert!(
            !stderr.contains(&format!("{}:4:", root.display())),
            "{stderr}"
        );
        assert!(
            stderr.contains("docs/error-reference.md#type_error"),
            "{stderr}"
        );
        let pause: serde_json::Value = serde_json::from_slice(&output.stdout).expect("pause JSON");
        assert_eq!(pause["code"], "type_error");
    }
    std::fs::remove_file(&recording).expect("remove recording");
    std::fs::remove_file(&library).expect("remove library");
    std::fs::remove_file(&root).expect("remove root");
    std::fs::remove_dir(&dir).expect("remove directory");
}
