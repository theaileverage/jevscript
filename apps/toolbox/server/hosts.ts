/** Explicit host-file execution over the toolbox's SDK run driver, spec section 11.2. */
import { execFile, spawn, type ChildProcess } from 'node:child_process'
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import type { Readable } from 'node:stream'

import type { Idea, PairCheck, RunSummary, ServerMessage } from '../shared/protocol.ts'
import type { Jevscript } from './jevscript.ts'
import type { RunManager } from './runs.ts'

/** Private stdio bridge: host code chooses inputs/task, the trusted host owns bindings and recordings. */
const BRIDGE = `import { writeSync } from 'node:fs'
import { createInterface } from 'node:readline'

export interface Options { inputs?: Record<string, unknown>; task?: string }
export interface Result { runId: string; recording: string; outcome: string | null; outputs: Record<string, unknown>; verified: boolean }
let called = false
export async function runIdea(options: Options = {}): Promise<Result> {
  if (called) throw new Error('Call runIdea once per host launch.')
  called = true
  const lines = createInterface({ input: process.stdin })
  writeSync(3, JSON.stringify(options) + '\\n')
  for await (const line of lines) {
    lines.close()
    const reply = JSON.parse(line)
    if (reply.error) throw new Error(reply.error)
    return reply.result
  }
  throw new Error('The toolbox host connection closed.')
}
`

interface HostProcess {
  child: ChildProcess
  runId: string | null
  closed: Promise<void>
}

export class HostRunner {
  readonly #jev: Jevscript
  readonly #runs: RunManager
  readonly #home: string
  readonly #send: (message: ServerMessage) => void
  readonly #live = new Map<string, HostProcess>()
  readonly #starting = new Set<string>()
  #closed = false

  constructor(jev: Jevscript, runs: RunManager, home: string, send: (message: ServerMessage) => void) {
    this.#jev = jev
    this.#runs = runs
    this.#home = join(home, 'host-work')
    this.#send = send
  }

  async #files(idea: Idea, hostFileId: string): Promise<{ dir: string; host: string }> {
    const host = idea.workspace.files.find(file => file.id === hostFileId && file.kind === 'host')
    if (!host || !/\.(ts|mts|js|mjs)$/.test(host.name)) throw new Error('Select a JavaScript or TypeScript host file (.js, .mjs, .ts or .mts).')
    const names = new Set<string>()
    let bytes = 0
    for (const file of idea.workspace.files) {
      if (!file.name.split('/').every(part => /^[A-Za-z0-9_.-]+$/.test(part) && !['.', '..'].includes(part)) || file.name === '.toolbox' || file.name.startsWith('.toolbox/') || file.name === 'package.json') throw new Error('Workspace files need safe relative names outside the reserved .toolbox folder and package.json.')
      if (names.has(file.name)) throw new Error('Workspace file names must be unique.')
      names.add(file.name)
      bytes += Buffer.byteLength(file.source)
    }
    if (bytes > 1024 * 1024) throw new Error('The host workspace exceeds the 1 MiB source limit.')
    await mkdir(this.#home, { recursive: true })
    const dir = await mkdtemp(join(this.#home, 'pair-'))
    try {
      for (const file of idea.workspace.files) {
        const path = join(dir, file.name)
        await mkdir(dirname(path), { recursive: true })
        await writeFile(path, file.source, { mode: 0o600 })
      }
      await mkdir(join(dir, '.toolbox'))
      await writeFile(join(dir, '.toolbox/host.ts'), BRIDGE, { mode: 0o600 })
      await writeFile(join(dir, 'package.json'), '{"type":"module"}', { mode: 0o600 })
      return { dir, host: join(dir, host.name) }
    } catch (error) { await rm(dir, { recursive: true, force: true }); throw error }
  }

  async #syntax(host: string): Promise<boolean> {
    return new Promise(resolve => {
      execFile(process.execPath, ['--check', host], { timeout: 5000, maxBuffer: 64 * 1024, env: {} }, error => resolve(!error))
    })
  }

  async check(idea: Idea, hostFileId: string): Promise<PairCheck> {
    const { dir, host } = await this.#files(idea, hostFileId)
    try {
      const [jev, ok] = await Promise.all([
        this.#jev.compile(idea.fileName, idea.source, idea.workspace.files), this.#syntax(host),
      ])
      return { jev, host: { ok, message: ok ? 'Node syntax check passed. Host imports and behavior are checked by Run pair.' : 'Node could not parse the selected host file. Fix its JavaScript/TypeScript syntax.' } }
    } finally { await rm(dir, { recursive: true, force: true }) }
  }

  async start(idea: Idea, hostFileId: string): Promise<{ runId: string; recording: string }> {
    if (this.#closed) throw new Error('The toolbox host runner is closing.')
    if (this.#starting.has(idea.id) || this.#live.has(idea.id)) throw new Error('This idea already has a host running.')
    this.#starting.add(idea.id)
    let dir: string | null = null
    try {
      const files = await this.#files(idea, hostFileId)
      dir = files.dir
      if (!await this.#syntax(files.host)) throw new Error('Node could not parse the selected host file. Fix its JavaScript/TypeScript syntax.')
      const inputs: unknown = JSON.parse(idea.inputs || '{}')
      if (!inputs || typeof inputs !== 'object' || Array.isArray(inputs)) throw new Error('The idea inputs must be a JSON object.')
      const sandbox = await realpath(dir)
      if (this.#closed) throw new Error('The toolbox host runner is closing.')
      const child = spawn(process.execPath, ['--permission', `--allow-fs-read=${sandbox}`, `--allow-fs-write=${sandbox}`, await realpath(files.host)], {
        cwd: sandbox, detached: true, env: { PATH: process.env['PATH'] }, stdio: ['pipe', 'pipe', 'pipe', 'pipe'],
      })
      return await new Promise((resolve, reject) => {
        let accepted = false
        let called = false
        let stopping = false
        let failure = ''
        let outputBytes = 0
        let control = ''
        let finishClosed: () => void
        const live: HostProcess = { child, runId: null, closed: new Promise<void>(done => { finishClosed = done }) }
        this.#live.set(idea.id, live)
        const stop = (message: string) => {
          if (stopping) return
          stopping = true
          failure = message
          if (child.pid) { try { process.kill(-child.pid, 'SIGKILL') } catch { child.kill('SIGKILL') } }
          if (live.runId) void this.#runs.abort(live.runId).catch(() => undefined)
        }
        const startup = setTimeout(() => stop('The host did not call runIdea() within 5 seconds.'), 5000)
        const timeout = setTimeout(() => stop('The host exceeded the 10 minute runtime limit.'), 10 * 60_000)
        const output = (chunk: Buffer) => {
          outputBytes += chunk.length
          if (outputBytes > 1024 * 1024) stop('The host exceeded the 1 MiB output limit.')
        }
        child.stdout?.on('data', output)
        child.stderr?.on('data', output)
        child.stdin?.on('error', () => undefined)
        const port = child.stdio[3] as Readable
        port.setEncoding('utf8').on('data', (chunk: string) => {
          control += chunk
          if (Buffer.byteLength(control) > 64 * 1024) { stop('The host request exceeds the 64 KiB limit.'); return }
          const newline = control.indexOf('\n')
          if (newline < 0) return
          if (called) { stop('Call runIdea once per host launch.'); return }
          called = true
          clearTimeout(startup)
          const line = control.slice(0, newline)
          control = control.slice(newline + 1)
          void (async () => {
            try {
              const options = JSON.parse(line) as { inputs?: unknown; task?: unknown }
              if (!options || typeof options !== 'object' || Array.isArray(options)) throw new Error('Host options must be an object.')
              if (options.inputs !== undefined && (!options.inputs || typeof options.inputs !== 'object' || Array.isArray(options.inputs))) throw new Error('Host inputs must be a JSON object.')
              if (options.task !== undefined && (typeof options.task !== 'string' || !/^[A-Za-z_][A-Za-z_0-9.]*$/.test(options.task))) throw new Error('Host task must be a Jev task name.')
              let ended!: (summary: RunSummary) => void
              const completed = new Promise<RunSummary>(done => { ended = done })
              const started = await this.#runs.start({
                ideaId: idea.id, title: idea.title, fileName: idea.fileName, source: idea.source,
                files: idea.workspace.files, inputs: (options.inputs ?? inputs) as Record<string, unknown>,
                bindings: idea.bindings, model: idea.model, sample: idea.sample,
                ...(typeof options.task === 'string' ? { task: options.task } : {}),
              }, ended)
              live.runId = started.runId
              if (stopping) { await this.#runs.abort(started.runId).catch(() => undefined); return }
              accepted = true
              resolve(started)
              const result = await completed
              if (!stopping) child.stdin?.end(JSON.stringify({ result }) + '\n')
            } catch (error) {
              stop(error instanceof Error ? error.message : 'The host could not start the Jev program.')
            }
          })()
        })
        child.on('error', () => { failure = 'Node could not start the host file.' })
        child.on('close', code => {
          clearTimeout(startup)
          clearTimeout(timeout)
          this.#live.delete(idea.id)
          const ok = !failure && called && accepted && code === 0
          const message = ok ? 'Host finished. The Jev result and recording are saved with this idea.' : failure || 'The host exited unsuccessfully. Check its imports and runIdea() call.'
          if (!accepted) reject(new Error(message))
          else {
            if (!ok && live.runId) void this.#runs.abort(live.runId).catch(() => undefined)
            this.#send({ type: 'host.ended', ideaId: idea.id, ok, message })
          }
          void rm(files.dir, { recursive: true, force: true }).finally(() => finishClosed())
        })
      })
    } catch (error) {
      if (dir && !this.#live.has(idea.id)) await rm(dir, { recursive: true, force: true })
      throw error
    } finally { this.#starting.delete(idea.id) }
  }

  async close(): Promise<void> {
    this.#closed = true
    await Promise.all([...this.#live.values()].map(async live => {
      if (live.child.pid) { try { process.kill(-live.child.pid, 'SIGKILL') } catch { live.child.kill('SIGKILL') } }
      await live.closed
    }))
  }
  busy(ideaId: string): boolean {
    return this.#starting.has(ideaId) || this.#live.has(ideaId)
  }
}
