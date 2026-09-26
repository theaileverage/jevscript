/**
 * The adapter's only ways of touching the machine: running a binary, time, and
 * a handful of file operations. Each is an interface with a real default so
 * that every adapter and backend can be exercised without a terminal, a clock
 * or a home directory (the tests in this repo never start a real agent CLI).
 */
import { execFile } from 'node:child_process'
import { constants } from 'node:fs'
import { access, chmod, mkdir, open, readdir, readFile, rename, stat, writeFile } from 'node:fs/promises'
import { delimiter, join } from 'node:path'

import { AdapterError } from './errors.ts'

/** What running one command yields. */
export interface ExecResult {
  code: number | null
  stdout: string
  stderr: string
}

/** How long a command may run and what it sees; all optional. */
export interface ExecOptions {
  /** Kill the command after this many milliseconds. */
  timeoutMs?: number
  /** Extra environment variables layered over the adapter's own. */
  env?: Record<string, string>
  cwd?: string
}

/**
 * Runs a binary with arguments and no shell in between. Stdin is always
 * detached, so a command that would prompt fails instead of hanging.
 */
export type Exec = (file: string, args: string[], options?: ExecOptions) => Promise<ExecResult>

/** The two things about time the adapter needs, so tests can fake both. */
export interface Clock {
  now(): number
  sleep(ms: number): Promise<void>
}

/** The file operations adapters and backends need. */
export interface Files {
  /** The file's text, or `undefined` when it does not exist. */
  read(path: string): Promise<string | undefined>
  /** The file's first `bytes` bytes as text, or `undefined` when it does not exist. */
  readHead(path: string, bytes: number): Promise<string | undefined>
  /** Write atomically (temporary file, then rename) with the given mode. */
  write(path: string, text: string, mode?: number): Promise<void>
  /** Create a directory and its parents, owner-only. */
  mkdirPrivate(path: string): Promise<void>
  /** The entries of a directory, or `[]` when it does not exist. */
  list(path: string): Promise<string[]>
  /** Modification time in milliseconds, or `undefined` when absent. */
  mtime(path: string): Promise<number | undefined>
}

export const defaultExec: Exec = (file, args, options = {}) =>
  new Promise((resolve, reject) => {
    const child = execFile(
      file,
      args,
      {
        maxBuffer: 16 * 1024 * 1024,
        ...(options.timeoutMs === undefined ? {} : { timeout: options.timeoutMs }),
        ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
        env: options.env ? { ...process.env, ...options.env } : process.env,
      },
      (error, stdout, stderr) => {
        if (error && (error as NodeJS.ErrnoException).code === 'ENOENT') {
          reject(new AdapterError(`\`${file}\` is not installed`, false))
          return
        }
        const code = error ? (typeof error.code === 'number' ? error.code : 1) : 0
        resolve({ code, stdout: String(stdout), stderr: String(stderr) })
      },
    )
    child.stdin?.end()
  })

export const defaultClock: Clock = {
  now: () => Date.now(),
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}

export const defaultFiles: Files = {
  async read(path) {
    try {
      return await readFile(path, 'utf8')
    } catch (error) {
      if (['ENOENT', 'ENOTDIR'].includes((error as NodeJS.ErrnoException).code ?? '')) return undefined
      throw new AdapterError(`cannot read ${path}: ${String(error)}`, true)
    }
  },
  async readHead(path, bytes) {
    let file
    try {
      file = await open(path, 'r')
    } catch (error) {
      if (['ENOENT', 'ENOTDIR'].includes((error as NodeJS.ErrnoException).code ?? '')) return undefined
      throw new AdapterError(`cannot read ${path}: ${String(error)}`, true)
    }
    try {
      const buffer = Buffer.alloc(bytes)
      const { bytesRead } = await file.read(buffer, 0, bytes, 0)
      return buffer.subarray(0, bytesRead).toString('utf8')
    } finally {
      await file.close()
    }
  },
  async write(path, text, mode = 0o600) {
    const temporary = `${path}.${process.pid}.${Date.now()}.tmp`
    await writeFile(temporary, text, { mode })
    await chmod(temporary, mode)
    await rename(temporary, path)
  },
  async mkdirPrivate(path) {
    await mkdir(path, { recursive: true, mode: 0o700 })
  },
  async list(path) {
    try {
      return await readdir(path)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return []
      throw error
    }
  },
  async mtime(path) {
    try {
      return (await stat(path)).mtimeMs
    } catch {
      return undefined
    }
  },
}

/**
 * Resolve a command name on `PATH` the way a shell would, or answer
 * `undefined`. A name containing a slash is taken as a path and only checked.
 */
export async function which(name: string, path = process.env['PATH'] ?? ''): Promise<string | undefined> {
  const candidates = name.includes('/') ? [name] : path.split(delimiter).filter(Boolean).map((dir) => join(dir, name))
  for (const candidate of candidates) {
    try {
      await access(candidate, constants.X_OK)
      return candidate
    } catch {
      // not here
    }
  }
  return undefined
}

/** Quote one word for a POSIX shell. */
export function shellQuote(word: string): string {
  return `'${word.replace(/'/g, `'\\''`)}'`
}
