/**
 * The pause stack (spec section 10.2).
 *
 * A run has at most one open pause, but the toolbox can hold many runs, so the
 * stack is keyed by run: the oldest pause waiting on the host is the front
 * card, the rest peek out behind it. Answering the front card resumes its run.
 * Only pauses the host has to answer become cards; `waiting` auto-resumes and
 * `done`/`stopped` end the run.
 */

export type PauseKind = 'confirm' | 'escalate' | 'waiting' | 'budget' | 'error' | 'stopped' | 'done'

/** The wire form of a pause; kind-specific fields as section 10.2 lists them. */
export interface Pause {
  kind: PauseKind
  run_id: string
  step: number
  task: string
  state?: string
  message?: string
  options?: string[]
  reason?: string
  key?: 'calls' | 'minutes' | 'usd' | 'steps'
  used?: number
  limit?: number
  code?: string
  retryable?: boolean
  on?: string
  condition?: string
  outputs?: Record<string, unknown>
  verified?: boolean
  usage?: { calls: number; tokens: number; usd: number; minutes: number; steps: number }
  context?: Record<string, unknown>
}

/** What the host sends back (spec section 10.2). */
export type Resume =
  | { answer: string; text?: string }
  | { resume: true }
  | { extend: Partial<Record<'calls' | 'minutes' | 'usd' | 'steps', number>> }
  | { retry: true }

export interface PauseCard {
  runId: string
  /** Which idea's program raised it, for the card's heading. */
  source: string
  pause: Pause
}

export interface PauseStack {
  cards: PauseCard[]
}

export type PauseAction =
  | { type: 'paused'; runId: string; source: string; pause: Pause }
  | { type: 'answered'; runId: string }
  | { type: 'ended'; runId: string }

export const emptyStack: PauseStack = { cards: [] }

/** Whether the host must answer this pause before the run can go on. */
export function needsHost(pause: Pause): boolean {
  switch (pause.kind) {
    case 'confirm':
    case 'escalate':
    case 'budget':
      return true
    case 'error':
      return pause.retryable === true
    default:
      return false
  }
}

export function pauseReducer(stack: PauseStack, action: PauseAction): PauseStack {
  const others = stack.cards.filter((card) => card.runId !== action.runId)
  if (action.type !== 'paused' || !needsHost(action.pause)) {
    return others.length === stack.cards.length ? stack : { cards: others }
  }
  const existing = stack.cards.findIndex((card) => card.runId === action.runId)
  const card: PauseCard = { runId: action.runId, source: action.source, pause: action.pause }
  if (existing === -1) return { cards: [...stack.cards, card] }
  const cards = [...stack.cards]
  cards[existing] = card
  return { cards }
}

/** One button on a pause card. `abort` ends the run instead of resuming it. */
export type CardChoice =
  | { label: string; primary: boolean; resume: Resume }
  | { label: string; primary: boolean; abort: true }

/**
 * The answers a card offers, taken from the pause itself: a `confirm` offers
 * exactly its declared options, a `budget` offers to double the limit that was
 * hit, and the rest offer the one resume their kind accepts.
 */
export function cardChoices(pause: Pause): CardChoice[] {
  switch (pause.kind) {
    case 'confirm':
      return (pause.options ?? ['yes', 'no']).map((option, index) => ({
        label: option,
        primary: index === 0,
        resume: { answer: option },
      }))
    case 'escalate':
      return [
        { label: 'Resume', primary: true, resume: { resume: true } },
        { label: 'End run', primary: false, abort: true },
      ]
    case 'budget': {
      const key = pause.key ?? 'calls'
      const limit = Math.max(1, (pause.limit ?? 0) * 2)
      return [
        { label: `Raise ${key} to ${limit}`, primary: true, resume: { extend: { [key]: limit } } },
        { label: 'Stop', primary: false, abort: true },
      ]
    }
    case 'error':
      return pause.retryable
        ? [
            { label: 'Retry', primary: true, resume: { retry: true } },
            { label: 'Abort', primary: false, abort: true },
          ]
        : []
    default:
      return []
  }
}
