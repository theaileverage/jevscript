/**
 * The types the SDK surface is written in.
 *
 * They mirror the runtime's wire types (spec sections 9, 10 and 11.5). The IR
 * is deliberately absent: the SDKs never expose it to application code
 * (spec section 11.1).
 */

/** The seven pause kinds (spec section 10.2). */
export type PauseKind =
  | 'confirm'
  | 'escalate'
  | 'waiting'
  | 'budget'
  | 'error'
  | 'stopped'
  | 'done'

/** Fields every pause carries. */
export interface PauseCommon {
  kind: PauseKind
  run_id: string
  step: number
  /** A pause raised inside a machine carries `machine:<qualified name>`. */
  task: string
  /** The machine state the pause was raised in, if it was raised in one. */
  state?: string
  source: SourceSpan
  recording_offset: number
}

/** A source range in a `.jev` file. */
export interface SourceSpan {
  start: { line: number; column: number; offset: number }
  end: { line: number; column: number; offset: number }
}

/** A person was asked something. Resumes with `{ answer, text? }`. */
export interface ConfirmPause extends PauseCommon {
  kind: 'confirm'
  message: string
  options: string[]
  context: Record<string, unknown>
}

/** A person was handed the run. Resumes with `{ resume: true }`. */
export interface EscalatePause extends PauseCommon {
  kind: 'escalate'
  reason: string
  context: Record<string, unknown>
}

/** Waiting on a capability. Auto-resumes; the host may `inject`. */
export interface WaitingPause extends PauseCommon {
  kind: 'waiting'
  on: string
  condition: string
  timeout_minutes: number
}

/** A budget ran out. Resumes with `{ extend: { key: n } }`. */
export interface BudgetPause extends PauseCommon {
  kind: 'budget'
  key: 'calls' | 'minutes' | 'usd' | 'steps'
  used: number
  limit: number
}

/** Something failed. Resumes with `{ retry: true }` if `retryable`. */
export interface ErrorPause extends PauseCommon {
  kind: 'error'
  code: string
  message: string
  retryable: boolean
}

/** The run stopped. Terminal. */
export interface StoppedPause extends PauseCommon {
  kind: 'stopped'
  reason: string
}

/** What a run cost. */
export interface Usage {
  calls: number
  tokens: number
  usd: number
  minutes: number
  steps: number
  /**
   * What spawned agents reported spending on their own models. For information
   * only: the runtime never sees those calls and never gates on them
   * (spec section 7.1).
   */
  adapter_usd?: number
  /** What spawned agents reported using in tokens, on the same terms. */
  adapter_tokens?: number
}

/** The task finished. Terminal. */
export interface DonePause extends PauseCommon {
  kind: 'done'
  outputs: Record<string, unknown>
  /** True only if a declared `verify` was true at the end (spec section 7.4). */
  verified: boolean
  usage: Usage
}

/** Any pause. */
export type Pause =
  | ConfirmPause
  | EscalatePause
  | WaitingPause
  | BudgetPause
  | ErrorPause
  | StoppedPause
  | DonePause

/** What a host resumes a pause with (spec section 10.2). */
export type Resume =
  | { answer: string; text?: string }
  | { resume: true }
  | { extend: Partial<Record<BudgetPause['key'], number>> }
  | { retry: true }
  | Record<string, never>

/** The four capability kinds (spec section 9). */
export type CapabilityKind = 'agent' | 'person' | 'llm' | 'tool'

/** An opaque reference to something an adapter owns. */
export interface Handle {
  capability: string
  id: string
  [field: string]: unknown
}

/** What an `agent` observation carries at least (spec section 9.1). */
export interface Observation {
  status: 'running' | 'waiting' | 'exited'
  last_message: string
  tail: string
  exit_code?: number | null
  [field: string]: unknown
}

/** The arguments of one capability call. */
export interface CallArgs {
  positional?: unknown[]
  named?: Record<string, unknown>
}

/** One verb of a {@link ToolManifest}. */
export interface ManifestVerb {
  params?: string[]
  returns?: 'text' | 'number' | 'bool' | 'list' | 'record' | 'handle' | 'none'
}

/**
 * What a `tool` adapter may publish about itself (spec section 9.4).
 *
 * At `task.start` the runtime compares every verb the program references
 * against this and refuses to start with `verb_missing` if one is absent, so a
 * mismatch surfaces before any model call.
 */
export interface ToolManifest {
  verbs: Record<string, ManifestVerb>
}

/**
 * An adapter, as the host binds it.
 *
 * The runtime asks for `call(verb, args)` and, for `agent` kinds,
 * `observe(handle)` (spec section 9.5). Adapters must return structured
 * records, never opaque blobs, so that `shape` and `trail` can work on them.
 */
export interface Adapter {
  kind: CapabilityKind
  /** The program-scoped capability name this adapter is bound under. */
  capability?: string
  /** Optional bind hook for adapters that own handles for several bindings. */
  bind?(capability: string): Promise<void> | void
  call(verb: string, args: CallArgs, capability?: string): Promise<unknown> | unknown
  observe?(handle: Handle, capability?: string): Promise<Observation> | Observation
  /** `tool` adapters only: the verbs this adapter implements. */
  manifest?: ToolManifest
}

/**
 * The result of a `pick`, or of a `pick among` over a runtime list
 * (spec sections 6.3 and 6.4a).
 */
export interface Choice {
  /** For a `pick among`, `"i<index>"` or `"none"`. */
  label: string
  confidence: number
  probabilities: Record<string, number>
  /** `pick among` only: the chosen element's position in the list. */
  index?: number | null
  /** `pick among` only: the chosen element itself. */
  item?: unknown
}

/** What a machine call returns (spec section 7.8). */
export interface MachineResult {
  /** The state it finished in. */
  state: string
  steps: number
  /** Whether the final state is terminal. */
  done: boolean
  /**
   * Whether the transition into the terminal state carried a `when` guard.
   * That guard is code, so it is the machine's equivalent of `verify`: a
   * terminal state reached through an unguarded event is done but not verified.
   */
  verified: boolean
  events: { step: number; from: string; event: string; to: string }[]
}

/** A declared input (spec section 3.2). */
export interface InputDecl {
  name: string
  shape: unknown
}

/** A declared capability (spec section 3.4). */
export interface NeedDecl {
  name: string
  kind: CapabilityKind
}

/**
 * A judgment's answer space, as `program.load` reports it: enough to list the
 * questions and their answers without running anything (spec section 11.1).
 */
export interface JudgmentDecl {
  name: string
  params: string[]
  results: JudgmentResultDecl[]
  /** Pin this to refuse a program whose answers moved (spec section 11.4). */
  shape_hash: string
}

/** One named result and its non-executable answer space (spec section 11.1). */
export interface JudgmentResultDecl {
  name: string
  each: boolean
  verb: 'feels' | 'pick' | 'rate' | 'pick_among'
  labels?: string[]
  levels?: Array<{ index: number; name?: string }>
  allow_none?: boolean
}

/** What a recording event looks like on the wire (spec section 10.3). */
export interface RecordingEvent {
  ts: string
  run_id: string
  seq: number
  event: string
  [field: string]: unknown
}

/** The four `log` levels, lowest first (spec section 5.8). */
export type LogLevel = 'debug' | 'info' | 'warn' | 'error'

/**
 * One `log` line (spec section 5.8). The host stream carries logs in full;
 * `redact` governs only what the stored recording keeps.
 */
export interface LogEvent {
  level: LogLevel
  /** The logged value's text form (spec section 4.2). */
  message: string
  /** The structured fields; structured values keep their `$jev` tags. */
  fields: Record<string, unknown>
  /** The task, `machine:<name>` or judgment the log ran under. */
  task: string
  /** Where the `log` was written. */
  source: SourceSpan
  /** The run it came from. Absent for a judgment run alone. */
  run_id?: string
  /** Its position in the run's recording. Absent for a judgment run alone. */
  seq?: number
}

/** A host callback that receives each `log` line as it is written. */
export type LogHandler = (log: LogEvent) => void

/** Options for running a judgment alone (spec section 11.3). */
export interface JudgmentRunOptions {
  /** A Jev model id, which also selects the model profile. */
  model?: string
  /** A profiles file to layer over the bundled ones (spec section 10.6). */
  profiles?: string
  /**
   * Receives the lines the judgment's `log`s wrote, in order, before `run`
   * resolves. A judgment run alone has no recording to hold them.
   */
  onLog?: LogHandler
}

/** What a task is started with (spec section 11.2). */
export interface StartOptions {
  /** A record matching the program's `in` declarations. */
  inputs?: Record<string, unknown>
  /**
   * Capability name to adapter. Every `needs` must be bound, and a `tool`
   * adapter may carry a `manifest` (spec section 9.4).
   */
  bind?: Record<string, Adapter>
  /** Create a recording here; an existing path is refused. */
  record?: string
  /** Replay instead of calling anything; mutually exclusive with `record`. */
  replay?: string
  /** Store a hash and a token count instead of payloads. */
  redact?: boolean
  /** A Jev model id, which also selects the model profile. */
  model?: string
  /**
   * Draw labels and levels from Jev's distribution instead of taking the argmax
   * (spec section 6.11). Every draw goes through the run's recorded random
   * source, so a sampled run still replays exactly.
   */
  sample?: boolean | { seed: number }
  /** A profiles file to layer over the bundled ones (spec section 10.6). */
  profiles?: string
  /**
   * Receives each `log` line this run writes, as it is written (spec section
   * 5.8). A replay checks recorded lines without emitting them again, so a
   * replayed run calls this only for lines written after it leaves the
   * recording.
   */
  onLog?: LogHandler
}
