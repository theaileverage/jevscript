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

/** Section 12's diagnostic, as `jevscrypt check` prints it: `file:line:col: code: message`. */
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
  at: string
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
  claude: { available: boolean; model: string }
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
}

export type Request =
  | { type: 'status' }
  | { type: 'check'; fileName: string; source: string }
  | { type: 'tools.check'; fileName: string; source: string; manifests: Record<string, unknown> }
  | { type: 'profiles' }
  | { type: 'ideas.list' }
  | { type: 'ideas.save'; idea: Idea }
  | { type: 'ideas.delete'; id: string }
  | { type: 'chat.send'; idea: Idea; text: string }
  | { type: 'annotate'; idea: Idea; target: PinTarget; query: string }
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
  check: CheckResult
  'tools.check': { missing: string[]; diagnostics: Diagnostic[] }
  profiles: { profiles: ProfileInfo[]; overlay: string | null }
  'ideas.list': { ideas: Idea[] }
  'ideas.save': { idea: Idea }
  'ideas.delete': { ok: true }
  'chat.send': { idea: Idea }
  annotate: { pin: Pin }
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

/** Parse `jevscrypt check`/`compile` stderr lines: `file:line:col: [warning: ]code: message`. */
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
