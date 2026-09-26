/**
 * What every backend is built from: how it runs its CLI, and the options a
 * host may give any of them.
 */
import { AdapterError } from '../errors.ts'
import type { Clock, Exec, ExecResult, Files } from '../process.ts'
import { defaultClock, defaultExec, defaultFiles, which } from '../process.ts'
import { defaultStateDir } from './launch.ts'
import type { BackendName, BackendProbe } from './types.ts'

/** Options every backend accepts. All optional; the defaults reach the real machine. */
export interface BackendOptions {
  /** The multiplexer binary. Defaults to its usual name on `PATH`. */
  bin?: string
  /**
   * The backend session or namespace: the tmux session panes are created in
   * (default `jevscript`), the Zellij session (default `jevscript`), or the
   * Herdr session (default: whichever server the environment reaches).
   */
  session?: string
  /** Where launch scripts and exit files go. Defaults to a per-user directory in the temp dir. */
  stateDir?: string
  exec?: Exec
  clock?: Clock
  files?: Files
}

/** The resolved form of {@link BackendOptions}. */
export interface BackendContext {
  bin: string
  session: string | undefined
  stateDir: string
  exec: Exec
  clock: Clock
  files: Files
}

export function contextOf(options: BackendOptions, defaultBin: string): BackendContext {
  return {
    bin: options.bin ?? defaultBin,
    session: options.session,
    stateDir: options.stateDir ?? defaultStateDir(),
    exec: options.exec ?? defaultExec,
    clock: options.clock ?? defaultClock,
    files: options.files ?? defaultFiles,
  }
}

/** Run a backend command and insist it succeeds. */
export async function run(
  ctx: BackendContext,
  name: BackendName,
  args: string[],
  env?: Record<string, string>,
): Promise<string> {
  const result = await ctx.exec(ctx.bin, args, env ? { env } : {})
  if (result.code !== 0) throw failure(name, args, result)
  return result.stdout
}

/** Run a backend command and answer `null` instead of throwing on failure. */
export async function attempt(
  ctx: BackendContext,
  args: string[],
  env?: Record<string, string>,
): Promise<ExecResult | null> {
  try {
    const result = await ctx.exec(ctx.bin, args, env ? { env } : {})
    return result
  } catch {
    return null
  }
}

/** The error a failed backend command becomes: retryable unless its target is gone. */
export function failure(name: BackendName, args: string[], result: ExecResult): AdapterError {
  const detail = result.stderr.trim() || result.stdout.trim() || `exit status ${result.code}`
  const gone = /can't find|no server running|not found|no such|does not exist|unknown (pane|terminal|surface)/i.test(detail)
  return new AdapterError(`${name} ${args[0] ?? ''}: ${detail}`, !gone)
}

/** Parse a JSON reply, or fail with an error naming the command. */
export function json(name: BackendName, args: string[], text: string): Record<string, unknown> {
  try {
    const value = JSON.parse(text) as unknown
    if (value && typeof value === 'object') return value as Record<string, unknown>
  } catch {
    // fall through
  }
  throw new AdapterError(`${name} ${args[0] ?? ''} returned something that is not JSON: ${text.slice(0, 200)}`, true)
}

/** Walk a dotted path through parsed JSON. */
export function pick(value: unknown, ...path: string[]): unknown {
  let current = value
  for (const key of path) {
    if (!current || typeof current !== 'object') return undefined
    current = (current as Record<string, unknown>)[key]
  }
  return current
}

export function stringAt(value: unknown, ...path: string[]): string | undefined {
  const found = pick(value, ...path)
  return typeof found === 'string' && found !== '' ? found : undefined
}

/** The binary and first line of `<bin> <args>`: the read-only half of every probe. */
export async function probeBinary(
  ctx: BackendContext,
  name: BackendName,
  versionArgs: string[],
  parse: (text: string) => string | null = firstVersion,
): Promise<Pick<BackendProbe, 'name' | 'installed' | 'path' | 'version'>> {
  const path = await which(ctx.bin)
  if (!path) return { name, installed: false, path: null, version: null }
  const result = await attempt(ctx, versionArgs)
  const version = result && result.code === 0 ? parse(result.stdout || result.stderr) : null
  return { name, installed: true, path, version }
}

/** The first thing that looks like a version number in `text`. */
export function firstVersion(text: string): string | null {
  const match = /\d+\.\d+(?:\.\d+)?(?:[-.+][0-9A-Za-z.-]+)?/.exec(text)
  return match ? match[0] : null
}

/** Compare dotted versions numerically: negative, zero or positive. */
export function compareVersions(a: string, b: string): number {
  const left = a.split(/[.-]/).map((part) => Number.parseInt(part, 10) || 0)
  const right = b.split(/[.-]/).map((part) => Number.parseInt(part, 10) || 0)
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0)
    if (diff !== 0) return diff
  }
  return 0
}

/** Replace newlines with spaces, for backends that have no paste and would submit at each one. */
export function oneLine(text: string): string {
  return text.replace(/\s*\n\s*/g, ' ').trim()
}
