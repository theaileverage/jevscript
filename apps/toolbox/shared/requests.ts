/**
 * The Requests tab: every Jev request of a run, from its recording's
 * `request` and `answers` events, with where it came from, and what the user
 * changed when editing one to resend.
 */
import type { Ir } from './ir.ts'
import type { JevAnswer, RecordingEvent } from './recording.ts'
import { machineSteps, requests } from './recording.ts'
import type { JevRequest, Question } from './wire.ts'

/**
 * Where a request came from. A machine step asks one Choice with id `event`
 * over `{state, goal, obs, recent}` (spec section 7.8). A named judgment's
 * state is exactly its declared parameters and its question ids are its
 * result names (section 6.9). Anything else is inline.
 */
export type RequestOrigin =
  | { kind: 'machine'; machine: string | null; state: string }
  | { kind: 'judgment'; name: string }
  | { kind: 'inline' }

export interface RequestEntry {
  index: number
  requestId: string
  seq: number
  /** A machine request's step number; otherwise the trail step it followed (spec section 7.5). */
  step: number
  origin: RequestOrigin
  request: JevRequest
  answers: JevAnswer[]
  latencyMs: number | null
  error: string | null
}

export function requestEntries(events: readonly RecordingEvent[], ir: Ir | null): RequestEntry[] {
  const views = requests(events)
  const trailSteps = new Map<number, number>()
  const machineStepNumbers = new Map<number, number>()
  let step = 0
  let machineStepCount = 0
  for (const event of events) {
    if (event.event === 'step') step = Number((event['record'] as { step?: number })?.step ?? step)
    if (event.event === 'machine_step') machineStepCount += 1
    if (event.event === 'request') {
      trailSteps.set(event.seq, step)
      machineStepNumbers.set(event.seq, machineStepCount + 1)
    }
  }
  const owners = new Map(machineSteps(events, ir).filter((step) => step.requestId !== null).map((step) => [step.requestId, step]))
  return views.map((view, index) => {
    const request: JevRequest = {
      state: (view.state as Record<string, unknown>) ?? {},
      questions: (events.find((event) => event.seq === view.seq)?.['questions'] as Question[]) ?? [],
    }
    const owner = owners.get(view.requestId)
    const origin = owner ? { kind: 'machine' as const, machine: owner.machine, state: owner.from } : originOf(request, ir)
    return {
      index: index + 1,
      requestId: view.requestId,
      seq: view.seq,
      step: (origin.kind === 'machine' ? machineStepNumbers : trailSteps).get(view.seq) ?? 0,
      origin,
      request,
      answers: view.answers,
      latencyMs: view.latencyMs,
      error: view.error,
    }
  })
}

function originOf(request: JevRequest, ir: Ir | null): RequestOrigin {
  const state = request.state
  const onlyEvent = request.questions.length === 1 && request.questions[0]?.id === 'event'
  if (onlyEvent && typeof state['state'] === 'string' && 'goal' in state && 'obs' in state) {
    return { kind: 'machine', machine: null, state: state['state'] }
  }
  const stateKeys = Object.keys(state).sort().join()
  for (const judgment of (ir?.judgments ?? []) as { name: string; params: string[]; results?: { name: string }[] }[]) {
    const results = new Set((judgment.results ?? []).map((result) => result.name))
    const ids = request.questions.map((question) => question.id.replace(/\[.*$/, ''))
    if ([...judgment.params].sort().join() === stateKeys && ids.length > 0 && ids.every((id) => results.has(id))) {
      return { kind: 'judgment', name: judgment.name }
    }
  }
  return { kind: 'inline' }
}

export function describeOrigin(origin: RequestOrigin): string {
  switch (origin.kind) {
    case 'machine':
      return origin.state
    case 'judgment':
      return origin.name
    case 'inline':
      return 'inline'
  }
}

/**
 * The dotted paths where `edited` differs from `recorded`, leaf-most. A path
 * present on one side only counts as edited.
 */
export function editedPaths(recorded: unknown, edited: unknown, prefix = ''): string[] {
  if (isRecord(recorded) && isRecord(edited)) {
    const keys = [...new Set([...Object.keys(recorded), ...Object.keys(edited)])]
    return keys.flatMap((key) => editedPaths(recorded[key], edited[key], prefix ? `${prefix}.${key}` : key))
  }
  if (Array.isArray(recorded) && Array.isArray(edited)) {
    const length = Math.max(recorded.length, edited.length)
    return Array.from({ length }, (_, index) => editedPaths(recorded[index], edited[index], `${prefix}[${index}]`)).flat()
  }
  return JSON.stringify(recorded) === JSON.stringify(edited) ? [] : [prefix || '(root)']
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** Parse the editor's text as a request, or say what is wrong with it. */
export function parseEditedRequest(text: string): { request: JevRequest } | { error: string } {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch (error) {
    return { error: `not JSON: ${error instanceof Error ? error.message : String(error)}` }
  }
  if (!isRecord(value) || !isRecord(value['state']) || !Array.isArray(value['questions'])) {
    return { error: 'a request is `{ "state": { ... }, "questions": [ ... ] }`' }
  }
  for (const [index, question] of value['questions'].entries()) {
    if (!isRecord(question) || !['noul', 'choice', 'score'].includes(String(question['type'])) || typeof question['id'] !== 'string') {
      return { error: `question ${index + 1} needs a \`type\` of noul, choice or score and a text \`id\`` }
    }
  }
  return { request: value as unknown as JevRequest }
}

export function requestText(request: JevRequest): string {
  return JSON.stringify(request, null, 2)
}

/** One resend of a request, kept with the idea. */
export interface ResendRecord {
  at: string
  recording: string
  requestId: string
  /** How it was sent. New resends are always `endpoint`; `judgment.run` remains for history saved by earlier builds. */
  via: 'judgment.run' | 'endpoint'
  edited: string[]
  request: JevRequest
  answers: JevAnswer[] | null
  error: string | null
  latencyMs: number
}

/** A one-line summary of an answer: label or level, and confidence. */
export function answerSummary(answer: JevAnswer): string {
  switch (answer.type) {
    case 'noul':
      return answer.prob.toFixed(2)
    case 'choice':
      return `${answer.label} ${answer.confidence.toFixed(2)}`
    case 'score':
      return `level ${answer.level} ${answer.confidence.toFixed(2)}`
  }
}
