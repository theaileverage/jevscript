//! `setup` installs the bundled Skill offline with safe, repeatable paths (spec section 11.6).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const SKILL: &[u8] = include_bytes!("../../../skills/jevscript/SKILL.md");

fn scratch() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "jevscript-setup-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    if path.exists() {
        fs::remove_dir_all(&path).unwrap();
    }
    fs::create_dir_all(&path).unwrap();
    path
}

fn cli(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    let empty_path = cwd.join("empty-bin");
    fs::create_dir_all(&empty_path).unwrap();
    Command::new(env!("CARGO_BIN_EXE_jevscript"))
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", empty_path)
        .args(args)
        .output()
        .unwrap()
}

fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn skill(path: &Path) {
    assert_eq!(fs::read(path.join("SKILL.md")).unwrap(), SKILL);
}

#[test]
fn project_default_links_and_repeat_are_stable() {
    let root = scratch();
    let home = root.join("home");
    let project = root.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    let first = success(&cli(
        &project,
        &home,
        &["setup", "--agent", "codex", "--agent", "claude-code"],
    ));
    assert!(first.contains("project:"));
    let canonical = project.join(".agents/skills/jevscript");
    skill(&canonical);
    let claude = project.join(".claude/skills/jevscript");
    skill(&claude);
    assert!(
        fs::symlink_metadata(&claude)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!home.join(".agents").exists());
    let second = success(&cli(
        &project,
        &home,
        &["setup", "--agent", "codex", "--agent", "claude-code"],
    ));
    assert!(second.contains("already installed"));
    assert_eq!(
        fs::read_link(&claude).unwrap(),
        canonical.canonicalize().unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_global_copy_and_missing_agent() {
    let root = scratch();
    let home = root.join("home");
    fs::create_dir(&home).unwrap();
    success(&cli(
        &root,
        &home,
        &[
            "setup",
            "--global",
            "--copy",
            "--agent",
            "codex",
            "--agent",
            "claude-code",
        ],
    ));
    for path in [
        home.join(".agents/skills/jevscript"),
        home.join(".codex/skills/jevscript"),
        home.join(".claude/skills/jevscript"),
    ] {
        skill(&path);
        assert!(!fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }
    success(&cli(
        &root,
        &home,
        &["setup", "--global", "--copy", "--agent", "codex"],
    ));
    for args in [
        &["setup"][..],
        &["setup", "--agent", "unknown"][..],
        &["setup", "--global", "--project", ".", "--agent", "codex"][..],
    ] {
        assert!(!cli(&root, &home, args).status.success());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn divergent_content_and_mixed_agent_retry_preserve_existing_files() {
    let root = scratch();
    let home = root.join("home");
    let project = root.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    let collision = project.join(".claude/skills/jevscript");
    fs::create_dir_all(&collision).unwrap();
    fs::write(collision.join("SKILL.md"), b"other Skill").unwrap();
    let first = cli(
        &root,
        &home,
        &[
            "setup",
            "--project",
            project.to_str().unwrap(),
            "--agent",
            "codex",
            "--agent",
            "claude-code",
        ],
    );
    assert!(!first.status.success());
    assert_eq!(
        fs::read(collision.join("SKILL.md")).unwrap(),
        b"other Skill"
    );
    skill(&project.join(".agents/skills/jevscript"));
    fs::remove_dir_all(&collision).unwrap();
    let second = success(&cli(
        &root,
        &home,
        &[
            "setup",
            "--project",
            project.to_str().unwrap(),
            "--agent",
            "codex",
            "--agent",
            "claude-code",
        ],
    ));
    assert!(second.contains("already installed"));
    skill(&collision);
    fs::write(
        project.join(".agents/skills/jevscript/SKILL.md"),
        b"changed",
    )
    .unwrap();
    assert!(
        !cli(
            &root,
            &home,
            &[
                "setup",
                "--project",
                project.to_str().unwrap(),
                "--agent",
                "codex"
            ]
        )
        .status
        .success()
    );
    assert_eq!(
        fs::read(project.join(".agents/skills/jevscript/SKILL.md")).unwrap(),
        b"changed"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_parent_cannot_escape_project() {
    let root = scratch();
    let home = root.join("home");
    let project = root.join("project");
    let outside = root.join("outside");
    for path in [&home, &project, &outside] {
        fs::create_dir(path).unwrap();
    }
    std::os::unix::fs::symlink(&outside, project.join(".agents")).unwrap();
    assert!(
        !cli(
            &root,
            &home,
            &[
                "setup",
                "--project",
                project.to_str().unwrap(),
                "--agent",
                "codex"
            ]
        )
        .status
        .success()
    );
    assert!(fs::read_dir(outside).unwrap().next().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn conflicting_agent_link_is_untouched() {
    let root = scratch();
    let home = root.join("home");
    let project = root.join("project");
    let outside = root.join("outside");
    for path in [&home, &project, &outside] {
        fs::create_dir(path).unwrap();
    }
    let link = project.join(".claude/skills/jevscript");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let output = cli(
        &root,
        &home,
        &[
            "setup",
            "--project",
            project.to_str().unwrap(),
            "--agent",
            "claude-code",
        ],
    );
    assert!(!output.status.success());
    assert_eq!(fs::read_link(&link).unwrap(), outside);
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fs::remove_dir_all(root).unwrap();
}
