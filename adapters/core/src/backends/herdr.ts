/**
 * Herdr: one tab per agent, the default backend.
 *
 * For Herdr 0.7 to 0.9, a tab is created with `--no-focus` in the workspace of
 * the pane the host runs in (or in a workspace this backend creates when the
 * host is not inside Herdr), its root pane's shell runs a launch script, and
 * everything after that is `pane read`, `pane send-text`, `pane send-keys`,
 * `pane get` and `pane close`.
 *
 * Only pane and tab verbs are used. Nothing here starts, stops, restarts or
 * deletes a Herdr server or session: those are lifecycle operations that
 * belong to whoever runs the multiplexer.
 *
 * Every call carries `--session <name>` when a session is configured, placed
 * before any `--` so it stays a Herdr option, because `HERDR_SESSION` alone
 * silently reaches the wrong server when more than one is running.
 */
import { AdapterError } from '../errors.ts'
import type { BackendContext, BackendOptions } from './common.ts'
import { attempt, contextOf, failure, json, pick, probeBinary, stringAt } from './common.ts'
import { readExitFile, writeLaunchScript } from './launch.ts'
import type { BackendProbe, Key, LaunchSpec, NativeStatus, PaneRef, PaneState, TerminalBackend } from './types.ts'

const KEYS: Record<Key, string> = { Enter: 'enter', Escape: 'esc', 'C-c': 'ctrl+c', 'C-u': 'ctrl+u' }
const NATIVE: readonly NativeStatus[] = ['working', 'idle', 'done', 'blocked', 'unknown']
/** Between creating the tab and typing into its shell, and between text and Enter. */
const SHELL_SETTLE_MS = 300

/** Herdr-only options. */
export interface HerdrOptions extends BackendOptions {
  /**
   * The workspace to open tabs in. Defaults to the workspace of the pane named
   * by `$HERDR_PANE_ID` (read live, since the injected workspace id goes stale),
   * else a workspace this backend creates once and reuses.
   */
  workspace?: string
  /** Where `$HERDR_PANE_ID` comes from, for tests. Defaults to `process.env`. */
  env?: Record<string, string | undefined>
}

export class HerdrBackend implements TerminalBackend {
  readonly name = 'herdr' as const
  readonly #ctx: BackendContext
  readonly #workspace: string | undefined
  readonly #env: Record<string, string | undefined>
  #ownWorkspace: Promise<string> | undefined

  constructor(options: HerdrOptions = {}) {
    this.#ctx = contextOf(options, 'herdr')
    this.#workspace = options.workspace
    this.#env = options.env ?? process.env
  }

  async probe(): Promise<BackendProbe> {
    const found = await probeBinary(this.#ctx, 'herdr', ['--version'])
    if (!found.installed) return { ...found, ready: false, detail: 'herdr is not on PATH' }
    const status = await attempt(this.#ctx, this.#args(['status', '--json']))
    let running = false
    try {
      running = pick(JSON.parse(status?.stdout ?? '{}'), 'server', 'running') === true
    } catch {
      running = false
    }
    return { ...found, ready: running, detail: running ? null : 'no Herdr server is running' }
  }

  async create(spec: LaunchSpec): Promise<PaneRef> {
    const session = spec.session ?? this.#ctx.session
    const workspace = await this.#resolveWorkspace(spec.cwd, session)
    const created = await this.#herdr(
      ['tab', 'create', '--workspace', workspace, '--cwd', spec.cwd, '--label', spec.title, '--no-focus'],
      session,
    )
    const pane = stringAt(created, 'result', 'root_pane', 'pane_id')
    const tab = stringAt(created, 'result', 'tab', 'tab_id')
    if (!pane) throw new AdapterError('herdr tab create returned no pane id', true)
    const script = await writeLaunchScript(this.#ctx.files, this.#ctx.stateDir, spec)
    await this.#ctx.clock.sleep(SHELL_SETTLE_MS)
    // The shell may still be starting; the terminal keeps the typed line until it reads it.
    await this.#herdr(['pane', 'run', pane, script.command], session, false)
    return {
      backend: 'herdr',
      pane,
      ...(tab ? { tab } : {}),
      workspace,
      ...(session ? { herdr_session: session } : {}),
      exit_file: script.exitFile,
    }
  }

  async state(ref: PaneRef): Promise<PaneState> {
    const pane = paneOf(ref)
    const reply = await this.#read(['pane', 'get', pane], sessionOf(ref))
    const exited = await readExitFile(this.#ctx.files, ref['exit_file'])
    if (reply === null || stringAt(reply, 'error', 'code') === 'pane_not_found') {
      return { exists: false, exited: true, exitCode: exited ?? null }
    }
    if (stringAt(reply, 'result', 'pane', 'pane_id') !== pane) {
      throw new AdapterError(`herdr pane get ${pane}: ${JSON.stringify(reply).slice(0, 200)}`, true)
    }
    return { exists: true, exited: exited !== undefined, exitCode: exited ?? null }
  }

  async capture(ref: PaneRef): Promise<string> {
    const result = await this.#ctx.exec(
      this.#ctx.bin,
      this.#args(['pane', 'read', paneOf(ref), '--source', 'visible'], sessionOf(ref)),
    )
    if (result.code !== 0) {
      if (/pane_not_found/.test(result.stdout + result.stderr)) return ''
      throw failure('herdr', ['pane', 'read'], result)
    }
    return result.stdout
  }

  async type(ref: PaneRef, text: string): Promise<void> {
    await this.#herdr(['pane', 'send-text', paneOf(ref), text], sessionOf(ref), false)
  }

  /**
   * Herdr's `send-text` delivers the text as typed, newlines as line feeds; a
   * TUI in raw mode takes a line feed as a new line rather than a submit, and
   * the burst reads as a paste.
   */
  async paste(ref: PaneRef, text: string): Promise<void> {
    await this.type(ref, text)
  }

  async key(ref: PaneRef, key: Key): Promise<void> {
    await this.#herdr(['pane', 'send-keys', paneOf(ref), KEYS[key]], sessionOf(ref), false)
  }

  async close(ref: PaneRef): Promise<void> {
    await attempt(this.#ctx, this.#args(['pane', 'close', paneOf(ref)], sessionOf(ref)))
  }

  async nativeStatus(ref: PaneRef): Promise<NativeStatus | null> {
    const reply = await this.#read(['agent', 'get', paneOf(ref)], sessionOf(ref))
    const status = stringAt(reply, 'result', 'agent', 'agent_status')
    return status && (NATIVE as readonly string[]).includes(status) ? (status as NativeStatus) : null
  }

  async #resolveWorkspace(cwd: string, session: string | undefined): Promise<string> {
    if (this.#workspace) return this.#workspace
    const here = this.#env['HERDR_PANE_ID']
    if (here && this.#env['HERDR_ENV'] !== undefined) {
      const reply = await this.#read(['pane', 'get', here], session)
      const workspace = stringAt(reply, 'result', 'pane', 'workspace_id')
      if (workspace) return workspace
    }
    this.#ownWorkspace ??= this.#herdr(
      ['workspace', 'create', '--cwd', cwd, '--label', 'jevscript', '--no-focus'],
      session,
    ).then((reply) => {
      const workspace = stringAt(reply, 'result', 'workspace', 'workspace_id')
      if (!workspace) throw new AdapterError('herdr workspace create returned no workspace id', true)
      return workspace
    })
    return this.#ownWorkspace
  }

  /** The argv for a Herdr call, with `--session` before any `--`. */
  #args(args: string[], session = this.#ctx.session): string[] {
    if (!session) return args
    const split = args.indexOf('--')
    return split === -1
      ? [...args, '--session', session]
      : [...args.slice(0, split), '--session', session, ...args.slice(split)]
  }

  /** Run a Herdr call that must succeed, parsing its JSON when `parse` is set. */
  async #herdr(args: string[], session: string | undefined, parse = true): Promise<Record<string, unknown>> {
    const result = await this.#ctx.exec(this.#ctx.bin, this.#args(args, session), session ? { env: { HERDR_SESSION: session } } : {})
    const errorCode = /"code"\s*:\s*"([a-z_]+)"/.exec(result.stdout)?.[1]
    if (result.code !== 0 || (errorCode && /"error"/.test(result.stdout))) {
      const gone = errorCode === 'pane_not_found' || errorCode === 'tab_not_found' || errorCode === 'workspace_not_found'
      const detail = result.stderr.trim() || result.stdout.trim() || `exit status ${result.code}`
      throw new AdapterError(`herdr ${args.slice(0, 2).join(' ')}: ${detail}`, !gone)
    }
    return parse ? json('herdr', args, result.stdout) : {}
  }

  /** Run a read-only Herdr call and parse its JSON, error bodies included; `null` when it cannot run. */
  async #read(args: string[], session: string | undefined): Promise<Record<string, unknown> | null> {
    const result = await attempt(this.#ctx, this.#args(args, session))
    if (!result) return null
    try {
      return JSON.parse(result.stdout) as Record<string, unknown>
    } catch {
      return null
    }
  }
}

function paneOf(ref: PaneRef): string {
  const pane = ref['pane']
  if (typeof pane !== 'string' || pane === '') throw new AdapterError('this handle names no Herdr pane', false)
  return pane
}

function sessionOf(ref: PaneRef): string | undefined {
  const session = ref['herdr_session']
  return typeof session === 'string' && session !== '' ? session : undefined
}
