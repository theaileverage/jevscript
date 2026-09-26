/**
 * The JSON-RPC 2.0 client (spec section 11.5).
 *
 * The runtime is a local process — `jevscript serve` — and the SDK speaks to it
 * over stdio, one JSON object per line. Requests go host to runtime; the
 * runtime asks back for `capability.call` and `capability.observe`, because
 * adapters live here in the host process and the runtime never links against
 * tmux, a browser or an agent CLI.
 */
import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process'
import { createInterface, type Interface } from 'node:readline'
import { resolveBinary } from './native.ts'

/** Methods the host calls on the runtime. */
export const METHODS = {
  programLoad: 'program.load',
  judgmentRun: 'judgment.run',
  taskStart: 'task.start',
  runNext: 'run.next',
  runResume: 'run.resume',
  runAbort: 'run.abort',
  runInject: 'run.inject',
} as const

/** Requests and notifications the runtime sends back to the host. */
export const HOST_METHODS = {
  capabilityCall: 'capability.call',
  capabilityObserve: 'capability.observe',
  event: 'event',
} as const

/** A JSON-RPC error returned by the runtime. */
export class JevscriptRpcError extends Error {
  readonly code: number
  readonly data: unknown

  constructor(code: number, message: string, data?: unknown) {
    super(message)
    this.name = 'JevscriptRpcError'
    this.code = code
    this.data = data
  }
}

/** Handles a request the runtime sent to the host. */
export type HostHandler = (method: string, params: unknown) => Promise<unknown>

export interface RpcClientOptions {
  /**
   * The runtime binary. Defaults to `$JEVSCRIPT_BIN`, then the verified
   * release-matched binary packaged with this SDK.
   */
  bin?: string
  /** Extra arguments, before `serve`. */
  args?: string[]
  /** Working directory for the runtime process. */
  cwd?: string
  /** Called for `capability.call` and `capability.observe`. */
  onHostRequest?: HostHandler
  /** Called for every `event` notification. */
  onEvent?: (params: unknown) => void
}

interface Pending {
  resolve: (value: unknown) => void
  reject: (error: unknown) => void
}

/** A line-delimited JSON-RPC client over a spawned `jevscript serve`. */
export class RpcClient {
  #child: ChildProcessWithoutNullStreams
  #lines: Interface
  #pending = new Map<number, Pending>()
  #nextId = 1
  #closed = false
  #options: RpcClientOptions

  constructor(options: RpcClientOptions = {}) {
    this.#options = options
    const bin = resolveBinary(options.bin)
    const args = [...(options.args ?? []), 'serve']
    this.#child = spawn(bin, args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    })
    this.#child.on('error', (error) => this.#failAll(error))
    this.#child.on('exit', () => this.#failAll(new Error('the runtime exited')))
    this.#lines = createInterface({ input: this.#child.stdout })
    this.#lines.on('line', (line) => {
      void this.#receive(line)
    })
  }

  /** Send a request and wait for its result. */
  async request<T>(method: string, params: unknown): Promise<T> {
    if (this.#closed) throw new Error('the runtime connection is closed')
    const id = this.#nextId++
    const promise = new Promise<unknown>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject })
    })
    this.#write({ jsonrpc: '2.0', id, method, params })
    return (await promise) as T
  }

  /** Send a notification, which takes no reply. */
  notify(method: string, params: unknown): void {
    this.#write({ jsonrpc: '2.0', method, params })
  }

  /** Shut the runtime down. */
  async close(): Promise<void> {
    if (this.#closed) return
    this.#closed = true
    this.#lines.close()
    this.#child.stdin.end()
    await new Promise<void>((resolve) => {
      if (this.#child.exitCode !== null) {
        resolve()
        return
      }
      this.#child.once('exit', () => resolve())
      this.#child.kill()
    })
  }

  #write(message: unknown): void {
    this.#child.stdin.write(`${JSON.stringify(message)}\n`)
  }

  async #receive(line: string): Promise<void> {
    if (line.trim() === '') return
    let message: Record<string, unknown>
    try {
      message = JSON.parse(line) as Record<string, unknown>
    } catch {
      return
    }

    // A request or notification from the runtime: a capability call, or an event.
    if (typeof message['method'] === 'string') {
      await this.#handleIncoming(message)
      return
    }

    const id = message['id']
    if (typeof id !== 'number') return
    const pending = this.#pending.get(id)
    if (!pending) return
    this.#pending.delete(id)
    const error = message['error'] as { code: number; message: string; data?: unknown } | undefined
    if (error) {
      pending.reject(new JevscriptRpcError(error.code, error.message, error.data))
    } else {
      pending.resolve(message['result'])
    }
  }

  async #handleIncoming(message: Record<string, unknown>): Promise<void> {
    const method = message['method'] as string
    const params = message['params']
    if (method === HOST_METHODS.event) {
      this.#options.onEvent?.(params)
      return
    }
    const id = message['id']
    if (id === undefined) return
    const handler = this.#options.onHostRequest
    if (!handler) {
      this.#write({
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: 'no adapter is bound for this call' },
      })
      return
    }
    try {
      const result = await handler(method, params)
      this.#write({ jsonrpc: '2.0', id, result })
    } catch (error) {
      this.#write({
        jsonrpc: '2.0',
        id,
        result: {
          error: {
            message: error instanceof Error ? error.message : String(error),
            retryable:
              typeof error === 'object' &&
              error !== null &&
              (error as { retryable?: unknown }).retryable === true,
          },
        },
      })
    }
  }

  #failAll(error: unknown): void {
    for (const pending of this.#pending.values()) pending.reject(error)
    this.#pending.clear()
  }
}
