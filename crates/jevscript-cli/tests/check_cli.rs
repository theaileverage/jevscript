//! `jevscript check` end to end: actionable diagnostics (spec sections 12 and 15.8).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("check_cli")
        .join(name);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write");
    path
}

fn check(file: &Path) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("check")
        .arg(file)
        .output()
        .expect("runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn check_with_tools(file: &Path, manifest: &Path) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .arg("check")
        .arg(file)
        .arg("--tools")
        .arg(manifest)
        .output()
        .expect("runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_library_diagnostic_is_printed_against_the_library() {
    let dir = scratch("library");
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib with claude\n\nneeds claude: agent\n\ntask main:\n  x = lib.f(1)\n",
    );
    write(
        &dir,
        "lib.jev",
        "program lib\n\nneeds claude: agent\n\ntask f(x):\n  claude.fly \"x\"\n  return x\n",
    );
    let (code, stderr) = check(&root);
    assert_eq!(code, 1, "{stderr}");
    let line = stderr
        .lines()
        .find(|l| l.contains("verb_unknown"))
        .unwrap_or_else(|| panic!("no verb_unknown line in {stderr}"));
    assert!(
        line.starts_with(&format!(
            "{}:6:9: verb_unknown:",
            dir.join("./lib.jev").display()
        )),
        "{line}"
    );
    assert!(stderr.contains("  6 |   claude.fly \"x\""), "{stderr}");
    assert!(
        stderr.contains("cause: The verb is not defined"),
        "{stderr}"
    );
    assert!(
        stderr.contains("docs/error-reference.md#verb_unknown"),
        "{stderr}"
    );
}

#[test]
fn a_nested_library_link_error_is_printed_against_the_importing_library() {
    // Review finding: `a.jev` fails to map one of `b.jev`'s needs; the error
    // belongs to `a.jev`, not the root.
    let dir = scratch("nested");
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./a.jev\" as a with tree\n\nneeds tree: tool\n\ntask main:\n  x = 1\n",
    );
    write(
        &dir,
        "a.jev",
        "program a\n\nuse \"./b.jev\" as b\n\nneeds tree: tool\n\ndef fa(x):\n  return x\n",
    );
    write(
        &dir,
        "b.jev",
        "program b\n\nneeds tree: tool\n\ndef fb(x):\n  return x\n",
    );
    let (code, stderr) = check(&root);
    assert_eq!(code, 1, "{stderr}");
    let line = stderr.lines().next().unwrap_or_default();
    assert!(
        line.starts_with(&format!(
            "{}:3:0: use_needs_unmapped:",
            dir.join("./a.jev").display()
        )),
        "{line}"
    );
    assert!(!line.contains("root.jev"), "{line}");
    assert!(stderr.contains("  3 | use \"./b.jev\" as b"), "{stderr}");
    assert!(
        stderr.contains("docs/error-reference.md#use_needs_unmapped"),
        "{stderr}"
    );
}

#[test]
fn an_unreadable_root_is_printed_against_its_own_path() {
    let missing = scratch("missing").join("missing.jev");
    let (code, stderr) = check(&missing);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.starts_with(&format!("{}:0:0: use_not_found:", missing.display())),
        "{stderr}"
    );
    assert!(stderr.contains("source location unavailable"), "{stderr}");
    assert!(stderr.contains("help: Correct the path"), "{stderr}");
}

#[test]
fn multiple_checker_errors_are_each_marked_and_linked() {
    let dir = scratch("parser-multiple");
    let source = write(
        &dir,
        "broken.jev",
        "program demo\n\ntask main:\n  x = missing\n  y = absent\n",
    );
    let (code, stderr) = check(&source);
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(
        stderr
            .matches("docs/error-reference.md#unassigned_read")
            .count(),
        2,
        "{stderr}"
    );
    assert!(stderr.contains("  4 |   x = missing"), "{stderr}");
    assert!(stderr.contains("  5 |   y = absent"), "{stderr}");
}

#[test]
fn parser_error_marks_the_offending_token() {
    let dir = scratch("parser");
    let source = write(&dir, "broken.jev", "program demo\n\ntask main:\n  x = ]\n");
    let (code, stderr) = check(&source);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains(":4:6: syntax:"), "{stderr}");
    assert!(stderr.contains("  4 |   x = ]\n    |       ^"), "{stderr}");
    assert!(
        stderr.contains("docs/error-reference.md#syntax"),
        "{stderr}"
    );
}

#[test]
fn a_duplicate_shows_the_first_declaration() {
    let dir = scratch("related");
    let source = write(
        &dir,
        "duplicate.jev",
        "program demo\n\ndef f():\n  return 1\n\ndef f():\n  return 2\n\ntask main:\n  x = f()\n",
    );
    let (code, stderr) = check(&source);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains(":6:4: duplicate_name:"), "{stderr}");
    assert!(stderr.contains("related:"), "{stderr}");
    assert!(
        stderr.contains(":3:4: `f` was first declared here"),
        "{stderr}"
    );
}

#[test]
fn stdin_and_unicode_columns_mark_characters() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .args(["check", "-"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all("program demo\n\ntask main:\n  x = \"é\" + missing\n".as_bytes())
        .expect("writes");
    let output = child.wait_with_output().expect("finishes");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("<stdin>:4:12: unassigned_read:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("  4 |   x = \"é\" + missing\n    |             ^^^^^^^"),
        "{stderr}"
    );
    assert!(stderr.contains("help: Initialize it before"), "{stderr}");
}

#[test]
fn wide_unicode_before_a_span_uses_terminal_cell_width() {
    let dir = scratch("wide-unicode");
    let source = write(
        &dir,
        "wide.jev",
        "program demo\n\ntask main:\n  x = \"界\" + missing\n",
    );
    let (code, stderr) = check(&source);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains(":4:12: unassigned_read:"), "{stderr}");
    assert!(
        stderr.contains("  4 |   x = \"界\" + missing\n    |              ^^^^^^^"),
        "{stderr}"
    );
}

#[test]
fn a_tool_manifest_requires_signature_records() {
    let dir = scratch("manifest-shape");
    let source = write(
        &dir,
        "program.jev",
        "program t\n\nneeds tree: tool\n\ntask main:\n  tree.open\n",
    );
    let manifest = write(&dir, "manifest.json", r#"{"verbs":{"open":42}}"#);
    let (code, stderr) = check_with_tools(&source, &manifest);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("invalid type"), "{stderr}");
}
