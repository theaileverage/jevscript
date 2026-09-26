/**
 * The CLI's subprocess adapter protocol (spec section 11.6,
 * `crates/jevscript-cli/README.md`), served by any in-process adapter.
 *
 * `jevscript run --bind dev='jevscript-adapter-codex'` starts the command once
 * for the run and writes one JSON object per line:
 *
 * ```json
 * {"operation":"call","capability":"dev","verb":"spawn","args":{"named":{"prompt":"..."}}}
 * {"operation":"observe","capability":"dev","handle":{"$jev":"handle","capability":"dev","id":"..."}}
 * ```
 *
 * and reads exactly one line back: `{"result": ...}`, `{"observation": ...}`
 * or `{"error": {"message", "retryable"}}`. Requests are answered one at a
 * time in order. Stdout carries protocol lines only; notices go to stderr.
 * The SDKs' subprocess helpers speak the same protocol, so one executable
 * serves the CLI, a Python host and a JS host alike.
 *
 * When stdin closes the server returns. It does not stop the agents it
 * spawned: their handles are reattachable, and stopping is the program's
 * `stop` verb, not a side effect of a host restarting.
 */
import { createInterface } from 'node:readline'
import type { Readable, Writable } from 'node:stream'

import type { Adapter, CallArgs, Handle } from 'jevscript'

/** Where the protocol runs; the defaults are the process's own streams. */
export interface JsonlIo {
  input?: Readable
  output?: Writable
  /** Diagnostics. Defaults to stderr. */
  log?: (message: string) => void
}

interface Request {
  operation?: unknown
  capability?: unknown
  verb?: unknown
  args?: unknown
  handle?: unknown
}

/** Answer one protocol request (exposed for hosts that frame lines themselves). */
export async function answer(adapter: Adapter, line: string): Promise<Record<string, unknown>> {
  let request: Request
  try {
    request = JSON.parse(line) as Request
  } catch (error) {
    return failure(`malformed request: ${error instanceof Error ? error.message : String(error)}`, false)
  }
  if (!request || typeof request !== 'object') return failure('a request must be a JSON object', false)
  const capability = typeof request.capability === 'string' ? request.capability : undefined
  try {
    if (request.operation === 'call') {
      if (typeof request.verb !== 'string') return failure('`call` needs a `verb`', false)
      const args = (request.args && typeof request.args === 'object' ? request.args : {}) as CallArgs
      const result = await adapter.call(request.verb, args, capability)
      return { result: withCapability(adapter, request.verb, capability, result) ?? null }
    }
    if (request.operation === 'observe') {
      if (!adapter.observe) return failure(`\`${capability ?? 'this capability'}\` cannot be observed`, false)
      if (!request.handle || typeof request.handle !== 'object') return failure('`observe` needs a `handle`', false)
      return { observation: await adapter.observe(request.handle as Handle, capability) }
    }
    return failure(`unknown operation \`${String(request.operation)}\``, false)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    const retryable = (error as { retryable?: unknown } | null)?.retryable === true
    return failure(message, retryable)
  }
}

/** Serve the protocol until input closes. */
export async function serveJsonl(adapter: Adapter, io: JsonlIo = {}): Promise<void> {
  const input = io.input ?? process.stdin
  const output = io.output ?? process.stdout
  const lines = createInterface({ input, crlfDelay: Number.POSITIVE_INFINITY })
  for await (const line of lines) {
    if (line.trim() === '') continue
    const reply = await answer(adapter, line)
    await new Promise<void>((resolve, reject) =>
      output.write(`${JSON.stringify(reply)}\n`, (error) => (error ? reject(error) : resolve())),
    )
  }
}

/** A spawned agent's handle names the capability it was bound under (spec section 9). */
function withCapability(adapter: Adapter, verb: string, capability: string | undefined, result: unknown): unknown {
  if (
    adapter.kind === 'agent' &&
    verb === 'spawn' &&
    capability &&
    result !== null &&
    typeof result === 'object' &&
    typeof (result as { id?: unknown }).id === 'string'
  ) {
    return { ...(result as Record<string, unknown>), capability }
  }
  return result
}

function failure(message: string, retryable: boolean): Record<string, unknown> {
  return { error: { message, retryable } }
}
