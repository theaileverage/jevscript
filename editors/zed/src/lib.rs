//! The Zed extension for Jevscript: registers `.jev` and starts
//! `jevscript lsp`, which supplies diagnostics, semantic highlighting, hover,
//! navigation, the outline and completion (see `docs/editors.md`).

use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

struct Jevscript;

impl zed::Extension for Jevscript {
    fn new() -> Self {
        Jevscript
    }

    /// `jevscript lsp`, from `lsp.jevscript.binary.path` in Zed's settings
    /// if set, otherwise from the worktree's `PATH`.
    fn language_server_command(
        &mut self,
        id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let configured = LspSettings::for_worktree(id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let command = configured
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which("jevscript"))
            .ok_or("jevscript is not on PATH; install it or set lsp.jevscript.binary.path")?;
        let args = configured
            .and_then(|binary| binary.arguments)
            .unwrap_or_else(|| vec!["lsp".to_string()]);
        Ok(zed::Command {
            command,
            args,
            env: worktree.shell_env(),
        })
    }

    /// The server's `initializationOptions`, from
    /// `lsp.jevscript.initialization_options` in Zed's settings.
    fn language_server_initialization_options(
        &mut self,
        id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree(id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.initialization_options))
    }
}

zed::register_extension!(Jevscript);
