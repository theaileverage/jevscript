//! Offline coding-agent Skill installation (spec section 11.6).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use sha2::{Digest, Sha256};

const SKILL: &str = include_str!("../../../skills/jevscript/SKILL.md");

/// The closed set of supported coding agents (spec section 11.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Agent {
    /// Codex discovers project Skills under `.agents/skills`.
    Codex,
    /// Claude Code discovers Skills under `.claude/skills`.
    ClaudeCode,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    fn directory(self, global: bool) -> &'static str {
        match (self, global) {
            (Self::Codex, false) => ".agents",
            (Self::Codex, true) => ".codex",
            (Self::ClaudeCode, _) => ".claude",
        }
    }
}

/// Install the embedded bytes, with a receipt that allows only same-content
/// reconciliation after an interrupted command (spec section 11.6).
pub(crate) fn install(
    agents: &[Agent],
    global: bool,
    copy: bool,
    project: Option<&Path>,
) -> Result<()> {
    let root = if global {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .context("HOME or USERPROFILE is required for --global")?;
        PathBuf::from(home)
    } else {
        project
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?)
    };
    let metadata = fs::symlink_metadata(&root)
        .with_context(|| format!("setup root does not exist: {}", root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("setup root must be a real directory: {}", root.display());
    }
    let root = root.canonicalize()?;
    let canonical = skill_path(&root, ".agents")?;
    let receipt = format!(
        "jevscript-setup-v1 sha256={}\n",
        hex::encode(Sha256::digest(SKILL.as_bytes()))
    );
    let canonical_state = install_directory(&canonical, &receipt)?;
    println!(
        "{}: {} ({})",
        if global { "global" } else { "project" },
        canonical.display(),
        canonical_state
    );

    let mut unique = Vec::new();
    for agent in agents {
        if !unique.contains(agent) {
            unique.push(*agent);
        }
    }
    for agent in unique {
        let destination = skill_path(&root, agent.directory(global))?;
        if destination == canonical {
            continue;
        }
        let state = if copy {
            install_directory(&destination, &receipt)?
        } else {
            install_link(&destination, &canonical)?
        };
        println!("{}: {} ({state})", agent.name(), destination.display());
    }
    Ok(())
}

fn skill_path(root: &Path, agent_root: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for name in [agent_root, "skills"] {
        path.push(name);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", path.display()));
            }
        }
        let kind = fs::symlink_metadata(&path)?.file_type();
        if kind.is_symlink() || !kind.is_dir() {
            bail!("Skill parent is not a real directory: {}", path.display());
        }
    }
    Ok(path.join("jevscript"))
}

fn install_directory(path: &Path, receipt: &str) -> Result<&'static str> {
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    if let Some(metadata) = existing {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("Skill destination is occupied: {}", path.display());
        }
        let recorded = path.join(".jevscript-setup-receipt");
        let skill = path.join("SKILL.md");
        if !plain_file_equals(&recorded, receipt.as_bytes())
            || !plain_file_equals(&skill, SKILL.as_bytes())
        {
            bail!(
                "Skill destination has unmanaged or changed content: {}",
                path.display()
            );
        }
        return Ok("already installed");
    }

    let parent = path.parent().context("Skill has no parent")?;
    let mut temporary = None;
    for serial in 0..1000 {
        let candidate = parent.join(format!(".jevscript-setup-{}-{serial}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                temporary = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("creating temporary Skill directory"),
        }
    }
    let temporary = temporary.context("no free temporary Skill directory name")?;
    let result = (|| -> Result<()> {
        write_new(&temporary.join("SKILL.md"), SKILL.as_bytes())?;
        write_new(
            &temporary.join(".jevscript-setup-receipt"),
            receipt.as_bytes(),
        )?;
        if fs::symlink_metadata(path).is_ok() {
            bail!(
                "Skill destination appeared during setup: {}",
                path.display()
            );
        }
        fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result?;
    Ok("installed")
}

fn plain_file_equals(path: &Path, expected: &[u8]) -> bool {
    matches!(fs::symlink_metadata(path), Ok(meta) if meta.is_file() && !meta.file_type().is_symlink())
        && fs::read(path).is_ok_and(|bytes| bytes == expected)
}

fn write_new(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn install_link(path: &Path, target: &Path) -> Result<&'static str> {
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    if let Some(metadata) = existing {
        if metadata.file_type().is_symlink() && fs::read_link(path).is_ok_and(|link| link == target)
        {
            return Ok("already installed");
        }
        bail!("Skill destination is occupied: {}", path.display());
    }
    create_directory_link(target, path).with_context(|| format!("linking {}", path.display()))?;
    Ok("installed")
}

#[cfg(unix)]
fn create_directory_link(target: &Path, path: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn create_directory_link(target: &Path, path: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, path)
}
