/**
 * Where things are and which secrets are loaded.
 *
 * The TypeSafe and Anthropic keys live in a gitignored `.env`. The toolbox
 * reads it at runtime from the checkout it runs in and, for a git worktree,
 * from the main checkout too, and never copies it anywhere. Variables already
 * in the environment win.
 */
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/** `apps/toolbox`. */
export const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
/** The repository checkout the toolbox sits in. */
export const REPO_ROOT = resolve(APP_ROOT, '../..')

export interface ToolboxPaths {
  bin: string
  home: string
  bundledProfiles: string
  examples: string
}

export function paths(env: NodeJS.ProcessEnv = process.env): ToolboxPaths {
  return {
    bin: env['JEVSCRYPT_BIN'] ?? join(REPO_ROOT, 'target/debug/jevscrypt'),
    home: env['JEVS_TOOLBOX_HOME'] ?? join(homedir(), '.jevs-toolbox'),
    bundledProfiles: join(REPO_ROOT, 'crates/jevscrypt-runtime/profiles/bundled.json'),
    examples: join(REPO_ROOT, 'examples'),
  }
}

/** The `.env` files to read, nearest first: this checkout, then the main one if this is a worktree. */
export function envFiles(root: string = REPO_ROOT): string[] {
  const files = [join(root, '.env')]
  try {
    const common = execFileSync('git', ['-C', root, 'rev-parse', '--path-format=absolute', '--git-common-dir'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
    const main = join(dirname(common), '.env')
    if (!files.includes(main)) files.push(main)
  } catch {
    // Not a git checkout; the local .env is all there is.
  }
  return files
}

/** Load `KEY=value` lines into `env` without overriding what is already set. Returns the keys loaded. */
export function loadEnvFiles(files: readonly string[], env: NodeJS.ProcessEnv = process.env): string[] {
  const loaded: string[] = []
  for (const file of files) {
    if (!existsSync(file)) continue
    for (const line of readFileSync(file, 'utf8').split('\n')) {
      const match = /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$/.exec(line)
      if (!match) continue
      const key = match[1] as string
      let value = match[2] as string
      if (/^(['"]).*\1$/.test(value)) value = value.slice(1, -1)
      if (env[key] === undefined && value !== '') {
        env[key] = value
        loaded.push(key)
      }
    }
  }
  return loaded
}
