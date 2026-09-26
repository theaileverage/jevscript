/**
 * Fakes for testing adapters without a terminal, a clock or a home directory:
 * a scriptable {@link FakeBackend}, a {@link FakeClock} that only moves when
 * something sleeps, and in-memory {@link FakeFiles}. Every adapter package in
 * this suite tests against them, and a custom adapter can too
 * (`import { FakeBackend } from '@jevscript/adapter-core/testing'`).
 */
import type { BackendProbe, Key, LaunchSpec, NativeStatus, PaneRef, PaneState, TerminalBackend } from './backends/types.ts'
import type { Clock, Files } from './process.ts'

/** One fake pane. */
export interface FakePane {
  spec: LaunchSpec
  /** What `capture` shows; a function for screens that change per read. */
  screen: string | (() => string)
  exited: boolean
  exitCode: number | null
  native: NativeStatus | null
  /** Everything typed, pasted or pressed, in order. */
  input: Array<{ type: 'type' | 'paste'; text: string } | { type: 'key'; key: Key }>
}

/** A backend whose panes are records in memory. */
export class FakeBackend implements TerminalBackend {
  readonly name
  readonly panes = new Map<string, FakePane>()
  /** Called on every key, so a test can make the screen react (a dialog answered, a turn started). */
  onKey: ((pane: FakePane, key: Key) => void) | null = null
  /** Called on every type or paste. */
  onText: ((pane: FakePane, text: string) => void) | null = null
  #next = 1

  constructor(name: TerminalBackend['name'] = 'herdr') {
    this.name = name
  }

  async probe(): Promise<BackendProbe> {
    return { name: this.name, installed: true, path: `/fake/${this.name}`, version: '0.0.0', ready: true, detail: null }
  }

  async create(spec: LaunchSpec): Promise<PaneRef> {
    const id = `fake:${this.#next++}`
    this.panes.set(id, { spec, screen: '', exited: false, exitCode: null, native: null, input: [] })
    return { backend: this.name, pane: id }
  }

  /** The pane a handle names; throws when there is none. */
  pane(ref: PaneRef | string): FakePane {
    const id = typeof ref === 'string' ? ref : String(ref['pane'])
    const pane = this.panes.get(id)
    if (!pane) throw new Error(`no fake pane ${id}`)
    return pane
  }

  async state(ref: PaneRef): Promise<PaneState> {
    const pane = this.panes.get(String(ref['pane']))
    if (!pane) return { exists: false, exited: true, exitCode: null }
    return { exists: true, exited: pane.exited, exitCode: pane.exited ? pane.exitCode : null }
  }

  async capture(ref: PaneRef): Promise<string> {
    const pane = this.panes.get(String(ref['pane']))
    if (!pane) return ''
    return typeof pane.screen === 'function' ? pane.screen() : pane.screen
  }

  async type(ref: PaneRef, text: string): Promise<void> {
    const pane = this.pane(ref)
    pane.input.push({ type: 'type', text })
    this.onText?.(pane, text)
  }

  async paste(ref: PaneRef, text: string): Promise<void> {
    const pane = this.pane(ref)
    pane.input.push({ type: 'paste', text })
    this.onText?.(pane, text)
  }

  async key(ref: PaneRef, key: Key): Promise<void> {
    const pane = this.pane(ref)
    pane.input.push({ type: 'key', key })
    this.onKey?.(pane, key)
  }

  async close(ref: PaneRef): Promise<void> {
    this.panes.delete(String(ref['pane']))
  }

  async nativeStatus(ref: PaneRef): Promise<NativeStatus | null> {
    return this.panes.get(String(ref['pane']))?.native ?? null
  }
}

/** A clock that only moves when something sleeps. */
export class FakeClock implements Clock {
  time: number
  slept: number[] = []
  /** Called after every sleep, so a test can change the world as time passes. */
  onSleep: ((now: number) => void) | null = null

  constructor(time = 1_000_000) {
    this.time = time
  }

  now(): number {
    return this.time
  }

  async sleep(ms: number): Promise<void> {
    this.slept.push(ms)
    this.time += ms
    this.onSleep?.(this.time)
  }
}

/** Files in a map. Directories are implied by the paths of the files under them. */
export class FakeFiles implements Files {
  readonly files = new Map<string, string>()
  readonly mtimes = new Map<string, number>()
  readonly modes = new Map<string, number>()

  async read(path: string): Promise<string | undefined> {
    return this.files.get(path)
  }

  async readHead(path: string, bytes: number): Promise<string | undefined> {
    return this.files.get(path)?.slice(0, bytes)
  }

  async write(path: string, text: string, mode = 0o600): Promise<void> {
    this.files.set(path, text)
    this.modes.set(path, mode)
  }

  async mkdirPrivate(): Promise<void> {}

  async list(path: string): Promise<string[]> {
    const prefix = path.endsWith('/') ? path : `${path}/`
    const names = new Set<string>()
    for (const file of this.files.keys()) {
      if (file.startsWith(prefix)) names.add(file.slice(prefix.length).split('/')[0] as string)
    }
    return [...names].sort()
  }

  async mtime(path: string): Promise<number | undefined> {
    return this.files.has(path) ? (this.mtimes.get(path) ?? 0) : undefined
  }
}
