/** Bounded SDK companion execution over JSONL stdio, spec sections 11.2 and 11.5. */
import { execFile, spawn, type ChildProcess } from 'node:child_process'
import { cp, mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import type { Duplex } from 'node:stream'
import { fileURLToPath } from 'node:url'
import { RpcClient, type CallArgs, type Handle } from 'jevscript'
import type { Idea, LiveRunState, RunSummary, ServerMessage } from '../shared/protocol.ts'
import { needsHost, type Pause, type Resume } from '../shared/pauses.ts'
import type { RecordingEvent } from '../shared/recording.ts'
import { bindAdapter, type BoundAdapter } from './adapters.ts'
import { PY_CONTEXT, TS_CONTEXT } from './host-context.ts'
import type { Jevscript } from './jevscript.ts'
import { summarize, type RunManager } from './runs.ts'

const ROOT = fileURLToPath(new URL('../../../', import.meta.url))
/** Live SDK host and its owned stdio connection, spec sections 11.2 and 11.5. */
interface Hosted extends LiveRunState {
  child: ChildProcess
  rpc: RpcClient
  answerPort: Duplex
  closed: Promise<void>
}

// Apply after interpreter startup: inherited stdio works, but generated code cannot spawn or open sockets.
const LINUX_SECCOMP = `import ctypes
policy = ctypes.CDLL('libseccomp.so.2')
policy.seccomp_init.argtypes = [ctypes.c_uint32]
policy.seccomp_init.restype = ctypes.c_void_p
policy.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
policy.seccomp_rule_add.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int, ctypes.c_uint]
policy.seccomp_load.argtypes = [ctypes.c_void_p]
policy.seccomp_release.argtypes = [ctypes.c_void_p]
context = policy.seccomp_init(0x7fff0000)
if not context:
    raise RuntimeError('Could not initialize Python host confinement.')
try:
    for name in ('fork', 'vfork', 'clone', 'clone3', 'execve', 'execveat', 'socket', 'socketpair', 'connect', 'bind', 'listen', 'accept', 'accept4'):
        syscall = policy.seccomp_syscall_resolve_name(name.encode())
        if syscall >= 0 and policy.seccomp_rule_add(context, 0x00050001, syscall, 0) != 0:
            raise RuntimeError('Could not configure Python host confinement.')
    if policy.seccomp_load(context) != 0:
        raise RuntimeError('Could not enable Python host confinement.')
finally:
    policy.seccomp_release(context)
`

/** Confines Python SDK hosts at the host boundary (spec section 11.5). */
export async function pythonCommand(dir: string, host: string): Promise<{ bin: string; args: string[] }> {
  const discovered = await new Promise<{ executable: string; framework: string | null; prefix: string }>((resolve, reject) => execFile('python3', ['-c', "import json,sys,sysconfig,os;name=sysconfig.get_config_var('PYTHONFRAMEWORK');app=os.path.join(sys.prefix,'Resources',str(name)+'.app','Contents','MacOS',str(name));print(json.dumps({'executable':sys.executable,'framework':app if name and os.path.isfile(app) else None,'prefix':sys.base_prefix}))"], { env: { PATH: process.env['PATH'] }, timeout: 5000 }, (error, stdout) => {
    if (error) reject(new Error('Python 3 is unavailable. Install it to run Python hosts.'))
    else { try { resolve(JSON.parse(stdout)) } catch { reject(new Error('Python executable discovery failed.')) } }
  }))
  // Framework python3 is a launcher; execute its discovered interpreter directly.
  const python = await realpath(discovered.framework ?? discovered.executable)
  const bootstrap = `import sys,runpy;sys.path[:0]=[${JSON.stringify(dir)},${JSON.stringify(join(dir, '.toolbox/python'))}];runpy.run_path(${JSON.stringify(host)},run_name='__main__')`
  if (process.platform === 'darwin') {
    // Credentials and project files are unreadable. Only the copied SDK, workspace and Python's libraries are readable.
    const profile = `(version 1)(deny default)(allow process-exec (literal ${JSON.stringify(python)}))(allow file-read* (literal "/"))(allow file-read-metadata (vnode-type DIRECTORY))(allow sysctl-read)(allow mach-lookup)(allow file-read* (subpath ${JSON.stringify(dir)}) (subpath "/Library/Frameworks") (subpath "/System") (subpath "/usr") (subpath "/opt/homebrew") (literal "/dev/null") (literal "/dev/urandom"))(allow file-write* (subpath ${JSON.stringify(dir)}))`
    return { bin: '/usr/bin/sandbox-exec', args: ['-p', profile, python, '-I', '-B', '-c', bootstrap] }
  }
  if (process.platform === 'linux') {
    await new Promise<void>((resolve, reject) => execFile('bwrap', ['--version'], { timeout: 5000 }, error => error ? reject(new Error('Python hosts require bubblewrap on Linux. Install bwrap; no unsandboxed host was started.')) : resolve()))
    // Bubblewrap passes inherited FDs to its command; no preserve-fds option exists in 0.9.
    return { bin: 'bwrap', args: ['--unshare-all', '--die-with-parent', '--ro-bind', '/usr', '/usr', '--ro-bind', '/lib', '/lib', '--ro-bind-try', '/lib64', '/lib64', '--ro-bind', discovered.prefix, discovered.prefix, '--dev', '/dev', '--proc', '/proc', '--bind', dir, dir, '--chdir', dir, python, '-I', '-B', '-c', LINUX_SECCOMP + '\n' + bootstrap] }
  }
  throw new Error('Python hosts need a supported OS sandbox (macOS Seatbelt or Linux bubblewrap).')
}

/** Executes saved SDK companions while the toolbox owns effects and recordings (11.2). */
export class SdkHosts {
  readonly #live = new Map<string, Hosted>()
  readonly #starting = new Set<string>()
  readonly #children = new Map<ChildProcess, Promise<void>>()
  #closed = false
  readonly jev: Jevscript
  readonly runs: RunManager
  readonly home: string
  readonly send: (message: ServerMessage) => void
  readonly ended: (ideaId: string, summary: RunSummary) => Promise<void>
  constructor(jev: Jevscript, runs: RunManager, home: string, send: (message: ServerMessage) => void, ended: (ideaId: string, summary: RunSummary) => Promise<void>) {
    this.jev = jev; this.runs = runs; this.home = home; this.send = send; this.ended = ended
  }
  supports(idea: Idea, id: string): boolean {
    const file = idea.workspace.files.find(file => file.id === id)
    return Boolean(file && (file.name.endsWith('.py') || /(?:from\s*['"](?:@theaileverage\/)?jevscript['"]|import\s+jevscript)/.test(file.source)))
  }
  busy(id: string): boolean { return this.#starting.has(id) || [...this.#live.values()].some(run => run.ideaId === id) }
  owns(id: string): boolean { return this.#live.has(id) }
  live(): LiveRunState[] { return [...this.#live.values()].map(({ child: _child, rpc: _rpc, answerPort: _port, closed: _closed, ...run }) => run) }
  resume(id: string, payload: Resume): void {
    const live = this.#live.get(id)
    if (!live?.waitingOn) throw new Error('This host has no pause awaiting an answer.')
    live.waitingOn = null
    live.answerPort.write(JSON.stringify(payload) + '\n')
  }
  async abort(id: string): Promise<void> {
    const live = this.#live.get(id)
    if (!live) throw new Error('No SDK host has this run ID.')
    await live.rpc.request('run.abort', { run_id: id })
    live.answerPort.write('{"abort":true}\n')
  }
  async inject(id: string, capability: string, message: string): Promise<void> {
    const live = this.#live.get(id)
    if (!live) throw new Error('No SDK host has this run ID.')
    await live.rpc.request('run.inject', { run_id: id, capability, message })
  }

  async files(idea: Idea, id: string): Promise<{ dir: string; host: string }> {
    const host = idea.workspace.files.find(file => file.id === id && file.kind === 'host')
    if (!host || !/\.(py|ts|mts|js|mjs)$/.test(host.name)) throw new Error('Choose a TypeScript, JavaScript or Python host.')
    const names = new Set<string>()
    let bytes = 0
    for (const file of idea.workspace.files) {
      if (!file.name.split('/').every(part => /^[A-Za-z0-9_.-]+$/.test(part) && !['.', '..'].includes(part)) || ['package.json', 'node_modules'].includes(file.name) || /^(\.toolbox|node_modules)\//.test(file.name) || file.name === '.toolbox') throw new Error('Workspace files need safe relative names outside reserved runtime folders.')
      if (names.has(file.name)) throw new Error('Workspace file names must be unique.')
      names.add(file.name); bytes += Buffer.byteLength(file.source)
    }
    if (bytes > 1024 * 1024) throw new Error('The host workspace exceeds the 1 MiB source limit.')
    await mkdir(join(this.home, 'host-work'), { recursive: true })
    const dir = await realpath(await mkdtemp(join(this.home, 'host-work/pair-')))
    try {
      for (const file of idea.workspace.files) {
        const path = join(dir, file.name)
        await mkdir(dirname(path), { recursive: true }); await writeFile(path, file.source, { mode: 0o600 })
      }
      await mkdir(join(dir, '.toolbox'))
      await writeFile(join(dir, '.toolbox/runtime.ts'), TS_CONTEXT)
      if (!idea.workspace.files.some(file => file.name === 'toolbox_runtime.py')) await writeFile(join(dir, 'toolbox_runtime.py'), PY_CONTEXT)
      await writeFile(join(dir, 'package.json'), '{"type":"module"}')
      await cp(join(ROOT, 'sdk/js/dist'), join(dir, 'node_modules/jevscript/dist'), { recursive: true })
      await cp(join(dir, 'node_modules/jevscript'), join(dir, 'node_modules/@theaileverage/jevscript'), { recursive: true })
      await writeFile(join(dir, 'node_modules/@theaileverage/jevscript/package.json'), '{"type":"module","exports":"./dist/index.js"}')
      await writeFile(join(dir, 'node_modules/jevscript/package.json'), '{"type":"module","exports":"./dist/index.js"}')
      await cp(join(ROOT, 'sdk/python/src/jevscript'), join(dir, '.toolbox/python/jevscript'), { recursive: true, filter: path => !path.includes('__pycache__') })
      return { dir, host: join(dir, host.name) }
    } catch (error) { await rm(dir, { recursive: true, force: true }); throw error }
  }
  async syntax(host: string): Promise<boolean> {
    const python = host.endsWith('.py')
    return new Promise(resolve => execFile(python ? 'python3' : process.execPath, python ? ['-I', '-c', 'import ast,sys;ast.parse(open(sys.argv[1]).read())', host] : ['--check', host], { env: { PATH: process.env['PATH'] }, timeout: 5000, maxBuffer: 64 * 1024 }, error => resolve(!error)))
  }

  async start(idea: Idea, id: string): Promise<{ runId: string; recording: string }> {
    if (this.#closed || this.busy(idea.id) || this.runs.busy(idea.id)) throw new Error('This idea already has a run in progress, or the host runner is closing.')
    this.#starting.add(idea.id)
    let dir: string | null = null
    let rpc: RpcClient | null = null
    const bound = new Map<string, BoundAdapter>()
    try {
      const compiled = await this.jev.compile(idea.fileName, idea.source, idea.workspace.files)
      if (!compiled.ir) throw new Error('The companion Jev program does not compile.')
      const files = await this.files(idea, id); dir = files.dir
      if (!await this.syntax(files.host)) throw new Error('The selected host has invalid syntax.')
      const inputs: unknown = JSON.parse(idea.inputs || '{}')
      if (!inputs || typeof inputs !== 'object' || Array.isArray(inputs)) throw new Error('Idea inputs must be a JSON object.')
      const recording = await this.runs.newRecordingPath(idea.title)
      let runId = ''
      const events: RecordingEvent[] = []
      for (const need of compiled.ir.needs) bound.set(need.name, bindAdapter(need, idea.bindings[need.name], (capability, message) => this.send({ type: 'notify', runId, capability, message })))
      await writeFile(join(dir, '.toolbox/config.json'), JSON.stringify({ inputs, recording, model: idea.model, sample: idea.sample, bindings: [...bound].map(([name, { adapter }]) => ({ name, kind: adapter.kind, ...(adapter.manifest ? { manifest: adapter.manifest } : {}) })) }), { mode: 0o600 })
      const runtime = rpc = new RpcClient({ bin: this.jev.bin, cwd: dir,
        onHostRequest: async (method, raw) => {
          const request = raw as { capability: string; verb?: string; args?: CallArgs; handle?: Handle }
          const adapter = bound.get(request.capability)?.adapter
          if (!adapter) throw new Error('No configured adapter is bound to this capability.')
          if (method === 'capability.observe') {
            if (!adapter.observe) throw new Error('This adapter has no observe verb.')
            return { observation: await adapter.observe(request.handle!, request.capability) }
          }
          const verb = request.verb ?? ''
          const result = await adapter.call(verb, request.args ?? {}, request.capability)
          const handle = adapter.manifest?.verbs[verb]?.returns === 'handle'
          if (handle && (!result || typeof result !== 'object' || typeof (result as Handle).id !== 'string')) throw new Error('The adapter returned an invalid handle.')
          return { result: (handle || adapter.kind === 'agent' && verb === 'spawn') && result && typeof result === 'object' ? { ...result, ...(handle ? { $jev: 'handle' } : {}), capability: request.capability } : result }
        },
        onEvent: raw => {
          const event = (raw as { event?: RecordingEvent }).event
          if (event) { events.push(event); if (runId) this.send({ type: 'run.event', runId, event }) }
        },
      })
      const command = files.host.endsWith('.py') ? await pythonCommand(dir, files.host) : { bin: process.execPath, args: ['--permission', `--allow-fs-read=${dir}`, `--allow-fs-write=${dir}`, files.host] }
      if (this.#closed) throw new Error('The host runner is closing.')
      const child = spawn(command.bin, command.args, { cwd: dir, detached: true, env: { PATH: process.env['PATH'], LANG: 'en_US.UTF-8', JEVS_TOOLBOX_EMBEDDED: '1' }, stdio: ['ignore', 'pipe', 'pipe', 'pipe', 'pipe'] })
      const port = child.stdio[3] as Duplex
      const answerPort = child.stdio[4] as Duplex
      return await new Promise((accept, reject) => {
        let accepted = false; let failure = ''; let loaded = false; let taskRequested = false; let bytes = 0; let control = ''
        let finishClosed!: () => void
        const live: Hosted = { runId: '', ideaId: idea.id, title: idea.title, recording, startedAt: new Date().toISOString(), events, pauses: [], waitingOn: null, child, rpc: runtime, answerPort, closed: new Promise(done => { finishClosed = done }) }
        this.#children.set(child, live.closed)
        const stop = (message: string) => {
          failure ||= message
          if (child.pid) { try { process.kill(-child.pid, 'SIGKILL') } catch { child.kill('SIGKILL') } }
        }
        const startup = setTimeout(() => stop('The host did not start its SDK run within 5 seconds.'), 5000)
        const timeout = setTimeout(() => stop('The host exceeded the 10 minute runtime limit.'), 600_000)
        const countOutput = (chunk: Buffer) => { bytes += chunk.length; if (bytes > 1024 * 1024) stop('The host exceeded the 1 MiB output limit.') }
        child.stdout?.on('data', countOutput); child.stderr?.on('data', countOutput)
        port.on('error', () => stop('The SDK connection closed unexpectedly.'))
        answerPort.on('error', () => undefined)
        createInterface({ input: answerPort }).on('line', line => {
          if (line.length > 64 * 1024) { stop('The pause request exceeds the size limit.'); return }
          try {
            const pause = JSON.parse(line) as Pause
            const last = live.pauses.at(-1)
            if (!last || pause.run_id !== runId || pause.kind !== last.kind || !needsHost(last)) throw new Error('Invalid pause.')
            // run.next already published this pause; an early UI answer may be buffered.
          } catch { stop('The host requested an invalid pause answer.') }
        })
        port.setEncoding('utf8').on('data', (chunk: string) => {
          control += chunk
          if (Buffer.byteLength(control) > 2 * 1024 * 1024) { stop('The SDK request exceeds the 2 MiB limit.'); return }
          let newline: number
          while ((newline = control.indexOf('\n')) >= 0) {
            const line = control.slice(0, newline); control = control.slice(newline + 1)
            void (async () => {
              let requestId: unknown
              try {
                const request = JSON.parse(line) as { id: number; method: string; params: Record<string, unknown> }
                requestId = request.id
                const params = { ...request.params }
                if (request.method === 'program.load') {
                  if (loaded) throw new Error('Load one companion program per host launch.')
                  loaded = true
                  if (typeof params.path !== 'string' || !idea.workspace.files.some(file => file.kind === 'jev' && (file.name === params.path || join(files.dir, file.name) === params.path))) throw new Error('Load a saved Jev companion from this idea.')
                  params.path = resolve(files.dir, params.path); params.paths = []
                  delete params.source
                } else if (request.method === 'task.start') {
                  if (taskRequested || !loaded) throw new Error('Start one SDK task per host launch.')
                  taskRequested = true
                  params.record = recording; delete params.replay; params.redact = true
                  if (JSON.stringify(params.inputs).length > 64 * 1024) throw new Error('Host inputs exceed the 64 KiB limit.')
                } else if (!['run.next', 'run.resume', 'run.abort', 'run.inject'].includes(request.method) || !runId || params.run_id !== runId) throw new Error('The SDK request is outside this host run.')
                const result = await runtime.request<Record<string, unknown>>(request.method, params)
                if (request.method === 'task.start') {
                  runId = String(result.run_id); live.runId = runId; this.#live.set(runId, live)
                  clearTimeout(startup); accepted = true; accept({ runId, recording })
                  for (const event of events) this.send({ type: 'run.event', runId, event })
                }
                if (request.method === 'run.next') {
                  const pause = result as unknown as Pause
                  live.pauses.push(pause)
                  live.waitingOn = needsHost(pause) ? pause : null
                  this.send({ type: 'run.pause', runId, ideaId: idea.id, title: idea.title, pause })
                }
                port.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n')
              } catch {
                port.write(JSON.stringify({ jsonrpc: '2.0', id: requestId, error: { code: -32000, message: 'The SDK request failed. Check the companion program, inputs and configured adapters.' } }) + '\n')
              }
            })()
          }
        })
        child.on('error', () => { failure = 'The selected host interpreter could not start.' })
        child.on('close', code => {
          clearTimeout(startup); clearTimeout(timeout)
          void (async () => {
            if (runId) {
              const last = live.pauses.at(-1)
              if (!last || !['done', 'stopped'].includes(last.kind) && !(last.kind === 'error' && !last.retryable)) {
                await runtime.request('run.abort', { run_id: runId }).catch(() => undefined)
                await runtime.request('run.next', { run_id: runId }).catch(() => undefined)
              }
            }
            await runtime.close()
            await Promise.allSettled([...bound.values()].map(adapter => adapter.close()))
            this.#live.delete(runId)
            if (accepted) {
              const summary = await summarize(runId, recording, live.startedAt, false)
              await this.ended(idea.id, summary)
              this.send({ type: 'run.ended', runId, ideaId: idea.id, summary })
            }
            const ok = accepted && !failure && code === 0
            const message = ok ? 'SDK host finished. The Jev result and recording are saved with this idea.' : failure || 'The SDK host exited unsuccessfully. Check its imports, inputs and SDK calls.'
            if (!accepted) reject(new Error(message))
            this.send({ type: 'host.ended', ideaId: idea.id, ok, message })
            await rm(files.dir, { recursive: true, force: true }); this.#children.delete(child); finishClosed()
          })().catch(() => { reject(new Error('The host could not finish.')); this.#children.delete(child); finishClosed() })
        })
      })
    } catch (error) {
      await rpc?.close(); await Promise.allSettled([...bound.values()].map(adapter => adapter.close()))
      if (dir) await rm(dir, { recursive: true, force: true })
      throw error
    } finally { this.#starting.delete(idea.id) }
  }
  async close(): Promise<void> {
    this.#closed = true
    await Promise.all([...this.#children].map(async ([child, closed]) => {
      if (child.pid) { try { process.kill(-child.pid, 'SIGKILL') } catch { child.kill('SIGKILL') } }
      await closed
    }))
  }
}
