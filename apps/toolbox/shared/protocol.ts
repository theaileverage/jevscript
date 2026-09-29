/**
 * The WebSocket protocol between the toolbox page and its local server, and
 * the records both sides persist. Requests carry an `id` and get exactly one
 * `reply`; runs push their pauses and recording events as they happen.
 */
import type { Pin, PinTarget } from './annotator.ts'
import type { Ir } from './ir.ts'
import type { Pause, Resume } from './pauses.ts'
import type { Profile, RecordingEvent } from './recording.ts'
import type { ResendRecord } from './requests.ts'
import type { JevRequest } from './wire.ts'

/** Section 12's diagnostic, as `jevscript check` prints it: `file:line:col: code: message`. */
export interface Diagnostic {
  file: string
  line: number
  column: number
  severity: 'error' | 'warning'
  code: string
  message: string
}

export interface CheckResult {
  diagnostics: Diagnostic[]
  /** Present when the program compiled (warnings allowed). */
  ir: Ir | null
}

/**
 * How one declared capability is bound for a toolbox run. `stub` is the
 * toolbox's host-side demonstration adapter; `subprocess` speaks the JSONL
 * protocol of spec section 11.6; `claude-code` is `adapters/claude-code`;
 * `toolbox` binds a `person` to this page, where `ask` and `take_over` land in
 * the pause stack.
 */
export type BindingSpec =
  | { kind: 'stub' }
  | { kind: 'subprocess'; command: string; manifest?: string }
  | { kind: 'claude-code'; session: string; idleSeconds: number; pollMs: number }
  | { kind: 'toolbox' }

export interface DraftProgram {
  source: string
  diagnostics: Diagnostic[]
  /** Zero errors. */
  clean: boolean
  /** Model turns spent: the draft plus any repairs. */
  attempts: number
}

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant'
  text: string
  program?: DraftProgram
  /** Toolbox host provenance; independent of runtime recordings in section 10.3. */
  origin?: ChatOrigin
  at: string
}

/** Toolbox drafting selection, separate from the Jev profile in section 10.6. */
export interface ChatAgent {
  harness: 'claude-code' | 'codex'
  model: string
}

/** Only the local CLI's discovered choices are offered as models. */
export interface AgentModel {
  id: string
  name: string
  resolved: string
}

/** Discovery does not prove that a later provider request will succeed. */
export type AgentStatus = {
  harness: ChatAgent['harness']
  models: AgentModel[]
} & ({ state: 'available' } | { state: 'unavailable'; reason: string })

/** A check or a failed invocation cannot be mistaken for an agent response. */
export type ChatOrigin =
  | { kind: 'agent'; harness: ChatAgent['harness']; model: string; requestedModel: string }
  | { kind: 'error'; harness: ChatAgent['harness']; model: string }
  | { kind: 'api' | 'fixture'; model: string }
  | { kind: 'check' }

/** Idea-owned source files; only Jev files enter the compiler of section 11.1. */
export interface IdeaFile {
  id: string
  name: string
  kind: 'jev' | 'host'
  source: string
}

/** A saved selection and reply, independent from machine pins and runtime events. */
export interface FileAnnotation {
  id: string
  fileId: string
  from: number
  to: number
  selected: string
  query: string
  reply: string
  origin: ChatOrigin
  at: string
}

/** Jev programs and their host files share one idea and one SQLite save. */
export interface IdeaWorkspace {
  files: IdeaFile[]
  activeFileId: string
  entryFileId: string
  annotations: FileAnnotation[]
}

/** A finished (or abandoned) run, kept with its idea. */
export interface RunSummary {
  runId: string
  recording: string
  startedAt: string
  replay: boolean
  outcome: string | null
  verified: boolean
  outputs: Record<string, unknown>
  usage: { calls: number; tokens: number; usd: number; steps: number } | null
}

/** A saved program plus its chat, persisted under the toolbox home, never in the repo. */
export interface Idea {
  id: string
  title: string
  fileName: string
  source: string
  createdAt: string
  updatedAt: string
  messages: ChatMessage[]
  /** The Anthropic message history, kept append-only and unedited so it can be resent as is. */
  claudeHistory: unknown[]
  /** Saved Chat CLI choice. Null retains compatibility with older API conversations. */
  chatAgent: ChatAgent | null
  workspace: IdeaWorkspace
  /** JSON text of the inputs record. */
  inputs: string
  bindings: Record<string, BindingSpec>
  model: string
  sample: boolean
  runs: RunSummary[]
  pins: Pin[]
}

export interface ProfileInfo extends Profile {
  source: 'bundled' | 'overlay'
}

export interface ServerStatus {
  bin: string
  home: string
  /** The SQLite database holding ideas, pins, the run index and resend history. */
  database: string
  claude: { available: boolean; model: string }
  agents: AgentStatus[]
  services: 'standard' | 'demo'
  typesafeKey: boolean
  profilesOverlay: string | null
  /** The language server command, and whether its program exists. */
  lsp: { command: string; available: boolean }
}

export interface ReplayResult {
  pauses: Pause[]
  events: RecordingEvent[]
  exitCode: number | null
  stderr: string
}

export interface StartRun {
  ideaId: string
  title: string
  fileName: string
  source: string
  inputs: Record<string, unknown>
  bindings: Record<string, BindingSpec>
  model: string
  sample: boolean
  files?: IdeaFile[]
  /** Host-selected task; defaults to main (spec section 11.2). */
  task?: string
}

/** Jev compile and Node syntax check; neither executes the host file. */
export interface PairCheck {
  jev: CheckResult
  host: { ok: boolean; message: string }
}

export type Request =
  | { type: 'status' }
  | { type: 'agents.refresh' }
  | { type: 'check'; fileName: string; source: string; files?: IdeaFile[] }
  | { type: 'tools.check'; fileName: string; source: string; manifests: Record<string, unknown>; files?: IdeaFile[] }
  | { type: 'profiles' }
  | { type: 'ideas.list' }
  | { type: 'ideas.save'; idea: Idea }
  | { type: 'ideas.delete'; ideaId: string }
  | { type: 'chat.send'; idea: Idea; text: string }
  | { type: 'annotate'; idea: Idea; target: PinTarget; query: string }
  | { type: 'file.annotate'; idea: Idea; fileId: string; from: number; to: number; query: string }
  | { type: 'pair.check'; idea: Idea; hostFileId: string }
  | { type: 'pair.run'; idea: Idea; hostFileId: string }
  | { type: 'run.start'; run: StartRun }
  | { type: 'run.resume'; runId: string; payload: Resume }
  | { type: 'run.abort'; runId: string }
  | { type: 'run.inject'; runId: string; capability: string; message: string }
  | { type: 'replay'; recording: string }
  | { type: 'recording.read'; recording: string }
  | { type: 'pane.tail'; pane: string }
  | {
      type: 'resend'
      ideaId: string
      recording: string
      requestId: string
      request: JevRequest
    }
  | { type: 'resends.list'; ideaId: string; recording: string; requestId: string }
  | { type: 'errors.reference' }
  | { type: 'runs.live' }
  | { type: 'judge'; fileName: string; source: string; judgment: string; state: Record<string, unknown>; model: string }

export interface Replies {
  status: ServerStatus
  'agents.refresh': { agents: AgentStatus[] }
  check: CheckResult
  'tools.check': { missing: string[]; diagnostics: Diagnostic[] }
  profiles: { profiles: ProfileInfo[]; overlay: string | null }
  'ideas.list': { ideas: Idea[] }
  'ideas.save': { idea: Idea }
  'ideas.delete': { ok: true }
  'chat.send': { idea: Idea }
  annotate: { pin: Pin }
  'file.annotate': { idea: Idea; annotation: FileAnnotation }
  'pair.check': PairCheck
  'pair.run': { runId: string; recording: string }
  'run.start': { runId: string; recording: string }
  'run.resume': { ok: true }
  'run.abort': { ok: true }
  'run.inject': { ok: true }
  replay: ReplayResult
  'recording.read': { events: RecordingEvent[] }
  'pane.tail': { text: string }
  resend: ResendRecord
  'resends.list': { history: ResendRecord[] }
  'errors.reference': Record<string, { meaning: string; correction: string }>
  judge: { answers: Record<string, unknown> }
  'runs.live': { runs: LiveRunState[] }
}

/** A run still in progress on the server, so a reloaded page can pick it back up. */
export interface LiveRunState {
  runId: string
  ideaId: string
  title: string
  recording: string
  startedAt: string
  events: RecordingEvent[]
  pauses: Pause[]
  /** The pause the run is parked on, if the host still has to answer it. */
  waitingOn: Pause | null
}

export type ClientMessage = Request & { id: number }

export type ServerMessage =
  | { type: 'reply'; id: number; ok: true; result: unknown }
  | { type: 'reply'; id: number; ok: false; error: string }
  | { type: 'run.event'; runId: string; event: RecordingEvent }
  | { type: 'run.pause'; runId: string; ideaId: string; title: string; pause: Pause }
  | { type: 'run.ended'; runId: string; ideaId: string; summary: RunSummary }
  | { type: 'run.failed'; runId: string; ideaId: string; message: string }
  | { type: 'notify'; runId: string; capability: string; message: string }
  | { type: 'idea.updated'; idea: Idea }
  | { type: 'host.ended'; ideaId: string; ok: boolean; message: string }

/** Parse `jevscript check`/`compile` stderr lines: `file:line:col: [warning: ]code: message`. */
export function parseDiagnostics(stderr: string): Diagnostic[] {
  const diagnostics: Diagnostic[] = []
  for (const line of stderr.split('\n')) {
    const match = /^(.*?):(\d+):(\d+): (warning: )?([a-z_]+): (.*)$/.exec(line)
    if (!match) continue
    diagnostics.push({
      file: match[1] as string,
      line: Number(match[2]),
      column: Number(match[3]),
      severity: match[4] ? 'warning' : 'error',
      code: match[5] as string,
      message: match[6] as string,
    })
  }
  return diagnostics
}

export function formatDiagnostic(diagnostic: Diagnostic): string {
  const severity = diagnostic.severity === 'warning' ? 'warning: ' : ''
  return `${diagnostic.file}:${diagnostic.line}:${diagnostic.column}: ${severity}${diagnostic.code}: ${diagnostic.message}`
}
