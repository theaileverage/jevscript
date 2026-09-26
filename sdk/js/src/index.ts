/**
 * The JavaScript host SDK for Jevscript.
 *
 * The surface is spec section 11.2, named idiomatically for JavaScript:
 *
 * ```ts
 * const program = await load('examples/fix_issue.jev')
 * const run = program.task('main').start({ inputs, bind: { claude, tree, me } })
 * for await (const pause of run) {
 *   if (pause.kind === 'confirm') await run.resume({ answer: 'yes' })
 * }
 * ```
 *
 * Every call goes to the Rust runtime over JSON-RPC on stdio (spec section
 * 11.5). Adapters stay here in the host process: the runtime asks back for
 * `capability.call` and `capability.observe`.
 *
 * The client, runtime methods, adapter callbacks and event stream are all live.
 * Replays are served entirely from the recording and never call adapters or
 * the model endpoint.
 */
import { HOST_METHODS, METHODS, RpcClient } from './rpc.ts'
import type {
  Adapter,
  CallArgs,
  Handle,
  InputDecl,
  JudgmentDecl,
  JudgmentRunOptions,
  LogEvent,
  LogHandler,
  LogLevel,
  NeedDecl,
  Pause,
  RecordingEvent,
  Resume,
  StartOptions,
} from './types.ts'

export * from './types.ts'
export { SubprocessAgentAdapter, SubprocessAdapterError, subprocessAgent, type SubprocessAgentOptions } from './subprocess-adapter.ts'
export { JevscriptRpcError, METHODS, HOST_METHODS, RpcClient } from './rpc.ts'

/** The `log` levels in order, lowest first (spec section 5.8). */
export const LOG_LEVELS: readonly LogLevel[] = ['debug', 'info', 'warn', 'error']

/**
 * The `log` line a recording event carries, or `undefined` for any other
 * event (spec section 5.8).
 */
export function logEvent(event: RecordingEvent): LogEvent | undefined {
  if (event.event !== 'log') return undefined
  return {
    level: event.level as LogLevel,
    message: event.message as string,
    fields: (event.fields as Record<string, unknown> | undefined) ?? {},
    task: event.task as string,
    source: event.source as LogEvent['source'],
    run_id: event.run_id,
    seq: event.seq,
  }
}

/** How to reach the runtime. */
export interface LoadOptions {
  /** The runtime binary. Defaults to `$JEVSCRIPT_BIN`, then `jevscript`. */
  bin?: string
  /** Working directory for the runtime process. */
  cwd?: string
  /** Treat the argument as source rather than a path. */
  source?: boolean
  /** Module search roots used before the linked program is returned (spec 3.9). */
  paths?: string[]
}

/** What `program.load` answers with. */
interface ProgramLoadResult {
  program_id: string
  name: string
  inputs: InputDecl[]
  needs: NeedDecl[]
  judgments: JudgmentDecl[]
}

/**
 * Load a program by path, or by source when `options.source` is set.
 *
 * Spawns one runtime process per program. Close it with {@link Program.close}.
 */
export async function load(pathOrSource: string, options: LoadOptions = {}): Promise<Program> {
  const bindings = new Map<string, Map<string, Adapter>>()
  const listeners = new Set<(event: RecordingEvent) => void>()
  const history: RecordingEvent[] = []
  const client = new RpcClient({
    ...(options.bin === undefined ? {} : { bin: options.bin }),
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    onHostRequest: (method, params) => dispatch(bindings, method, params),
    onEvent: (params) => {
      const event = (params as { event?: RecordingEvent })?.event
      if (event) {
        history.push(event)
        for (const listener of listeners) listener(event)
      }
    },
  })

  const params = options.source
    ? { source: pathOrSource, paths: options.paths ?? [] }
    : { path: pathOrSource, paths: options.paths ?? [] }

  try {
    const result = await client.request<ProgramLoadResult>(METHODS.programLoad, params)
    return new Program(client, result, bindings, listeners, history)
  } catch (error) {
    await client.close()
    throw error
  }
}

/** Route a `capability.call` or `capability.observe` to the bound adapter. */
async function dispatch(
  bindings: Map<string, Map<string, Adapter>>,
  method: string,
  params: unknown,
): Promise<unknown> {
  const request = params as {
    run_id: string
    capability: string
    verb?: string
    args?: CallArgs
    handle?: Handle
  }
  const adapter = bindings.get(request.run_id)?.get(request.capability)
  if (!adapter) throw new Error(`no adapter is bound for \`${request.capability}\``)

  if (method === HOST_METHODS.capabilityObserve) {
    if (!adapter.observe) throw new Error(`\`${request.capability}\` cannot be observed`)
    return {
      observation: await adapter.observe(request.handle as Handle, request.capability),
    }
  }
  if (method === HOST_METHODS.capabilityCall) {
    const verb = request.verb ?? ''
    const result = await adapter.call(verb, request.args ?? {}, request.capability)
    return { result: encodeAdapterResult(adapter, verb, request.capability, result) }
  }
  throw new Error(`unknown method \`${method}\``)
}

/** Encode only results whose declared capability contract makes them handles. */
function encodeAdapterResult(
  adapter: Adapter,
  verb: string,
  capability: string,
  result: unknown,
): unknown {
  if (adapter.manifest?.verbs[verb]?.returns === 'handle') {
    if (result === null || typeof result !== 'object' || typeof (result as { id?: unknown }).id !== 'string') {
      throw new Error(`\`${capability}.${verb}\` declared a handle result without a string id`)
    }
    return { ...(result as Record<string, unknown>), $jev: 'handle', capability }
  }
  if (
    adapter.kind === 'agent' &&
    verb === 'spawn' &&
    result !== null &&
    typeof result === 'object' &&
    typeof (result as { id?: unknown }).id === 'string'
  ) {
    return { ...(result as Record<string, unknown>), capability }
  }
  return result
}

/** A loaded program. */
export class Program {
  readonly name: string
  /** Declared inputs (spec section 3.2). */
  readonly inputs: InputDecl[]
  /** Declared capabilities, with kinds (spec section 3.4). */
  readonly needs: NeedDecl[]
  /** Judgments with their answer spaces and shape hashes (spec section 11.4). */
  readonly judgments: JudgmentDecl[]

  readonly #client: RpcClient
  readonly #id: string
  readonly #bindings: Map<string, Map<string, Adapter>>
  readonly #listeners: Set<(event: RecordingEvent) => void>
  readonly #history: RecordingEvent[]

  constructor(
    client: RpcClient,
    loaded: ProgramLoadResult,
    bindings: Map<string, Map<string, Adapter>>,
    listeners: Set<(event: RecordingEvent) => void>,
    history: RecordingEvent[],
  ) {
    this.#client = client
    this.#id = loaded.program_id
    this.#bindings = bindings
    this.#listeners = listeners
    this.#history = history
    this.name = loaded.name
    this.inputs = loaded.inputs ?? []
    this.needs = loaded.needs ?? []
    this.judgments = loaded.judgments ?? []
  }

  /** One judgment, which can be run on its own with no bindings (spec 11.3). */
  judgment(name: string): Judgment {
    return new Judgment(this.#client, this.#id, name)
  }

  /** One task. */
  task(name: string): Task {
    return new Task(
      this.#client,
      this.#id,
      name,
      this.#bindings,
      this.#listeners,
      this.#history,
    )
  }

  /** Shut the runtime process down. */
  async close(): Promise<void> {
    await this.#client.close()
  }
}

/** A named judgment (spec section 6.7). */
export class Judgment {
  readonly name: string
  readonly #client: RpcClient
  readonly #programId: string

  constructor(client: RpcClient, programId: string, name: string) {
    this.#client = client
    this.#programId = programId
    this.name = name
  }

  /**
   * Run this judgment against a state: exactly one Jev request, no bindings
   * needed (spec section 11.3).
   */
  async run(
    state: Record<string, unknown>,
    options: JudgmentRunOptions = {},
  ): Promise<Record<string, unknown>> {
    const { onLog, ...wire } = options
    const result = await this.#client.request<{
      answers: Record<string, unknown>
      logs?: LogEvent[]
    }>(METHODS.judgmentRun, { program_id: this.#programId, name: this.name, state, ...wire })
    for (const line of result.logs ?? []) onLog?.({ ...line, fields: line.fields ?? {} })
    return result.answers
  }
}

/** A task (spec section 7). */
export class Task {
  readonly name: string
  readonly #client: RpcClient
  readonly #programId: string
  readonly #bindings: Map<string, Map<string, Adapter>>
  readonly #listeners: Set<(event: RecordingEvent) => void>
  readonly #history: RecordingEvent[]

  constructor(
    client: RpcClient,
    programId: string,
    name: string,
    bindings: Map<string, Map<string, Adapter>>,
    listeners: Set<(event: RecordingEvent) => void>,
    history: RecordingEvent[],
  ) {
    this.#client = client
    this.#programId = programId
    this.name = name
    this.#bindings = bindings
    this.#listeners = listeners
    this.#history = history
  }

  /**
   * Start the task. The returned run is an async iterable of pauses: iterate it,
   * answer each pause with {@link Run.resume}, and the iteration ends when the
   * run reaches a terminal pause.
   */
  start(options: StartOptions = {}): Run {
    const bindHooks: Promise<void>[] = []
    for (const [name, adapter] of Object.entries(options.bind ?? {})) {
      adapter.capability = name
      bindHooks.push(Promise.resolve(adapter.bind?.(name)))
    }
    const bindings = Object.entries(options.bind ?? {}).map(([name, adapter]) => ({
      name,
      kind: adapter.kind,
      ...(adapter.manifest === undefined ? {} : { manifest: adapter.manifest }),
    }))
    const started = Promise.all(bindHooks)
      .then(() => this.#client.request<{ run_id: string }>(METHODS.taskStart, {
        program_id: this.#programId,
        name: this.name,
        inputs: options.inputs ?? {},
        bindings,
        ...(options.record === undefined ? {} : { record: options.record }),
        ...(options.replay === undefined ? {} : { replay: options.replay }),
        ...(options.redact === undefined ? {} : { redact: options.redact }),
        ...(options.model === undefined ? {} : { model: options.model }),
        ...(options.sample === undefined ? {} : { sample: options.sample }),
        ...(options.profiles === undefined ? {} : { profiles: options.profiles }),
      }))
      .then((result) => {
        this.#bindings.set(result.run_id, new Map(Object.entries(options.bind ?? {})))
        return result
      })
    return new Run(this.#client, started, this.#listeners, this.#history, options.onLog)
  }
}

/** A started run (spec section 10.1). */
export class Run {
  readonly #client: RpcClient
  readonly #started: Promise<{ run_id: string }>
  readonly #listeners: Set<(event: RecordingEvent) => void>
  readonly #history: RecordingEvent[]
  /**
   * Where this run's events begin in the program's history. A replay reuses
   * its recording's run id, so an earlier run's events can carry the same id
   * (spec section 10.4).
   */
  readonly #historyStart: number
  #ended = false
  #resumeVersion = 0
  #logListener: ((event: RecordingEvent) => void) | undefined

  constructor(
    client: RpcClient,
    started: Promise<{ run_id: string }>,
    listeners: Set<(event: RecordingEvent) => void>,
    history: RecordingEvent[],
    onLog?: LogHandler,
  ) {
    this.#client = client
    this.#started = started
    this.#listeners = listeners
    this.#history = history
    this.#historyStart = history.length
    if (onLog) this.#forwardLogs(onLog)
  }

  #forwardLogs(onLog: LogHandler): void {
    const held: RecordingEvent[] = []
    let id: string | undefined
    const deliver = (event: RecordingEvent) => {
      const line = event.run_id === id ? logEvent(event) : undefined
      if (line) onLog(line)
    }
    const listener = (event: RecordingEvent) => {
      if (id === undefined) held.push(event)
      else deliver(event)
    }
    this.#logListener = listener
    this.#listeners.add(listener)
    this.#started.then(
      (result) => {
        id = result.run_id
        for (const event of held.splice(0)) deliver(event)
      },
      () => this.#stopLogs(),
    )
  }

  #stopLogs(): void {
    if (this.#logListener) this.#listeners.delete(this.#logListener)
    this.#logListener = undefined
  }

  #finish(): void {
    this.#ended = true
    this.#stopLogs()
  }

  /** This run's id, once the runtime has assigned one. */
  async id(): Promise<string> {
    return (await this.#started).run_id
  }

  /** Advance until the next pause. */
  async next(): Promise<Pause> {
    const pause = await this.#client.request<Pause>(METHODS.runNext, {
      run_id: await this.id(),
    })
    if (
      pause.kind === 'done' ||
      pause.kind === 'stopped' ||
      (pause.kind === 'error' && !pause.retryable)
    ) {
      this.#finish()
    }
    return pause
  }

  /** Answer the open pause (spec section 10.2). */
  async resume(payload: Resume): Promise<void> {
    await this.#client.request(METHODS.runResume, { run_id: await this.id(), payload })
    this.#resumeVersion += 1
  }

  /** Give up on the run. */
  async abort(): Promise<void> {
    this.#ended = true
    await this.#client.request(METHODS.runAbort, { run_id: await this.id() })
    this.#finish()
  }

  /** Send a message to a capability during a `waiting` pause. */
  async inject(capability: string, message: string): Promise<void> {
    await this.#client.request(METHODS.runInject, {
      run_id: await this.id(),
      capability,
      message,
    })
  }

  /** Recording events as they are written (spec section 10.3). */
  events(): AsyncIterable<RecordingEvent> {
    const queue: RecordingEvent[] = []
    let notify: (() => void) | undefined
    const history = this.#history
    const historyStart = this.#historyStart
    const listeners = this.#listeners
    const runId = this.id()
    const ended = () => this.#ended

    return {
      async *[Symbol.asyncIterator]() {
        const id = await runId
        const seen = new Set<number>()
        const enqueue = (event: RecordingEvent) => {
          if (event.run_id === id && !seen.has(event.seq)) {
            seen.add(event.seq)
            queue.push(event)
            notify?.()
          }
        }
        const listener = (event: RecordingEvent) => enqueue(event)
        for (const event of history.slice(historyStart)) enqueue(event)
        listeners.add(listener)
        try {
          while (!ended() || queue.length > 0) {
            const event = queue.shift()
            if (event) {
              yield event
              continue
            }
            await new Promise<void>((resolve) => {
              notify = resolve
            })
          }
        } finally {
          listeners.delete(listener)
          notify = undefined
        }
      },
    }
  }

  /** This run's `log` lines as they are written (spec section 5.8). */
  logs(): AsyncIterable<LogEvent> {
    const events = this.events()
    return {
      async *[Symbol.asyncIterator]() {
        for await (const event of events) {
          const line = logEvent(event)
          if (line) yield line
        }
      },
    }
  }

  /** Iterate pauses until the run reaches a terminal one. */
  async *[Symbol.asyncIterator](): AsyncIterator<Pause> {
    while (!this.#ended) {
      const pause = await this.next()
      const resumeVersion = this.#resumeVersion
      yield pause
      if (pause.kind === 'done' || pause.kind === 'stopped') return
      if (
        (pause.kind === 'escalate' || (pause.kind === 'error' && !pause.retryable)) &&
        this.#resumeVersion === resumeVersion
      ) {
        this.#finish()
        return
      }
    }
  }
}
