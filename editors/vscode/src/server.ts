// How the extension starts `jevscript lsp`. Kept free of the `vscode` module
// so the tests can run it under plain Node.

import * as path from 'node:path'

/** The settings under `jevscript.*` that shape the server. */
export interface Settings {
  /** `jevscript.server.path`: the binary, a bare name looked up on PATH. */
  serverPath: string
  /** `jevscript.modulePaths`: search roots for non-relative `use` paths. */
  modulePaths: string[]
  /** `jevscript.errorReference`: where diagnostics link, or empty. */
  errorReference: string
}

/** The process to spawn: `<binary> lsp`, speaking LSP on stdio. */
export interface ServerCommand {
  command: string
  args: string[]
}

/**
 * The command for `settings`. A relative binary path resolves against the
 * first workspace folder, so a checkout can point at its own
 * `target/debug/jevscript`.
 */
export function serverCommand(settings: Settings, workspaceFolder: string | undefined): ServerCommand {
  const configured = settings.serverPath.trim() || 'jevscript'
  const isBare = !configured.includes('/') && !configured.includes('\\')
  const command =
    isBare || path.isAbsolute(configured) || workspaceFolder === undefined
      ? configured
      : path.join(workspaceFolder, configured)
  return { command, args: ['lsp'] }
}

/** What the server reads from `initializationOptions`. */
export interface InitializationOptions {
  paths: string[]
  errorReference?: string
}

/** The server's `initializationOptions` for `settings`. */
export function initializationOptions(settings: Settings): InitializationOptions {
  const options: InitializationOptions = { paths: settings.modulePaths }
  const reference = settings.errorReference.trim()
  if (reference !== '') {
    options.errorReference = reference
  }
  return options
}
