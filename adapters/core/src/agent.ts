/**
 * The `agent` capability (spec section 9.1), once, for every harness and backend.
 *
 * `AgentAdapter` is what a host binds: it answers `call(verb, args)` for the
 * five verbs and `observe(handle)`, exactly the adapter contract of spec
 * section 9.5, so it plugs into the JS SDK's `bind`, and through `serveJsonl`
 * into the CLI's subprocess protocol and the SDKs' subprocess helpers.
 *
 * Handles are plain JSON holding everything needed to find the agent again:
 * the backend's pane reference, the working directory and whatever the harness
 * adds (a session id, a start time). An adapter in a fresh process given an old
 * handle reattaches to the same pane. The only state kept in memory is a hint
 * about a turn just started (see {@link AgentAdapter.wait}), which a fresh
 * process can do without.
 *
 * Nothing this adapter returns is interpreted by it. `tail` and `last_message`
 * are agent-written text and go to the program as they are (spec section 7.5);
 * the screen is read only for the harness's fixed busy and input-box markers.
 */
import { randomUUID } from 'node:crypto'
import { homedir } from 'node:os'

import type { Adapter, CallArgs, Handle, Observation } from 'jevscript'

import { conditionOf, cwdOf, flag, handleOf, list, minutesOf, optionalString, textOf } from './args.ts'
import {
  type AnyBackendOptions,
  type BackendName,
  backendName,
  backendOf,
  createBackend,
  defaultBackendName,
  type Key,
  type PaneRef,
  type TerminalBackend,
} from './backends/index.ts'
import { AdapterError } from './errors.ts'
import {
  type AgentHandle,
  defaultClassify,
  type DialogRule,
  type Harness,
  type HarnessContext,
  type SpawnRequest,
  TRUST_MODES,
  type TranscriptSummary,
  type TrustMode,
} from './harness.ts'
import type { Clock, Exec, Files } from './process.ts'
import { defaultClock, defaultExec, defaultFiles } from './process.ts'
import { boundTail, lastLines, matchesNear, safeText } from './screen.ts'

/** How a host configures an adapter. Every option is optional. */
export interface AgentAdapterOptions {
  /**
   * The capability name the program bound this adapter under, used when the
   * SDK or the subprocess protocol does not say. Defaults to the harness name.
   */
  capability?: string
  /** The terminal backend: a name, or a backend instance. Defaults to `$JEVSCRIPT_AGENT_BACKEND`, then Herdr. */
  backend?: BackendName | TerminalBackend
  /** Options for backends the adapter constructs (session, state directory, binary). */
  backendOptions?: AnyBackendOptions
  /** The backend session panes are created in (tmux and Zellij session, Herdr session). */
  session?: string
  /** The agent CLI binary. Defaults to the harness's first binary name on `PATH`. */
  bin?: string
  /** Extra arguments passed to every spawn, before the spawn's own. */
  args?: string[]
  /** Default model and effort for spawns that do not name one. */
  model?: string
  effort?: string
  /** Approve the agent's tool use without asking. Off unless a host turns it on. */
  yolo?: boolean
  /** Folder-trust handling; see {@link TrustMode}. Defaults to `off`. */
  trust?: TrustMode
  /** The working directory when a spawn names none. Defaults to `process.cwd()`. */
  cwd?: string
  /** How long `wait idle` lets a screen go unchanged before calling it idle. Default 20. */
  idleSeconds?: number
  /** How often `wait` looks, in milliseconds. Default 1000. */
  pollMs?: number
  /**
   * After a `spawn` or `send`, how long `wait` keeps looking for the turn to
   * start before it believes an idle screen, in seconds. Default 10 after a
   * send and 60 after a spawn (a cold start can take twenty seconds).
   */
  turnStartSeconds?: { send?: number; spawn?: number }
  /** Where notices go (an effort a CLI does not accept, say). Defaults to stderr. */
  log?: (message: string) => void
  exec?: Exec
  clock?: Clock
  files?: Files
  /** Fresh ids. Defaults to `randomUUID`. */
  uuid?: () => string
  home?: string
  env?: Record<string, string | undefined>
  /** Anything else is the harness's own (Claude's `configDir`, say). */
  [option: string]: unknown
}

/** What every observation carries beyond spec section 9.1's four fields. */
export interface AgentObservation extends Observation {
  harness: string
  backend: BackendName
  /** A busy marker is on screen. */
  busy: boolean
  /** A dialog that holds the agent before or during a turn, if one is showing. */
  dialog: DialogRule['kind'] | null
  /** Why a `wait` returned, only on observations that `wait` returns. */
  waited?: 'status' | 'idle' | 'timeout'
}

const DEFAULT_IDLE_SECONDS = 20
const DEFAULT_POLL_MS = 1000
const DEFAULT_SETTLE_MS = 300
const PASTE_SETTLE_MS = 200
const SEND_TURN_START_S = 10
const SPAWN_TURN_START_S = 60
const STARTUP_SECONDS = 60
const DIALOG_LINES = 30

/** A turn that was just asked for and has not been seen running yet. */
interface Pending {
  since: number
  graceMs: number
}

export class AgentAdapter implements Adapter {
  readonly kind = 'agent' as const
  readonly harness: Harness
  readonly options: AgentAdapterOptions
  /** Set by the SDK at bind time (spec section 11.2). */
  capability?: string

  readonly #exec: Exec
  readonly #clock: Clock
  readonly #files: Files
  readonly #uuid: () => string
  readonly #backends = new Map<BackendName, TerminalBackend>()
  readonly #pending = new Map<string, Pending>()
  readonly #defaultBackend: BackendName

  constructor(harness: Harness, options: AgentAdapterOptions = {}, defaultBackend?: BackendName) {
    this.harness = harness
    this.options = options
    this.#exec = options.exec ?? defaultExec
    this.#clock = options.clock ?? defaultClock
    this.#files = options.files ?? defaultFiles
    this.#uuid = options.uuid ?? randomUUID
    const configured = options.backend
    if (configured && typeof configured === 'object') {
      this.#backends.set(configured.name, configured)
      this.#defaultBackend = configured.name
    } else {
      this.#defaultBackend = configured ? backendName(configured) : (defaultBackend ?? defaultBackendName(this.#env()))
    }
  }

  /** The runtime's entry point: one verb, its arguments, its result. */
  async call(verb: string, args: CallArgs, capability?: string): Promise<unknown> {
    switch (verb) {
      case 'spawn':
        return this.spawn(args, capability)
      case 'send':
        return this.send(handleOf(args), textOf(args))
      case 'wait':
        return this.wait(handleOf(args), minutesOf(args), conditionOf(args))
      case 'stop':
        return this.stop(handleOf(args))
      case 'observe':
        return this.observe(handleOf(args))
      default:
        throw new AdapterError(`an \`agent\` has no verb \`${verb}\` (spec section 9.1)`, false)
    }
  }

  /**
   * `spawn in <handle>?, prompt <text>, ...`
   *
   * Named arguments beyond `prompt` and `in` pass through unchecked, as spec
   * section 9.1 allows: `model`, `effort`, `backend`, `session`, `yolo`,
   * `trust`, `cwd`, `args` (extra flags) and any the harness reads itself.
   */
  async spawn(args: CallArgs, capability?: string): Promise<AgentHandle> {
    const named = args.named ?? {}
    const prompt = named['prompt']
    if (typeof prompt !== 'string' || prompt === '') {
      throw new AdapterError('`spawn` needs a `prompt` (spec section 9.1)', false)
    }
    const cwd = cwdOf(named['in']) ?? optionalString(named['cwd']) ?? this.options.cwd ?? process.cwd()
    const backend = this.#backend(named['backend'] === undefined ? this.#defaultBackend : backendName(named['backend']))
    const trust = trustOf(named['trust'] ?? this.options.trust)
    const request: SpawnRequest = {
      bin: optionalString(named['bin']) ?? this.options.bin ?? this.harness.bins[0] ?? this.harness.name,
      prompt: safeText(prompt),
      cwd,
      sessionId: this.#uuid(),
      model: optionalString(named['model']) ?? this.options.model,
      effort: this.#effort(optionalString(named['effort']) ?? this.options.effort),
      yolo: flag(named['yolo']) ?? this.options.yolo ?? false,
      trust,
      args: [...(this.options.args ?? []), ...list(named['args'])],
      named,
      startedAt: this.#clock.now(),
    }
    if (trust === 'register') await this.harness.register?.(request, this.#context())
    const plan = this.harness.launch(request)
    const ref = await backend.create({
      cwd,
      argv: plan.argv,
      env: plan.env ?? {},
      unset: plan.unset ?? [],
      title: `jev-${this.harness.name}-${request.sessionId.slice(0, 8)}`,
      ...(optionalString(named['session']) ?? this.options.session
        ? { session: (optionalString(named['session']) ?? this.options.session) as string }
        : {}),
    })
    const handle: AgentHandle = {
      capability: capability ?? this.capability ?? this.options.capability ?? this.harness.name,
      id: request.sessionId,
      ...ref,
      cwd,
      ...(this.harness.handleFields?.(request) ?? {}),
      ...(trust === 'off' ? {} : { trust }),
    }
    const answerable = trust !== 'off' && (this.harness.screen.dialogs ?? []).some((d) => d.kind === 'trust' && d.answer)
    if (plan.typePrompt || answerable) await this.#startup(handle, backend, plan.typePrompt ? request.prompt : undefined, trust)
    this.#pending.set(keyOf(handle), {
      since: this.#clock.now(),
      graceMs: (this.options.turnStartSeconds?.spawn ?? SPAWN_TURN_START_S) * 1000,
    })
    return handle
  }

  /**
   * `<handle>.observe`
   *
   * `status` is `exited` once the agent's process has ended (its exit status
   * in `exit_code` when the backend can learn it); otherwise the harness
   * decides between `running` and `waiting` from its transcript, its screen
   * markers and, on Herdr, the multiplexer's own agent status. A dialog on
   * screen always means `running`, named in `dialog`.
   */
  async observe(handle: Handle): Promise<AgentObservation> {
    const agent = this.#agentHandle(handle)
    const backend = this.#backend(backendOf(agent))
    const state = await backend.state(agent)
    const screen = state.exists ? await backend.capture(agent) : ''
    const transcript = await this.#transcript(agent)
    const native =
      state.exists && !state.exited && backend.nativeStatus ? await backend.nativeStatus(agent).catch(() => null) : null
    const rules = this.harness.screen
    const scan = rules.scanLines ?? 12
    const busy = matchesNear(screen, rules.busy, scan)
    const idle = typeof rules.idle === 'function' ? rules.idle(screen) : matchesNear(screen, rules.idle, scan)
    const dialog = dialogOf(screen, rules.dialogs)?.kind ?? null
    const classify = this.harness.classify ?? defaultClassify
    const status = state.exited || !state.exists ? 'exited' : classify({ screen, busy, idle, dialog: dialog, transcript, native })

    return {
      status,
      last_message: transcript?.lastMessage ?? '',
      tail: boundTail(screen),
      exit_code: state.exited ? state.exitCode : null,
      ...(this.harness.observationFields?.(agent, transcript) ?? {}),
      ...(transcript ? { turns: transcript.turns, usage: { tokens: transcript.tokens, usd: transcript.usd } } : {}),
      harness: this.harness.name,
      backend: backend.name,
      busy,
      dialog,
      ...(native ? { native_status: native } : {}),
    }
  }

  /**
   * `<handle>.send <text>`
   *
   * Control characters are dropped first (an Escape or a `^C` in the text
   * would be a key, not a character). One line is typed; more than one is
   * pasted so the CLI takes it as one message rather than submitting at the
   * first newline. After a settle (longer for a leading `/` or other popup
   * prefix) one Enter submits.
   */
  async send(handle: Handle, text: string): Promise<void> {
    const agent = this.#agentHandle(handle)
    const backend = this.#backend(backendOf(agent))
    const clean = safeText(text)
    const submit = this.harness.submit ?? {}
    const command = (submit.commandPrefixes ?? ['/']).some((prefix) => clean.startsWith(prefix))
    if (clean.includes('\n')) {
      await backend.paste(agent, clean)
      await this.#clock.sleep(PASTE_SETTLE_MS)
    } else if (clean !== '') {
      await backend.type(agent, clean)
      await this.#clock.sleep(command ? (submit.commandSettleMs ?? 1200) : (submit.settleMs ?? DEFAULT_SETTLE_MS))
    }
    await backend.key(agent, 'Enter')
    if (command && submit.commandExtraEnter) {
      await this.#clock.sleep(400)
      await backend.key(agent, 'Enter')
    }
    this.#pending.set(keyOf(agent), {
      since: this.#clock.now(),
      graceMs: (this.options.turnStartSeconds?.send ?? SEND_TURN_START_S) * 1000,
    })
  }

  /**
   * `<handle>.wait idle, minutes <n>`
   *
   * Polls `observe` until the status is no longer `running`, the screen has
   * not changed for `idleSeconds`, or `minutes` have passed, and returns the
   * last observation with `waited` saying which. This blocks the host's
   * adapter for the duration, which is how spec section 9.6 says a call
   * reaches the host; the runtime pauses the run with `waiting` meanwhile.
   *
   * Turn end is a transition, not a snapshot: right after a `spawn` or `send`
   * the screen can still show the empty input box of the turn before. So a
   * `waiting` seen before the new turn has been seen running is not believed
   * until the turn-start grace has passed. With trust handling on, a trust
   * dialog that appears mid-wait is answered the way `spawn` answers it.
   */
  async wait(handle: Handle, minutes: number, condition = 'idle'): Promise<AgentObservation> {
    if (condition !== 'idle') {
      throw new AdapterError(`\`wait\` knows only \`idle\`, not \`${condition}\` (spec section 9.1)`, false)
    }
    const agent = this.#agentHandle(handle)
    const key = keyOf(agent)
    const idleMs = (this.options.idleSeconds ?? DEFAULT_IDLE_SECONDS) * 1000
    const pollMs = this.options.pollMs ?? DEFAULT_POLL_MS
    const trust = trustOf(agent['trust'] ?? this.options.trust)
    const started = this.#clock.now()
    const deadline = started + minutes * 60_000
    let lastTail: string | undefined
    let changedAt = started

    for (;;) {
      const observation = await this.observe(agent)
      const now = this.#clock.now()
      const pending = this.#pending.get(key)
      if (observation.busy) this.#pending.delete(key)
      if (observation.status === 'exited') {
        this.#pending.delete(key)
        return { ...observation, waited: 'status' }
      }
      if (observation.status === 'waiting') {
        if (!pending || now - pending.since >= pending.graceMs) {
          this.#pending.delete(key)
          return { ...observation, waited: 'status' }
        }
      }
      if (observation.dialog === 'trust' && trust !== 'off') await this.#answer(agent, observation.tail)
      if (observation.tail !== lastTail) {
        lastTail = observation.tail
        changedAt = now
      } else if (now - changedAt >= idleMs && observation.status === 'running') {
        return { ...observation, waited: 'idle' }
      }
      if (now >= deadline) return { ...observation, waited: 'timeout' }
      await this.#clock.sleep(pollMs)
    }
  }

  /**
   * `<handle>.stop`
   *
   * The harness's interrupt keys with their verified gap, then the pane is
   * closed if it is still there. Safe to call again.
   */
  async stop(handle: Handle): Promise<void> {
    const agent = this.#agentHandle(handle)
    const backend = this.#backend(backendOf(agent))
    const state = await backend.state(agent)
    this.#pending.delete(keyOf(agent))
    if (!state.exists) return
    if (!state.exited) {
      for (const key of this.harness.interrupt.keys) {
        await backend.key(agent, key).catch(() => undefined)
        await this.#clock.sleep(this.harness.interrupt.gapMs)
      }
    }
    if ((await backend.state(agent)).exists) await backend.close(agent)
  }

  /** The backend a handle lives on, constructed once per name. */
  #backend(name: BackendName): TerminalBackend {
    let backend = this.#backends.get(name)
    if (!backend) {
      const options: AnyBackendOptions = {
        ...(this.options.backendOptions ?? {}),
        exec: this.#exec,
        clock: this.#clock,
        files: this.#files,
      }
      if (this.options.session && options.session === undefined) options.session = this.options.session
      backend = createBackend(name, options)
      this.#backends.set(name, backend)
    }
    return backend
  }

  /** Return a supported effort value; omit unsupported values from the launch request. */
  #effort(effort: string | undefined): string | undefined {
    if (effort === undefined) return undefined
    const accepted = this.harness.efforts
    if (accepted && accepted.includes(effort)) return effort
    this.#log(
      accepted
        ? `${this.harness.title} accepts effort ${accepted.join(', ')}; \`${effort}\` is omitted`
        : `${this.harness.title} has no verified effort flag; \`${effort}\` is omitted`,
    )
    return undefined
  }

  async #transcript(handle: AgentHandle): Promise<TranscriptSummary | undefined> {
    const source = this.harness.transcript
    if (!source) return undefined
    const path = await source.locate(handle, this.#context())
    return source.parse(path ? await this.#files.read(path) : undefined)
  }

  /**
   * After launch: answer a trust dialog if trust handling is on, and type the
   * prompt once the CLI is ready if it takes none on its command line. Returns
   * once the turn is seen running, the prompt has been typed, or the startup
   * window has passed.
   */
  async #startup(handle: AgentHandle, backend: TerminalBackend, prompt: string | undefined, trust: TrustMode): Promise<void> {
    const pollMs = this.options.pollMs ?? DEFAULT_POLL_MS
    const deadline = this.#clock.now() + STARTUP_SECONDS * 1000
    while (this.#clock.now() < deadline) {
      const state = await backend.state(handle)
      if (!state.exists || state.exited) return
      const screen = await backend.capture(handle)
      const dialog = dialogOf(screen, this.harness.screen.dialogs)
      if (dialog) {
        if (dialog.kind === 'trust' && trust !== 'off' && dialog.answer) await this.#answer(handle, screen)
        else if (!prompt) return
      } else if (prompt !== undefined) {
        if ((this.harness.ready ?? []).some((pattern) => lastLines(screen, 40).some((line) => pattern.test(line)))) {
          await this.send(handle, prompt)
          return
        }
      } else if (matchesNear(screen, this.harness.screen.busy, this.harness.screen.scanLines ?? 12)) {
        return
      }
      await this.#clock.sleep(pollMs)
    }
    if (prompt !== undefined) {
      this.#log(`${this.harness.title} did not show its ready marker; typing the prompt anyway`)
      await this.send(handle, prompt)
    }
  }

  /** Press the verified keys of the trust dialog on screen, if there is one. */
  async #answer(handle: AgentHandle, screen: string): Promise<void> {
    const dialog = dialogOf(screen, this.harness.screen.dialogs)
    if (!dialog || dialog.kind !== 'trust' || !dialog.answer) return
    const backend = this.#backend(backendOf(handle))
    for (const key of dialog.answer) await backend.key(handle, key as Key)
    await this.#clock.sleep(this.options.pollMs ?? DEFAULT_POLL_MS)
  }

  #agentHandle(handle: Handle): AgentHandle {
    const record = handle as Record<string, unknown>
    if (typeof record['cwd'] !== 'string' || typeof record['id'] !== 'string') {
      throw new AdapterError(`this handle was not made by the ${this.harness.title} adapter`, false)
    }
    const agent = record as AgentHandle
    this.harness.validate?.(agent)
    return agent
  }

  #context(): HarnessContext {
    return {
      exec: this.#exec,
      files: this.#files,
      home: this.options.home ?? homedir(),
      env: this.#env(),
      options: this.options,
    }
  }

  #env(): Record<string, string | undefined> {
    return this.options.env ?? process.env
  }

  #log(message: string): void {
    const log = this.options.log ?? ((line: string) => process.stderr.write(`${line}\n`))
    log(`[${this.harness.name}] ${message}`)
  }
}

function trustOf(value: unknown): TrustMode {
  if (value === undefined || value === null || value === false) return 'off'
  if (value === true) return 'dialog'
  if (typeof value === 'string' && (TRUST_MODES as readonly string[]).includes(value)) return value as TrustMode
  throw new AdapterError(`\`trust\` must be one of ${TRUST_MODES.join(', ')}`, false)
}

/** A dialog showing in the bottom of the screen, where dialogs draw; older output above does not count. */
function dialogOf(screen: string, dialogs: readonly DialogRule[] | undefined): DialogRule | undefined {
  if (!dialogs || screen === '') return undefined
  const bottom = lastLines(screen, DIALOG_LINES).join('\n')
  return dialogs.find((dialog) => dialog.all.every((pattern) => pattern.test(bottom)))
}

function keyOf(handle: PaneRef & { id: string }): string {
  return `${String(handle['backend'] ?? 'tmux')}:${String(handle['pane'] ?? handle['terminal'] ?? handle['surface'] ?? '')}:${handle.id}`
}
