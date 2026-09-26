/**
 * Zellij: one tab per agent, in a session the adapter names (Zellij 0.44 or
 * later, which added `--pane-id`, `list-panes`, `paste` and `send-keys`).
 *
 * Every call uses `zellij --session S action ...` with `--pane-id`, because a
 * new session opens a focused plugin pane and plugin ids share nothing with
 * terminal pane ids. `new-tab` always takes focus, so the previously active
 * tab is refocused straight after. `zellij action` exits 0 even when its
 * target is missing, so every lookup checks the reply's shape instead.
 */
import { AdapterError } from '../errors.ts'
import type { BackendContext, BackendOptions } from './common.ts'
import { attempt, compareVersions, contextOf, probeBinary, run } from './common.ts'
import { readExitFile, writeLaunchScript } from './launch.ts'
import type { BackendProbe, Key, LaunchSpec, PaneRef, PaneState, TerminalBackend } from './types.ts'

const MIN_VERSION = '0.44.0'
const DEFAULT_SESSION = 'jevscript'
const KEYS: Record<Key, string> = { Enter: 'Enter', Escape: 'Esc', 'C-c': 'Ctrl c', 'C-u': 'Ctrl u' }

interface ZellijPane {
  id: number
  tab_id: number
  is_plugin: boolean
}

export class ZellijBackend implements TerminalBackend {
  readonly name = 'zellij' as const
  readonly #ctx: BackendContext

  constructor(options: BackendOptions = {}) {
    this.#ctx = contextOf(options, 'zellij')
  }

  async probe(): Promise<BackendProbe> {
    const found = await probeBinary(this.#ctx, 'zellij', ['--version'])
    if (!found.installed) return { ...found, ready: false, detail: 'zellij is not on PATH' }
    if (!found.version || compareVersions(found.version, MIN_VERSION) < 0) {
      return { ...found, ready: false, detail: `zellij ${found.version ?? '(unknown)'} is older than ${MIN_VERSION}` }
    }
    return { ...found, ready: true, detail: null }
  }

  async create(spec: LaunchSpec): Promise<PaneRef> {
    const session = spec.session ?? this.#ctx.session ?? DEFAULT_SESSION
    await this.#ensureSession(session)
    const previous = (await this.#tabs(session)).find((tab) => tab['active'] === true)?.['tab_id']
    const created = (await this.#action(session, 'new-tab', '--cwd', spec.cwd, '--name', spec.title)).trim()
    if (!/^\d+$/.test(created)) throw new AdapterError(`zellij new-tab returned no tab id: ${created.slice(0, 80)}`, true)
    const tab = Number(created)
    const pane = (await this.#panes(session)).find((entry) => entry.tab_id === tab && !entry.is_plugin)
    if (typeof previous === 'number') await attempt(this.#ctx, this.#args(session, 'go-to-tab-by-id', String(previous)))
    if (!pane) throw new AdapterError(`zellij tab ${tab} has no terminal pane`, true)
    const script = await writeLaunchScript(this.#ctx.files, this.#ctx.stateDir, spec)
    await this.#action(session, 'paste', '--pane-id', String(pane.id), '--', script.command)
    await this.#action(session, 'send-keys', '--pane-id', String(pane.id), 'Enter')
    return { backend: 'zellij', zellij_session: session, pane: pane.id, tab, exit_file: script.exitFile }
  }

  async state(ref: PaneRef): Promise<PaneState> {
    const { session, pane } = targetOf(ref)
    const exists = (await this.#panes(session).catch(() => [])).some((entry) => entry.id === pane && !entry.is_plugin)
    const exited = await readExitFile(this.#ctx.files, ref['exit_file'])
    return { exists, exited: !exists || exited !== undefined, exitCode: exited ?? null }
  }

  async capture(ref: PaneRef): Promise<string> {
    const { session, pane } = targetOf(ref)
    const result = await attempt(this.#ctx, this.#args(session, 'dump-screen', '--pane-id', String(pane)))
    return result && result.code === 0 ? result.stdout : ''
  }

  async type(ref: PaneRef, text: string): Promise<void> {
    const { session, pane } = targetOf(ref)
    await this.#action(session, 'paste', '--pane-id', String(pane), '--', text)
  }

  /** `action paste` is a bracketed paste already. */
  async paste(ref: PaneRef, text: string): Promise<void> {
    await this.type(ref, text)
  }

  async key(ref: PaneRef, key: Key): Promise<void> {
    const { session, pane } = targetOf(ref)
    await this.#action(session, 'send-keys', '--pane-id', String(pane), KEYS[key])
  }

  /** Close the whole tab: `close-pane` would leave an empty tab behind. */
  async close(ref: PaneRef): Promise<void> {
    const { session, pane } = targetOf(ref)
    const found = (await this.#panes(session).catch(() => [])).find((entry) => entry.id === pane)
    if (found) await attempt(this.#ctx, this.#args(session, 'close-tab-by-id', String(found.tab_id)))
  }

  async #ensureSession(session: string): Promise<void> {
    const sessions = await attempt(this.#ctx, ['list-sessions', '--short', '--no-formatting'])
    if (sessions?.stdout.split('\n').some((line) => line.trim() === session)) return
    // Starts a detached session; it returns once the server is up.
    await attempt(this.#ctx, ['attach', '--create-background', session])
    for (let i = 0; i < 20; i++) {
      const again = await attempt(this.#ctx, ['list-sessions', '--short', '--no-formatting'])
      if (again?.stdout.split('\n').some((line) => line.trim() === session)) return
      await this.#ctx.clock.sleep(500)
    }
    throw new AdapterError(`zellij session ${session} did not start`, true)
  }

  async #tabs(session: string): Promise<Record<string, unknown>[]> {
    return parseList(await this.#action(session, 'list-tabs', '--json'))
  }

  async #panes(session: string): Promise<ZellijPane[]> {
    return parseList(await this.#action(session, 'list-panes', '--json')).filter(
      (entry): entry is Record<string, unknown> & ZellijPane => typeof entry['id'] === 'number' && typeof entry['tab_id'] === 'number',
    )
  }

  #args(session: string, ...args: string[]): string[] {
    return ['--session', session, 'action', ...args]
  }

  async #action(session: string, ...args: string[]): Promise<string> {
    return run(this.#ctx, 'zellij', this.#args(session, ...args), { ZELLIJ_SESSION_NAME: session })
  }
}

function parseList(text: string): Record<string, unknown>[] {
  try {
    const value = JSON.parse(text) as unknown
    return Array.isArray(value) ? (value as Record<string, unknown>[]) : []
  } catch {
    return []
  }
}

function targetOf(ref: PaneRef): { session: string; pane: number } {
  const { zellij_session: session, pane } = ref as { zellij_session?: unknown; pane?: unknown }
  if (typeof session !== 'string' || typeof pane !== 'number') {
    throw new AdapterError('this handle names no Zellij pane', false)
  }
  return { session, pane }
}
