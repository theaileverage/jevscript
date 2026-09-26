/**
 * Orca: one terminal per agent, in the Orca worktree that contains the
 * agent's working directory (macOS, Orca 1.4 or later).
 *
 * The backend uses `terminal create --worktree path:<cwd>` with the launch
 * script as its startup command, and the runtime-issued terminal
 * handle for everything after. Reads use `--screen`, the rendered frame,
 * because the default accumulated stream stacks every TUI repaint.
 *
 * Orca has no Escape key and no paste: an interrupt that needs Escape is
 * refused, and several lines are sent as one.
 */
import { AdapterError } from '../errors.ts'
import type { BackendContext, BackendOptions } from './common.ts'
import { attempt, contextOf, oneLine, pick, probeBinary, stringAt } from './common.ts'
import { readExitFile, writeLaunchScript } from './launch.ts'
import type { BackendProbe, Key, LaunchSpec, PaneRef, PaneState, TerminalBackend } from './types.ts'

export class OrcaBackend implements TerminalBackend {
  readonly name = 'orca' as const
  readonly #ctx: BackendContext

  constructor(options: BackendOptions = {}) {
    this.#ctx = contextOf(options, 'orca')
  }

  async probe(): Promise<BackendProbe> {
    const found = await probeBinary(this.#ctx, 'orca', ['--version'])
    if (!found.installed) return { ...found, ready: false, detail: 'orca is not on PATH' }
    const status = await attempt(this.#ctx, ['status', '--json'])
    let ready = false
    try {
      const reply = JSON.parse(status?.stdout ?? '{}') as Record<string, unknown>
      const result = (reply['result'] ?? reply) as Record<string, unknown>
      const reachable = pick(result, 'runtime', 'reachable') ?? result['runtimeReachable']
      const state = pick(result, 'runtime', 'state') ?? result['runtimeState']
      ready = pick(reply, 'ok') !== false && reachable === true && state === 'ready'
    } catch {
      ready = false
    }
    return { ...found, ready, detail: ready ? null : 'the Orca app is not running (orca open starts it)' }
  }

  async create(spec: LaunchSpec): Promise<PaneRef> {
    const script = await writeLaunchScript(this.#ctx.files, this.#ctx.stateDir, spec)
    const reply = await this.#orca(
      'terminal', 'create', '--worktree', `path:${spec.cwd}`, '--title', spec.title, '--command', script.command, '--json',
    )
    const terminal = stringAt(reply, 'result', 'terminal', 'handle') ?? stringAt(reply, 'result', 'handle')
    if (!terminal) throw new AdapterError('orca terminal create returned no terminal handle', true)
    return { backend: 'orca', terminal, exit_file: script.exitFile }
  }

  async state(ref: PaneRef): Promise<PaneState> {
    const result = await attempt(this.#ctx, ['terminal', 'show', '--terminal', terminalOf(ref), '--json'])
    let exists = false
    try {
      exists = result?.code === 0 && pick(JSON.parse(result.stdout), 'ok') !== false
    } catch {
      exists = false
    }
    const exited = await readExitFile(this.#ctx.files, ref['exit_file'])
    return { exists, exited: !exists || exited !== undefined, exitCode: exited ?? null }
  }

  async capture(ref: PaneRef): Promise<string> {
    const result = await attempt(this.#ctx, ['terminal', 'read', '--terminal', terminalOf(ref), '--screen', '--json'])
    if (!result || result.code !== 0) return ''
    try {
      const reply = JSON.parse(result.stdout) as Record<string, unknown>
      const tail = pick(reply, 'result', 'terminal', 'tail') ?? pick(reply, 'result', 'tail')
      if (Array.isArray(tail)) return tail.map(String).join('\n')
      const record = (pick(reply, 'result') ?? {}) as Record<string, unknown>
      for (const key of ['text', 'output', 'content', 'preview']) {
        if (typeof record[key] === 'string') return record[key]
      }
      return ''
    } catch {
      return ''
    }
  }

  async type(ref: PaneRef, text: string): Promise<void> {
    await this.#orca('terminal', 'send', '--terminal', terminalOf(ref), '--text', oneLine(text), '--json')
  }

  async paste(ref: PaneRef, text: string): Promise<void> {
    await this.type(ref, text)
  }

  async key(ref: PaneRef, key: Key): Promise<void> {
    const terminal = terminalOf(ref)
    if (key === 'Enter') {
      await this.#orca('terminal', 'send', '--terminal', terminal, '--text', '', '--enter', '--json')
    } else if (key === 'C-c') {
      await this.#orca('terminal', 'send', '--terminal', terminal, '--interrupt', '--json')
    } else {
      throw new AdapterError(`orca cannot send ${key}`, false)
    }
  }

  async close(ref: PaneRef): Promise<void> {
    await attempt(this.#ctx, ['terminal', 'close', '--terminal', terminalOf(ref), '--json'])
  }

  async #orca(...args: string[]): Promise<Record<string, unknown>> {
    const result = await this.#ctx.exec(this.#ctx.bin, args)
    let reply: Record<string, unknown> = {}
    try {
      reply = JSON.parse(result.stdout) as Record<string, unknown>
    } catch {
      // judged by the exit status below
    }
    if (result.code !== 0 || reply['ok'] === false) {
      const detail =
        stringAt(reply, 'error', 'message') ?? stringAt(reply, 'error', 'code') ?? (result.stderr.trim() || `exit status ${result.code}`)
      throw new AdapterError(`orca ${args.slice(0, 2).join(' ')}: ${detail}`, !/not found|unknown terminal/i.test(detail))
    }
    return reply
  }
}

function terminalOf(ref: PaneRef): string {
  const terminal = ref['terminal']
  if (typeof terminal !== 'string' || terminal === '') throw new AdapterError('this handle names no Orca terminal', false)
  return terminal
}
