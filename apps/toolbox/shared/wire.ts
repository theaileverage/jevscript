/**
 * The TypeSafe wire mapping, ported line for line from `wire` in
 * `crates/jevscrypt-runtime/src/jev.rs` so the Requests tab can resend a
 * recorded request exactly as `HttpJevClient` would have sent it. The JSON-RPC
 * surface has no raw-request method (spec section 11.5), and `judgment.run`
 * rebuilds its questions from current source, so every resend goes through
 * this port; SPEC-GAPS.md says so. `test/resend.test.ts` checks it byte for
 * byte against the body the real runtime posts.
 */
import type { JevAnswer } from './recording.ts'

/** Section 6.8's detail block, as the runtime records it. */
export interface Instruction {
  focus?: string
  note?: string
  compare?: string[]
  yes?: string[]
  no?: string[]
}

export interface ChoiceLabel {
  name: string
  description?: string
  what?: string
  not_for?: string
  examples?: string[]
}

/** The runtime's `Question`, as a `request` event records it. */
export type Question =
  | { type: 'noul'; id: string; path: string; condition: string; instruction?: Instruction }
  | { type: 'choice'; id: string; path: string; question?: string; labels: ChoiceLabel[]; instruction?: Instruction }
  | { type: 'score'; id: string; path: string; levels: { name?: string; situation: string }[]; instruction?: Instruction }

/** What a `request` event records: exactly what was sent, minus the model. */
export interface JevRequest {
  state: Record<string, unknown>
  questions: Question[]
}

/** The description the bare `other` / `none` label is sent with (spec section 6.3). */
export const ESCAPE_DESCRIPTION = 'none of the other options fits'

/**
 * `wire::request_body`. The runtime builds it from serde_json maps, which are
 * ordered by key, so every object here is emitted in key order too; together
 * with `JSON.stringify` that gives the same bytes the runtime posts.
 */
export function requestBody(request: JevRequest, model: string): Record<string, unknown> {
  const questions = Object.fromEntries(request.questions.map((question) => [question.id, questionBody(question)]))
  return sortDeep({ state: request.state, model, questions }) as Record<string, unknown>
}

function questionBody(question: Question): Record<string, unknown> {
  switch (question.type) {
    case 'noul': {
      const body: Record<string, unknown> = {
        type: 'noul',
        instructions: instructions(`Is it the case that \`${question.path}\` ${question.condition}?`, question.path, question.instruction),
      }
      const yes = question.instruction?.yes ?? []
      const no = question.instruction?.no ?? []
      if (yes.length > 0 || no.length > 0) {
        const criteria: Record<string, unknown> = {}
        if (no.length > 0) criteria['false'] = { examples: no }
        if (yes.length > 0) criteria['true'] = { examples: yes }
        body['criteria'] = criteria
      }
      return body
    }
    case 'choice':
      return {
        type: 'choice',
        instructions: instructions(question.question ?? `Which option describes \`${question.path}\`?`, question.path, question.instruction),
        criteria: Object.fromEntries(question.labels.map((label) => [label.name, criterion(label)])),
      }
    case 'score':
      return {
        type: 'score',
        instructions: instructions(`Which level describes \`${question.path}\`?`, question.path, question.instruction),
        criteria: question.levels.map((level) => level.situation),
      }
  }
}

function instructions(text: string, path: string, detail: Instruction | undefined): Record<string, unknown> {
  const map: Record<string, unknown> = { question: text, inspect: path }
  if (detail?.focus !== undefined) map['focus'] = detail.focus
  if (detail?.note !== undefined) map['note'] = detail.note
  if (detail?.compare && detail.compare.length > 0) map['compare'] = detail.compare
  return map
}

function criterion(label: ChoiceLabel): unknown {
  if (label.what === undefined && label.not_for === undefined && (label.examples ?? []).length === 0) {
    return label.description ?? ESCAPE_DESCRIPTION
  }
  const map: Record<string, unknown> = {}
  const what = label.what ?? label.description
  if (what !== undefined) map['what'] = what
  if (label.not_for !== undefined) map['not_for'] = label.not_for
  if ((label.examples ?? []).length > 0) map['examples'] = label.examples
  return map
}

/** Every object's keys in code-unit order, as serde_json's `BTreeMap`-backed map writes them. */
export function sortDeep(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortDeep)
  if (value === null || typeof value !== 'object') return value
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .sort(byKey)
      .map(([key, inner]) => [key, sortDeep(inner)]),
  )
}

const byKey = ([a]: [string, unknown], [b]: [string, unknown]) => (a < b ? -1 : a > b ? 1 : 0)

/**
 * `wire::parse_response`: one answer per question, keyed back to our ids, in
 * question order. Malformed answers fail with the runtime's own messages. A
 * JSON number is finite in serde_json, which rejects an out-of-range literal,
 * so a number here must be finite too.
 */
export function parseResponse(body: unknown, request: JevRequest): JevAnswer[] {
  const answers = isObject(body) ? body['answers'] : undefined
  if (!isObject(answers)) throw new Error('the response has no `answers` object')
  return request.questions.map((question) => {
    if (!Object.hasOwn(answers, question.id)) throw new Error(`the response has no answer for \`${question.id}\``)
    const answer = answers[question.id]
    const field = (key: string) => (isObject(answer) ? answer[key] : undefined)
    const number = (key: string) => {
      const value = field(key)
      if (!isNumber(value)) throw new Error(`answer \`${question.id}\` has no numeric \`${key}\``)
      return value
    }
    const probabilities = () => {
      const value = field('probabilities')
      if (!isObject(value)) throw new Error(`answer \`${question.id}\` has no \`probabilities\``)
      // In key order, so the first bad probability named is the one the runtime names.
      return Object.fromEntries(
        Object.entries(value)
          .sort(byKey)
          .map(([key, p]) => {
            if (!isNumber(p)) throw new Error(`answer \`${question.id}\` probability \`${key}\` is not a number`)
            return [key, p]
          }),
      )
    }
    const confidence = (distribution: Record<string, number>) => {
      const value = field('confidence')
      return isNumber(value) ? value : confidenceOf(distribution)
    }
    switch (question.type) {
      case 'noul':
        return { type: 'noul', id: question.id, prob: number('noul') }
      case 'choice': {
        const distribution = probabilities()
        const label = field('choice')
        if (typeof label !== 'string') throw new Error(`answer \`${question.id}\` has no \`choice\``)
        return { type: 'choice', id: question.id, label, confidence: confidence(distribution), probabilities: distribution }
      }
      case 'score': {
        const score = number('score')
        const names = question.levels.map((level) => level.name)
        const named = names.length > 0 && names.every((name) => name !== undefined)
        const distribution = Object.fromEntries(
          Object.entries(probabilities()).map(([key, p]) => {
            const index = /^\d+$/.test(key) ? Number(key) : Number.NaN
            return [named && names[index] !== undefined ? (names[index] as string) : key, p]
          }),
        )
        return {
          type: 'score',
          id: question.id,
          level: Math.min(Math.max(Math.round(score), 0), Math.max(question.levels.length - 1, 0)),
          score,
          confidence: confidence(distribution),
          probabilities: distribution,
        }
      }
    }
  })
}

/** serde_json's `as_object`: a JSON object, not an array or null. */
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** serde_json's `as_f64` on a parsed number. */
function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

/** `wire::confidence_of`: `(n * peak - 1) / (n - 1)`, clamped. */
export function confidenceOf(probabilities: Record<string, number>): number {
  const values = Object.values(probabilities)
  if (values.length < 2) return values.length === 1 ? 1 : 0
  const peak = Math.max(0, ...values)
  return Math.min(1, Math.max(0, (values.length * peak - 1) / (values.length - 1)))
}

/** `wire::error_message`: the usual message keys of an error body, else the raw body. */
export function errorMessage(status: number, body: string): string {
  let message: string | undefined
  try {
    const json = JSON.parse(body) as Record<string, unknown>
    for (const key of ['message', 'detail', 'error']) {
      const value = json[key]
      if (typeof value === 'string') {
        message = value
        break
      }
      if (value && typeof value === 'object' && !Array.isArray(value)) {
        const inner = (value as { message?: unknown }).message
        if (typeof inner === 'string') {
          message = inner
          break
        }
        continue
      }
      if (value !== undefined && value !== null) {
        message = JSON.stringify(value)
        break
      }
    }
  } catch {
    // Not JSON; fall back to the raw body.
  }
  return `HTTP ${status}: ${message ?? body.trim()}`
}
