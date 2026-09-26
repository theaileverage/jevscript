/**
 * Bind a persistent JSONL adapter process to a JavaScript host (spec sections
 * 9.5 and 11.6; crates/jevscript-cli/README.md). The agent executable is the
 * same one accepted by `jevscript run --bind`.
 */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { createInterface } from 'node:readline'

import type { Adapter, CallArgs, Handle, Observation } from './types.ts'

/** An error returned by a spec section 11.6 adapter executable. */
export class SubprocessAdapterError extends Error {
  readonly retryable: boolean

  constructor(message: string, retryable = false) {
    super(message)
    this.name = 'SubprocessAdapterError'
    this.retryable = retryable
  }
}

/** Launch options for a spec section 11.6 adapter. Pass argv, never a shell command. */
export interface SubprocessAgentOptions {
  command: string
  args?: string[]
  cwd?: string
  env?: NodeJS.ProcessEnv
}

/**
 * A spec section 9.1 `agent` capability backed by one persistent subprocess. Calls are
 * serialized because JSONL replies have no request ids. Call `close()` after
 * the program is done; it closes the adapter process, not any agent panes.
 */
export class SubprocessAgentAdapter implements Adapter {
  readonly kind = 'agent' as const
  capability?: string
  readonly #child: ChildProcessWithoutNullStreams
  #queue: Promise<unknown> = Promise.resolve()
  #reply: ((line: string) => void) | undefined
  #reject: ((error: Error) => void) | undefined
  #failure: Error | undefined
  #closed = false
  #finished = false

  constructor(options: SubprocessAgentOptions) {
    this.#child = spawn(options.command, options.args ?? [], {
      stdio: ['pipe', 'pipe', 'pipe'],
      ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
      ...(options.env === undefined ? {} : { env: options.env }),
    })
    this.#child.stderr.pipe(process.stderr, { end: false })
    const lines = createInterface({ input: this.#child.stdout, crlfDelay: Infinity })
    lines.on('line', (line) => this.#reply?.(line))
    this.#child.on('error', (error) => this.#fail(error))
    this.#child.on('close', (code, signal) => {
      this.#finished = true
      this.#fail(new Error(`adapter process exited (${code ?? signal ?? 'unknown'})`))
    })
  }

  async call(verb: string, args: CallArgs, capability?: string): Promise<unknown> {
    const reply = await this.#request({ operation: 'call', capability: capability ?? this.capability, verb, args })
    if (!Object.hasOwn(reply, 'result')) throw new SubprocessAdapterError('adapter reply has no result')
    return reply['result']
  }

  async observe(handle: Handle, capability?: string): Promise<Observation> {
    const reply = await this.#request({ operation: 'observe', capability: capability ?? this.capability, handle })
    const observation = reply['observation']
    if (!observation || typeof observation !== 'object' || Array.isArray(observation)) {
      throw new SubprocessAdapterError('adapter reply has no observation')
    }
    return observation as Observation
  }

  /** Close and reap the child. Agents it spawned retain reattachable handles. */
  async close(): Promise<void> {
    if (this.#closed) return
    this.#closed = true
    if (this.#finished) return
    const exited = new Promise<void>((resolve) => this.#child.once('close', () => resolve()))
    this.#child.stdin.end()
    const timer = setTimeout(() => this.#child.kill('SIGKILL'), 5_000)
    try { await exited } finally { clearTimeout(timer) }
  }

  #request(message: Record<string, unknown>): Promise<Record<string, unknown>> {
    const run = async (): Promise<Record<string, unknown>> => {
      if (this.#closed || this.#failure) throw this.#failure ?? new Error('adapter connection is closed')
      const answer = new Promise<string>((resolve, reject) => {
        this.#reply = resolve
        this.#reject = reject
      })
      try {
        this.#child.stdin.write(`${JSON.stringify(message)}\n`)
        const line = await answer
        let reply: unknown
        try { reply = JSON.parse(line) } catch { throw new SubprocessAdapterError('adapter emitted malformed JSON') }
        if (!reply || typeof reply !== 'object' || Array.isArray(reply)) {
          throw new SubprocessAdapterError('adapter reply must be a JSON object')
        }
        const object = reply as Record<string, unknown>
        if (object['error']) {
          const error = object['error'] as { message?: unknown; retryable?: unknown }
          throw new SubprocessAdapterError(String(error.message ?? 'adapter error'), error.retryable === true)
        }
        return object
      } finally {
        this.#reply = undefined
        this.#reject = undefined
      }
    }
    const result = this.#queue.then(run, run)
    this.#queue = result.catch(() => undefined)
    return result
  }

  #fail(error: Error): void {
    this.#failure = error
    this.#reject?.(error)
  }
}

/** Construct a subprocess backed `agent` binding from an executable and argv. */
export function subprocessAgent(command: string, args: string[] = [], options: Omit<SubprocessAgentOptions, 'command' | 'args'> = {}): SubprocessAgentAdapter {
  return new SubprocessAgentAdapter({ command, args, ...options })
}
