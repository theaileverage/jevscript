/**
 * The pause stack (spec section 10.2): the oldest pause waiting on you in
 * front, the others peeking out behind. Answering the front card resumes its
 * run; the answer is recorded, so a replay lands in the same place.
 */
import { cardChoices, type PauseCard } from '../../shared/pauses.ts'
import { actions, useStore } from '../store.ts'

function heading(card: PauseCard): { title: string; body: string } {
  const pause = card.pause
  switch (pause.kind) {
    case 'confirm':
      return { title: pause.message || 'Confirm', body: `${card.source} asks. Your answer is recorded, so a replay lands in the same place.` }
    case 'escalate':
      return { title: 'Handed to you', body: `${pause.reason ?? ''} · ${card.source}. Resume to continue, or end the run.` }
    case 'budget':
      return { title: `Budget: ${pause.key ?? ''}`, body: `${card.source} used ${String(pause.used)} of ${String(pause.limit)} ${pause.key ?? ''} in ${pause.task}.` }
    case 'error':
      return { title: pause.code ?? 'Error', body: pause.message ?? '' }
    default:
      return { title: pause.kind, body: '' }
  }
}

export function PauseStack() {
  const cards = useStore((s) => s.stack.cards)
  if (cards.length === 0) return null
  const front = cards[0] as PauseCard
  const { title, body } = heading(front)
  const peeks = Math.min(cards.length - 1, 2)
  return (
    <section className="stack">
      <h5 className="amber">
        <span>
          {cards.length} {cards.length === 1 ? 'pause' : 'pauses'} waiting
        </span>
      </h5>
      <div className="card">
        <h6>{title}</h6>
        {body ? <p>{body}</p> : null}
        <div className="choices">
          {cardChoices(front.pause).map((choice, index) => (
            <button
              key={choice.label}
              className={`btn ${index === 0 ? 'first' : ''}`}
              onClick={() => ('abort' in choice ? void actions.abort(front.runId) : void actions.resume(front.runId, choice.resume))}
            >
              {choice.label}
            </button>
          ))}
        </div>
      </div>
      {Array.from({ length: peeks }, (_, index) => (
        <div key={index} className="card-peek" style={{ bottom: 20 - (index + 1) * 8, left: 10 + index * 10, right: 10 + index * 10, zIndex: 2 - index }} />
      ))}
    </section>
  )
}
