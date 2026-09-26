/**
 * cmux: one workspace per agent, created unfocused (cmux 0.64 or later).
 *
 * The backend uses `new-workspace --focus false` with the launch script as its initial
 * command, the workspace found again by its unique title, and the surface
 * addressed as `--workspace W --surface S` from then on.
 *
 * cmux's socket rejects processes it did not start unless Settings >
 * Automation allows them, which `probe` reports as not ready.
 */
import { AdapterError } from '../errors.ts'
import type { BackendContext, BackendOptions } from './common.ts'
import { attempt, contextOf, json, pick, probeBinary, run, stringAt } from './common.ts'
import { readExitFile, writeLaunchScript } from './launch.ts'
import type { BackendProbe, Key, LaunchSpec, PaneRef, PaneState, TerminalBackend } from './types.ts'

const KEYS: Record<Key, string> = { Enter: 'enter', Escape: 'escape', 'C-c': 'ctrl-c', 'C-u': 'ctrl-u' }

/** cmux-only options. */
export interface CmuxOptions extends BackendOptions {
  /** The socket password, when cmux's automation mode is `password`. Defaults to `$CMUX_SOCKET_PASSWORD`. */
  password?: string
}

export class CmuxBackend implements TerminalBackend {
  readonly name = 'cmux' as const
  readonly #ctx: BackendContext
  readonly #env: Record<string, string>

  constructor(options: CmuxOptions = {}) {
    this.#ctx = contextOf(options, 'cmux')
    const password = options.password ?? process.env['CMUX_SOCKET_PASSWORD']
    this.#env = { CMUX_QUIET: '1', ...(password ? { CMUX_SOCKET_PASSWORD: password } : {}) }
  }

  async probe(): Promise<BackendProbe> {
    const found = await probeBinary(this.#ctx, 'cmux', ['version'], (text) => text.trim().split(/\s+/)[1] ?? null)
    if (!found.installed) return { ...found, ready: false, detail: 'cmux is not on PATH' }
    const ping = await attempt(this.#ctx, ['ping'], this.#env)
    const said = `${ping?.stdout ?? ''}${ping?.stderr ?? ''}`.trim()
    if (said === 'PONG') return { ...found, ready: true, detail: null }
    const detail = /only processes started inside cmux/i.test(said)
      ? 'cmux only accepts processes it started; allow automation in Settings > Automation'
      : /password|authentication/i.test(said)
        ? 'cmux wants a socket password (CMUX_SOCKET_PASSWORD)'
        : /socket not found|no live cmux socket/i.test(said)
          ? 'cmux is not running'
          : said || 'cmux did not answer ping'
    return { ...found, ready: false, detail }
  }

  async create(spec: LaunchSpec): Promise<PaneRef> {
    const script = await writeLaunchScript(this.#ctx.files, this.#ctx.stateDir, spec)
    const title = `${spec.title} ${script.exitFile.split('/').at(-2) ?? ''}`.trim()
    await this.#cmux('new-workspace', '--name', title, '--cwd', spec.cwd, '--command', script.command, '--focus', 'false')
    const listing = json('cmux', ['workspace'], await this.#cmux('workspace', 'list', '--json', '--id-format', 'uuids'))
    const workspaces = pick(listing, 'workspaces')
    const workspace = Array.isArray(workspaces)
      ? (workspaces as Record<string, unknown>[]).find((entry) => entry['title'] === title)
      : undefined
    const workspaceId = stringAt(workspace, 'id')
    if (!workspaceId) throw new AdapterError(`cmux created no workspace titled ${title}`, true)
    const surface = await this.#surfaceOf(workspaceId)
    if (!surface) throw new AdapterError(`cmux workspace ${workspaceId} has no terminal surface`, true)
    return { backend: 'cmux', workspace: workspaceId, surface, exit_file: script.exitFile }
  }

  async state(ref: PaneRef): Promise<PaneState> {
    const { workspace, surface } = targetOf(ref)
    const panes = await attempt(this.#ctx, ['list-panes', '--workspace', workspace, '--json', '--id-format', 'uuids'], this.#env)
    let exists = false
    try {
      const entries = pick(JSON.parse(panes?.stdout ?? '{}'), 'panes')
      exists =
        panes?.code === 0 &&
        Array.isArray(entries) &&
        entries.some((pane) => {
          const ids = pick(pane, 'surface_ids')
          return Array.isArray(ids) && ids.includes(surface)
        })
    } catch {
      exists = false
    }
    const exited = await readExitFile(this.#ctx.files, ref['exit_file'])
    return { exists, exited: !exists || exited !== undefined, exitCode: exited ?? null }
  }

  async capture(ref: PaneRef): Promise<string> {
    const { workspace, surface } = targetOf(ref)
    const result = await attempt(this.#ctx, ['read-screen', '--workspace', workspace, '--surface', surface, '--json'], this.#env)
    // A brand-new surface cannot be read until it has drawn something.
    if (!result || result.code !== 0) return ''
    try {
      return stringAt(JSON.parse(result.stdout), 'text') ?? ''
    } catch {
      return result.stdout
    }
  }

  /** `cmux send` turns `\n` and `\r` into Enter, so typed text is one line by construction. */
  async type(ref: PaneRef, text: string): Promise<void> {
    const { workspace, surface } = targetOf(ref)
    await this.#cmux('send', '--workspace', workspace, '--surface', surface, '--', text.replace(/[\r\n]+/g, ' '))
  }

  /** Through a named tmux-compatible buffer, so newlines arrive as a paste instead of Enters. */
  async paste(ref: PaneRef, text: string): Promise<void> {
    const { workspace, surface } = targetOf(ref)
    const name = `jevscript-${surface.slice(0, 8)}`
    await this.#cmux('set-buffer', '--name', name, '--', text)
    await this.#cmux('paste-buffer', '--name', name, '--workspace', workspace, '--surface', surface)
  }

  async key(ref: PaneRef, key: Key): Promise<void> {
    const { workspace, surface } = targetOf(ref)
    await this.#cmux('send-key', '--workspace', workspace, '--surface', surface, KEYS[key])
  }

  async close(ref: PaneRef): Promise<void> {
    await attempt(this.#ctx, ['close-workspace', '--workspace', targetOf(ref).workspace], this.#env)
  }

  async #surfaceOf(workspace: string): Promise<string | undefined> {
    const panes = json('cmux', ['list-panes'], await this.#cmux('list-panes', '--workspace', workspace, '--json', '--id-format', 'uuids'))
    const first = (pick(panes, 'panes') as unknown[] | undefined)?.[0]
    const ids = pick(first, 'surface_ids')
    return stringAt(first, 'selected_surface_id') ?? (Array.isArray(ids) && typeof ids[0] === 'string' ? ids[0] : undefined)
  }

  async #cmux(...args: string[]): Promise<string> {
    return run(this.#ctx, 'cmux', args, this.#env)
  }
}

function targetOf(ref: PaneRef): { workspace: string; surface: string } {
  const { workspace, surface } = ref as { workspace?: unknown; surface?: unknown }
  if (typeof workspace !== 'string' || typeof surface !== 'string') {
    throw new AdapterError('this handle names no cmux surface', false)
  }
  return { workspace, surface }
}
