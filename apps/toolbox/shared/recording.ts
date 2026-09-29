/**
 * Views over a recording (spec section 10.3).
 *
 * Every panel that shows what a run did reads it from here, and everything
 * here reads recorded events: the console lines, the batched requests, the
 * trail, call counts and the machine timeline. Nothing is re-run or guessed
 * from source.
 */
import type { Ir, IrMachine, Thresholds } from './ir.ts'
import { printExpr } from './ir.ts'

/** One JSONL line (spec section 10.3). */
export interface RecordingEvent {
  ts: string
  run_id: string
  seq: number
  event: string
  [field: string]: unknown
}

/** Section 10.6: the fields the toolbox reads from a profile. */
export interface Profile {
  model: string
  endpoint?: string
  total_tokens: number
  state_plus_question_tokens: number
  max_questions_per_request: number
  max_criteria_per_question: number
  tokenizer: string
  price_per_million_input_usd: number
  price_per_million_output_usd: number
  aliases?: string | null
}

/** What the `start` event says about the run. */
export interface StartInfo {
  program: string
  task: string
  inputs: Record<string, unknown>
  bindings: { name: string; kind: string }[]
  ir: Ir | null
  profile: Profile | null
  sample: unknown
}

export function startInfo(events: readonly RecordingEvent[]): StartInfo | null {
  const start = events.find((event) => event.event === 'start')
  if (!start) return null
  return {
    program: String(start['program'] ?? ''),
    task: String(start['task'] ?? ''),
    inputs: (start['inputs'] as Record<string, unknown>) ?? {},
    bindings: (start['bindings'] as { name: string; kind: string }[]) ?? [],
    ir: (start['ir'] as Ir | undefined) ?? null,
    profile: (start['profile'] as Profile | undefined) ?? null,
    sample: start['sample'] ?? null,
  }
}

/** Section 10.3's JSON form of an answer. */
export type JevAnswer =
  | { type: 'noul'; id: string; prob: number }
  | { type: 'choice'; id: string; label: string; confidence: number; probabilities: Record<string, number> }
  | {
      type: 'score'
      id: string
      level: number
      score: number
      confidence: number
      probabilities: Record<string, number>
    }

export interface QuestionView {
  id: string
  type: string
  path: string
  /** The labels a Choice was offered, in the order they were sent. */
  labels: string[]
}

/**
 * One Jev request with its answers. A request is one batch: every question in
 * it was grouped by the batching pass (spec section 6.6), which is why the
 * console shows requests rather than questions.
 */
export interface RequestView {
  requestId: string
  seq: number
  state: unknown
  questions: QuestionView[]
  answers: JevAnswer[]
  tokens: number
  usd: number | null
  latencyMs: number | null
  sampled: boolean
  /** An `effect_error` recorded for this request instead of answers. */
  error: string | null
}

export function requests(events: readonly RecordingEvent[]): RequestView[] {
  const views: RequestView[] = []
  const byId = new Map<string, RequestView>()
  for (const event of events) {
    if (event.event === 'request') {
      const view: RequestView = {
        requestId: String(event['request_id']),
        seq: event.seq,
        state: event['state'],
        questions: ((event['questions'] as Record<string, unknown>[]) ?? []).map((question) => ({
          id: String(question['id']),
          type: String(question['type']),
          path: String(question['path'] ?? ''),
          labels: ((question['labels'] as { name: string }[] | undefined) ?? []).map((label) => label.name),
        })),
        answers: [],
        tokens: 0,
        usd: null,
        latencyMs: null,
        sampled: false,
        error: null,
      }
      views.push(view)
      byId.set(view.requestId, view)
    } else if (event.event === 'answers') {
      const view = byId.get(String(event['request_id']))
      if (!view) continue
      const usage = (event['usage'] as { tokens?: number; usd?: number }) ?? {}
      view.answers = (event['answers'] as JevAnswer[]) ?? []
      view.tokens = usage.tokens ?? 0
      view.usd = usage.usd ?? null
      view.latencyMs = (event['latency_ms'] as number | undefined) ?? null
      view.sampled = event['sampled'] === true
    } else if (event.event === 'effect_error' && event['operation'] === 'request') {
      const identity = event['identity'] as { request_id?: string } | undefined
      const view = identity?.request_id ? byId.get(identity.request_id) : undefined
      if (view) view.error = `${String(event['code'])}: ${String(event['message'])}`
    }
  }
  return views
}

/** Section 7.5: a runtime-written step record. */
export interface StepRecord {
  step: number
  action: string
  target: string
  args: string
  changed: boolean
  outcome: string
}

export function trail(events: readonly RecordingEvent[]): StepRecord[] {
  return events.filter((event) => event.event === 'step').map((event) => event['record'] as StepRecord)
}

/** Calls per capability and verb, from `call` and `observe` events. */
export function callCounts(events: readonly RecordingEvent[]): Record<string, Record<string, number>> {
  const counts: Record<string, Record<string, number>> = {}
  for (const event of events) {
    if (event.event !== 'call' && event.event !== 'observe' && event.event !== 'generate') continue
    const capability = String(event['capability'])
    const verb =
      event.event === 'call' ? String(event['verb']) : event.event === 'observe' ? 'observe' : 'write'
    const forCapability = (counts[capability] ??= {})
    forCapability[verb] = (forCapability[verb] ?? 0) + 1
  }
  return counts
}

/** Section 10.2's `usage`, summed from what has been recorded so far. */
export interface UsageSoFar {
  calls: number
  tokens: number
  usd: number
  steps: number
}

/**
 * Usage while a run is still going. `calls` counts model requests and `llm`
 * generations, which is what the `calls` budget counts (spec section 7.1).
 * Spend comes from the answers' recorded `usd` when the runtime priced them,
 * otherwise from the run's own profile price. Once the run ends, the `end`
 * event's usage is the authority.
 */
export function usageSoFar(events: readonly RecordingEvent[]): UsageSoFar {
  const end = events.find((event) => event.event === 'end')
  if (end) {
    const usage = end['usage'] as { calls: number; tokens: number; usd: number; steps: number }
    return { calls: usage.calls, tokens: usage.tokens, usd: usage.usd, steps: usage.steps }
  }
  const price = startInfo(events)?.profile?.price_per_million_input_usd ?? 0
  let calls = 0
  let tokens = 0
  let usd = 0
  let steps = 0
  for (const event of events) {
    if (event.event === 'answers' || event.event === 'generate') {
      calls += 1
      const usage = (event['usage'] as { tokens?: number; usd?: number }) ?? {}
      tokens += usage.tokens ?? 0
      usd += usage.usd ?? ((usage.tokens ?? 0) * price) / 1_000_000
    } else if (event.event === 'machine_step') {
      steps += 1
    } else if (event.event === 'pause') {
      steps = Math.max(steps, Number((event['payload'] as { step?: number })?.step ?? 0))
    }
  }
  return { calls, tokens, usd, steps }
}

/** One line of the Output console. */
export interface OutputLine {
  seq: number
  kind: 'start' | 'call' | 'machine' | 'pause' | 'resume' | 'warning' | 'error' | 'end' | 'generate'
  text: string
  /** Agent-written text is shown but never interpreted (spec section 9.1). */
  agentWritten?: boolean
}

export function outputLines(events: readonly RecordingEvent[]): OutputLine[] {
  const lines: OutputLine[] = []
  let machineStep = 0
  for (const event of events) {
    switch (event.event) {
      case 'start':
        lines.push({ seq: event.seq, kind: 'start', text: `start ${String(event['program'])}.${String(event['task'])}` })
        break
      case 'call':
        lines.push({
          seq: event.seq,
          kind: 'call',
          text: `${String(event['capability'])}.${String(event['verb'])} ${argsText(event['args'])}`.trim(),
        })
        break
      case 'generate':
        lines.push({ seq: event.seq, kind: 'generate', text: `${String(event['capability'])}.write` })
        break
      case 'machine_step': {
        machineStep += 1
        const chosen = String(event['chosen'])
        lines.push({
          seq: event.seq,
          kind: 'machine',
          text: `${machineStep}  ${String(event['state'])}  → ${chosen}  ${fixed(event['confidence'])}${
            event['to'] !== event['state'] ? `  ⇒ ${String(event['to'])}` : ''
          }`,
        })
        break
      }
      case 'pause': {
        const payload = event['payload'] as Record<string, unknown>
        const kind = String(event['kind'])
        if (kind === 'done' || kind === 'stopped') break
        lines.push({ seq: event.seq, kind: kind === 'error' ? 'error' : 'pause', text: `pause ${kind}  ${pauseSummary(payload)}` })
        break
      }
      case 'resume':
        lines.push({ seq: event.seq, kind: 'resume', text: `resume ${JSON.stringify(event['payload'])}` })
        break
      case 'effect_error':
        lines.push({
          seq: event.seq,
          kind: 'error',
          text: `${String(event['operation'])} failed: ${String(event['code'])}: ${String(event['message'])}`,
        })
        break
      case 'warning':
        lines.push({ seq: event.seq, kind: 'warning', text: `warning ${String(event['code'])}: ${String(event['message'])}` })
        break
      case 'abort':
        lines.push({ seq: event.seq, kind: 'end', text: `abort ${String(event['reason'])}` })
        break
      case 'end':
        lines.push({
          seq: event.seq,
          kind: 'end',
          text: `${String(event['kind'])}${event['verified'] ? ' · verified' : ''}  ${argsText(event['outputs'])}`.trim(),
        })
        break
    }
  }
  return lines
}

export function pauseSummary(payload: Record<string, unknown>): string {
  switch (payload['kind']) {
    case 'confirm':
      return String(payload['message'] ?? '')
    case 'escalate':
    case 'stopped':
      return String(payload['reason'] ?? '')
    case 'waiting':
      return `on ${String(payload['on'])}: ${String(payload['condition'])}`
    case 'budget':
      return `${String(payload['key'])} ${String(payload['used'])} of ${String(payload['limit'])}`
    case 'error':
      return `${String(payload['code'])}: ${String(payload['message'])}`
    default:
      return ''
  }
}

function argsText(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (typeof value === 'object' && !Array.isArray(value)) {
    const record = value as { positional?: unknown[]; named?: Record<string, unknown> }
    if ('positional' in record || 'named' in record) {
      const parts = (record.positional ?? []).map((item) => short(item))
      for (const [name, item] of Object.entries(record.named ?? {})) parts.push(`${name} ${short(item)}`)
      return parts.join(', ')
    }
    if (Object.keys(record).length === 0) return ''
  }
  return short(value)
}

function short(value: unknown): string {
  const handle = value as { $jev?: string; capability?: string; id?: string } | null
  if (handle && typeof handle === 'object' && handle.$jev === 'handle') return `${handle.capability}#${handle.id}`
  const text = JSON.stringify(value)
  return text.length > 60 ? `${text.slice(0, 57)}…"` : text
}

function fixed(value: unknown): string {
  return typeof value === 'number' ? value.toFixed(2) : ''
}

/** One entry of Jev's menu at a machine step. */
export interface MenuEntry {
  event: string
  probability: number
  /** `stay` has no target. */
  target: string | null
  /** The guard's source, when the event has one. Being on the menu means it passed. */
  guard: string | null
  risky: boolean
}

export interface GateCheck {
  label: string
  value: number
  threshold: number
  pass: boolean
}

/** Section 7.6's four verdicts, plus `stay` when Jev chose not to move. */
export type Verdict = 'proceed' | 'confirm' | 'escalate' | 'stop' | 'stay' | 'no_enabled_events'

/** One `machine_step` event, joined with the request Jev saw and the pause the gate raised. */
export interface MachineStepView {
  index: number
  seq: number
  machine: string
  from: string
  to: string
  chosen: string
  confidence: number
  menu: MenuEntry[]
  checks: GateCheck[]
  verdict: Verdict
  /** The host's answer when the gate asked, from the recorded `resume`. */
  answer: string | null
  /** `obs` from the request whose answer this step records; null if no request was sent. */
  observed: Record<string, unknown> | null
  requestId: string | null
}

/**
 * The machine timeline. `machine_step` is recorded after the gate verdict is
 * resolved (spec section 7.8), so a `confirm`, `escalate` or `stopped` pause
 * between the step's answers and its `machine_step` is the gate's verdict;
 * the observation is the `obs` of the request that step answered.
 */
export function machineSteps(events: readonly RecordingEvent[], ir: Ir | null): MachineStepView[] {
  const steps: MachineStepView[] = []
  let lastRequest: RecordingEvent | null = null
  let windowPauses: RecordingEvent[] = []
  let answer: string | null = null
  for (const event of events) {
    if (event.event === 'request') {
      lastRequest = event
      windowPauses = []
      answer = null
      continue
    }
    if (event.event === 'pause') {
      windowPauses.push(event)
      continue
    }
    if (event.event === 'resume') {
      const payload = event['payload'] as { answer?: string } | undefined
      if (payload?.answer !== undefined) answer = payload.answer
      continue
    }
    if (event.event !== 'machine_step') continue

    const machineName = String(event['machine'])
    const machine = findMachine(ir, machineName)
    const from = String(event['state'])
    const chosen = String(event['chosen'])
    const probabilities = (event['probabilities'] as Record<string, number>) ?? {}
    const enabled = (event['enabled'] as string[]) ?? []
    const noDecision = Object.keys(probabilities).length === 0
    const transitions = machine?.states.find((state) => state.name === from)?.transitions ?? []
    const menu: MenuEntry[] = enabled
      .map((name) => {
        const transition = transitions.find((candidate) => candidate.event === name)
        return {
          event: name,
          probability: probabilities[name] ?? 0,
          target: transition?.target ?? null,
          guard: transition?.when ? printExpr(transition.when) : null,
          risky: transition?.risky ?? false,
        }
      })
      .sort((a, b) => b.probability - a.probability)
    const confidence = Number(event['confidence'] ?? 0)
    const chosenRisky = transitions.find((candidate) => candidate.event === chosen)?.risky ?? false
    const checks = noDecision ? [] : gateChecks(machine?.thresholds ?? {}, confidence, chosenRisky ? 1 : 0)
    const gatePause = windowPauses.find((pause) => ['confirm', 'escalate'].includes(String(pause['kind'])))
    let verdict: Verdict
    if (noDecision) verdict = 'no_enabled_events'
    else if (gatePause) verdict = String(gatePause['kind']) as Verdict
    else if (chosen === 'stay') verdict = 'stay'
    else verdict = event['to'] === from && transitions.find((t) => t.event === chosen)?.target !== from ? 'stop' : 'proceed'
    const request = noDecision ? null : lastRequest
    const state = request?.['state'] as { obs?: Record<string, unknown> } | undefined
    steps.push({
      index: steps.length + 1,
      seq: event.seq,
      machine: machineName,
      from,
      to: String(event['to']),
      chosen,
      confidence,
      menu,
      checks,
      verdict,
      answer: gatePause?.['kind'] === 'confirm' ? answer : null,
      observed: state?.obs ?? null,
      requestId: request ? String(request['request_id']) : null,
    })
    windowPauses = []
    answer = null
  }
  return steps
}

/** Section 7.6: the comparisons a gate makes, in the order it makes them. */
export function gateChecks(thresholds: Thresholds, confidence: number, risk: number): GateCheck[] {
  const checks: GateCheck[] = []
  if (thresholds.stop_confidence !== undefined) {
    checks.push({
      label: 'confidence ≥ stop_confidence',
      value: confidence,
      threshold: thresholds.stop_confidence,
      pass: confidence >= thresholds.stop_confidence,
    })
  }
  if (thresholds.risk_confirm !== undefined) {
    checks.push({ label: 'risk < risk_confirm', value: risk, threshold: thresholds.risk_confirm, pass: risk < thresholds.risk_confirm })
  }
  if (thresholds.min_confidence !== undefined) {
    checks.push({
      label: 'confidence ≥ min_confidence',
      value: confidence,
      threshold: thresholds.min_confidence,
      pass: confidence >= thresholds.min_confidence,
    })
  }
  return checks
}

function findMachine(ir: Ir | null, name: string): IrMachine | undefined {
  return ir?.machines.find((machine) => machine.name === name || name.endsWith(`.${machine.name}`))
}

/** The terminal pause of a run, from its `end` event. */
export function endOf(events: readonly RecordingEvent[]): RecordingEvent | undefined {
  return events.find((event) => event.event === 'end')
}

/** The latest answered request, for the Chat run panel. */
export function latestAnswered(events: readonly RecordingEvent[]): RequestView | undefined {
  return requests(events)
    .filter((view) => view.answers.length > 0)
    .at(-1)
}

/** Parse a JSONL recording. Blank lines are skipped. */
export function parseRecording(text: string): RecordingEvent[] {
  return text
    .split('\n')
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line) as RecordingEvent)
}
