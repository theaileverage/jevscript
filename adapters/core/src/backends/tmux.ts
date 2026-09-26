/**
 * tmux: one window per agent, in a session the adapter owns.
 *
 * The pane is created first and `remain-on-exit` set on it before the agent is
 * started into it with `respawn-pane`, so a process that exits at once still
 * leaves a dead pane whose exit status (`pane_dead_status`) `state` can read.
 * This is the original Claude Code adapter's mechanism, kept command for
 * command so its handles (`{session, pane}`, no `backend`) still reattach.
 */
import { AdapterError } from '../errors.ts'
import { shellQuote } from '../process.ts'
import type { BackendContext, BackendOptions } from './common.ts'
import { attempt, contextOf, probeBinary, run } from './common.ts'
import type { BackendProbe, Key, LaunchSpec, PaneRef, PaneState, TerminalBackend } from './types.ts'

const DEFAULT_SESSION = 'jevscript'

const KEYS: Record<Key, string> = { Enter: 'Enter', Escape: 'Escape', 'C-c': 'C-c', 'C-u': 'C-u' }

export class TmuxBackend implements TerminalBackend {
  readonly name = 'tmux' as const
  readonly #ctx: BackendContext

  constructor(options: BackendOptions = {}) {
    this.#ctx = contextOf(options, 'tmux')
  }

  async probe(): Promise<BackendProbe> {
    const found = await probeBinary(this.#ctx, 'tmux', ['-V'])
    // tmux starts its own server on demand, so an installed tmux is a ready one.
    return { ...found, ready: found.installed, detail: found.installed ? null : 'tmux is not on PATH' }
  }

  async create(spec: LaunchSpec): Promise<PaneRef> {
    const session = spec.session ?? this.#ctx.session ?? DEFAULT_SESSION
    if ((await this.#try('has-session', '-t', `=${session}`)) === null) {
      await this.#tmux('new-session', '-d', '-s', session, '-x', '200', '-y', '50', '-c', spec.cwd)
    }
    const pane = (
      await this.#tmux('new-window', '-d', '-P', '-F', '#{pane_id}', '-t', `${session}:`, '-c', spec.cwd)
    ).trim()
    await this.#tmux('set-option', '-p', '-t', pane, 'remain-on-exit', 'on')
    const env = Object.entries({ ...(process.env['PATH'] ? { PATH: process.env['PATH'] } : {}), ...spec.env }).flatMap(
      ([key, value]) => ['-e', `${key}=${value}`],
    )
    const unset = spec.unset.length > 0 ? ['env', ...spec.unset.flatMap((name) => ['-u', name])] : []
    const command = [...unset, ...spec.argv].map(shellQuote).join(' ')
    await this.#tmux('respawn-pane', '-k', '-t', pane, '-c', spec.cwd, ...env, command)
    return { session, pane }
  }

  async state(ref: PaneRef): Promise<PaneState> {
    const pane = paneOf(ref)
    const listing = await this.#try('list-panes', '-a', '-F', '#{pane_id}\t#{pane_dead}\t#{pane_dead_status}')
    if (listing !== null) {
      for (const line of listing.split('\n')) {
        const [id, dead, status] = line.split('\t')
        if (id !== pane) continue
        const code = status === undefined || status === '' ? null : Number(status)
        const exitCode = code === null || Number.isNaN(code) ? null : code
        return { exists: true, exited: dead === '1', exitCode: dead === '1' ? exitCode : null }
      }
    }
    return { exists: false, exited: true, exitCode: null }
  }

  async capture(ref: PaneRef): Promise<string> {
    return this.#tmux('capture-pane', '-p', '-J', '-t', paneOf(ref))
  }

  async type(ref: PaneRef, text: string): Promise<void> {
    await this.#tmux('send-keys', '-t', paneOf(ref), '-l', '--', text)
  }

  /** A tmux buffer and `paste-buffer -p`, which brackets the paste when the app asked for it. */
  async paste(ref: PaneRef, text: string): Promise<void> {
    const pane = paneOf(ref)
    const buffer = `jevscript-${pane}`
    await this.#tmux('set-buffer', '-b', buffer, '--', text)
    await this.#tmux('paste-buffer', '-p', '-d', '-b', buffer, '-t', pane)
  }

  async key(ref: PaneRef, key: Key): Promise<void> {
    await this.#tmux('send-keys', '-t', paneOf(ref), KEYS[key])
  }

  async close(ref: PaneRef): Promise<void> {
    await this.#try('kill-pane', '-t', paneOf(ref))
  }

  async #tmux(...args: string[]): Promise<string> {
    return run(this.#ctx, 'tmux', args)
  }

  async #try(...args: string[]): Promise<string | null> {
    const result = await attempt(this.#ctx, args)
    return result && result.code === 0 ? result.stdout : null
  }
}

function paneOf(ref: PaneRef): string {
  const pane = ref['pane']
  if (typeof pane !== 'string' || pane === '') throw new AdapterError('this handle names no tmux pane', false)
  return pane
}
