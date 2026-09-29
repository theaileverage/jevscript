/**
 * The adapters a toolbox run binds (spec sections 9.5 and 11.5). They live in
 * this host process; the runtime reaches them through `capability.call` and
 * `capability.observe`.
 */
import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process'
import { createInterface, type Interface } from 'node:readline'

import type { Adapter, CallArgs, Handle, Observation, ToolManifest } from 'jevscript'
import { ClaudeCodeAdapter } from '@jevscript/adapter-claude-code'

import type { CapabilityKind, IrNeed, IrSignature } from '../shared/ir.ts'
import type { BindingSpec } from '../shared/protocol.ts'

/** Something to close when the run ends. */
export interface BoundAdapter {
  adapter: Adapter
  close(): Promise<void>
}

/**
 * The toolbox's demonstration adapter, the host-side twin of the CLI's
 * `--stub` (spec section 11.6). It mirrors the CLI stub verb for verb, with
 * one visible difference: where the program declared a tool verb's return
 * type, it returns a value of that type (`true`, `0`, `"stub <verb>"`, ...)
 * instead of an empty record, because the runtime checks declared returns
 * (section 9.4) and the CLI stub fails every typed verb with `type_error`.
 * See SPEC-GAPS.md.
 */
export function stubAdapter(need: IrNeed): Adapter {
  let nextHandle = 1
  const observation = (): Observation => ({
    status: 'waiting',
    last_message: 'built-in stub observation',
    tail: 'built-in stub',
  })
  return {
    kind: need.kind,
    call(verb: string, args: CallArgs) {
      switch (`${need.kind}.${verb}`) {
        case 'agent.spawn':
          return { id: `stub-${nextHandle++}` }
        case 'agent.wait':
        case 'agent.observe':
          return observation()
        case 'agent.send':
        case 'agent.stop':
        case 'person.notify':
          return null
        case 'llm.write':
          return args.positional?.[0] ?? ''
      }
      const signature = need.signatures?.find((candidate) => candidate.name === verb)
      return signature ? typedStub(signature, need.name) : {}
    },
    observe: () => observation(),
    ...(need.kind === 'tool' && need.signatures
      ? { manifest: manifestOf(need.signatures) }
      : {}),
  }
}

function typedStub(signature: IrSignature, capability: string): unknown {
  switch (signature.returns) {
    case 'text':
      return `stub ${signature.name}`
    case 'number':
      return 0
    case 'bool':
      return true
    case 'list':
      return []
    case 'record':
      return {}
    case 'handle':
      return { id: `stub-${signature.name}`, $jev: 'handle', capability }
    case 'none':
      return null
  }
}

function manifestOf(signatures: readonly IrSignature[]): ToolManifest {
  return {
    verbs: Object.fromEntries(
      signatures.map((signature) => [
        signature.name,
        { ...(signature.params ? { params: signature.params } : {}), returns: signature.returns },
      ]),
    ),
  }
}

/**
 * A persistent subprocess speaking the JSONL protocol of spec section 11.6:
 * one request line in, one `{ result }`, `{ observation }` or `{ error }` line
 * out. The command is the user's own, typed into the toolbox, never text a
 * program produced.
 */
export class SubprocessAdapter implements Adapter {
  readonly kind: CapabilityKind
  readonly name: string
  readonly manifest?: ToolManifest
  readonly #child: ChildProcessWithoutNullStreams
  readonly #lines: Interface
  readonly #waiting: ((line: string | null) => void)[] = []
  #queue: Promise<unknown> = Promise.resolve()
  #stderr = ''
  /** Set once the child can no longer answer; later exchanges fail at once instead of waiting. */
  #gone = false

  constructor(name: string, kind: CapabilityKind, command: string, manifest?: ToolManifest) {
    this.name = name
    this.kind = kind
    if (manifest) this.manifest = manifest
    this.#child = spawn('sh', ['-c', command], { stdio: ['pipe', 'pipe', 'pipe'] })
    this.#child.stderr.setEncoding('utf8').on('data', (chunk: string) => {
      this.#stderr = (this.#stderr + chunk).slice(-4000)
    })
    this.#child.on('error', () => this.#drain())
    // Writing to a child that has exited is EPIPE; without a listener it would take the server down.
    this.#child.stdin.on('error', () => this.#drain())
    this.#lines = createInterface({ input: this.#child.stdout })
    this.#lines.on('line', (line) => this.#waiting.shift()?.(line))
    this.#lines.on('close', () => this.#drain())
  }

  call(verb: string, args: CallArgs): Promise<unknown> {
    return this.#exchange({ operation: 'call', capability: this.name, verb, args }, 'result')
  }

  observe(handle: Handle): Promise<Observation> {
    return this.#exchange({ operation: 'observe', capability: this.name, handle }, 'observation') as Promise<Observation>
  }

  async close(): Promise<void> {
    this.#child.stdin.end()
    if (this.#child.exitCode === null) this.#child.kill()
  }

  /** One exchange at a time: the protocol has no request ids. */
  #exchange(request: unknown, field: string): Promise<unknown> {
    const next = this.#queue.then(async () => {
      const line = this.#gone
        ? null
        : await new Promise<string | null>((resolve) => {
            this.#waiting.push(resolve)
            this.#child.stdin.write(`${JSON.stringify(request)}\n`)
          })
      if (line === null) {
        throw new Error(`adapter \`${this.name}\` exited unexpectedly${this.#stderr ? `: ${this.#stderr.trim()}` : ''}`)
      }
      let reply: Record<string, unknown>
      try {
        reply = JSON.parse(line) as Record<string, unknown>
      } catch (error) {
        throw new Error(`adapter \`${this.name}\` returned malformed JSON: ${String(error)}`)
      }
      if (reply['error']) {
        const failure = reply['error'] as { message?: string; retryable?: boolean }
        throw Object.assign(new Error(failure.message ?? 'adapter error'), { retryable: failure.retryable === true })
      }
      if (!(field in reply)) throw new Error(`adapter \`${this.name}\` reply lacks \`${field}\``)
      return reply[field]
    })
    this.#queue = next.catch(() => undefined)
    return next
  }

  #drain(): void {
    this.#gone = true
    for (const resolve of this.#waiting.splice(0)) resolve(null)
  }
}

/** Receives what a `person` bound to the toolbox is told with `notify`. */
export type Notify = (capability: string, message: string) => void

/**
 * A `person` bound to this page. `ask` and `take_over` never reach it: the
 * runtime turns them into `confirm` and `escalate` pauses (spec section 9.2),
 * which land in the pause stack. Only `notify` is a call.
 */
export function toolboxPerson(name: string, notify: Notify): Adapter {
  return {
    kind: 'person',
    call(verb: string, args: CallArgs) {
      if (verb === 'notify') {
        notify(name, String(args.positional?.[0] ?? ''))
        return null
      }
      throw new Error(`a \`person\` bound to the toolbox has no verb \`${verb}\``)
    },
  }
}

export function parseManifest(text: string | undefined): ToolManifest | undefined {
  if (!text?.trim()) return undefined
  const value = JSON.parse(text) as ToolManifest
  if (!value || typeof value !== 'object' || typeof value.verbs !== 'object') {
    throw new Error('a manifest is `{ "verbs": { ... } }` (spec section 9.4)')
  }
  return value
}

/** Build the adapter a binding spec names for one declared capability. */
export function bindAdapter(need: IrNeed, spec: BindingSpec | undefined, notify: Notify): BoundAdapter {
  const done = async () => {}
  switch (spec?.kind ?? (need.kind === 'person' ? 'toolbox' : 'stub')) {
    case 'toolbox':
      if (need.kind !== 'person') throw new Error(`only a \`person\` can be bound to the toolbox, not \`${need.name}\``)
      return { adapter: toolboxPerson(need.name, notify), close: done }
    case 'subprocess': {
      const binding = spec as Extract<BindingSpec, { kind: 'subprocess' }>
      if (!binding.command.trim()) throw new Error(`\`${need.name}\` is bound to a subprocess with no command`)
      const adapter = new SubprocessAdapter(need.name, need.kind, binding.command, parseManifest(binding.manifest))
      return { adapter, close: () => adapter.close() }
    }
    case 'claude-code': {
      if (need.kind !== 'agent') throw new Error(`Claude Code binds an \`agent\`, and \`${need.name}\` is a \`${need.kind}\``)
      const binding = spec as Extract<BindingSpec, { kind: 'claude-code' }>
      const adapter = new ClaudeCodeAdapter({
        capability: need.name,
        session: binding.session,
        idleSeconds: binding.idleSeconds,
        pollMs: binding.pollMs,
      })
      return { adapter, close: done }
    }
    default:
      return { adapter: stubAdapter(need), close: done }
  }
}
