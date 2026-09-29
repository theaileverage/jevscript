import { describe, expect, it } from 'vitest'

import { cardChoices, emptyStack, type Pause, pauseReducer } from '../shared/pauses.ts'

const pause = (runId: string, kind: Pause['kind'], extra: Partial<Pause> = {}): Pause => ({
  kind,
  run_id: runId,
  step: 1,
  task: 'main',
  ...extra,
})

describe('the pause stack (spec 10.2)', () => {
  it('stacks answerable pauses oldest first, one per run', () => {
    let stack = pauseReducer(emptyStack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'confirm') })
    stack = pauseReducer(stack, { type: 'paused', runId: 'b', source: 'B', pause: pause('b', 'budget') })
    stack = pauseReducer(stack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'escalate') })
    expect(stack.cards.map((card) => [card.runId, card.pause.kind])).toEqual([
      ['a', 'escalate'],
      ['b', 'budget'],
    ])
  })

  it('drops a card when it is answered or its run ends', () => {
    let stack = pauseReducer(emptyStack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'confirm') })
    stack = pauseReducer(stack, { type: 'paused', runId: 'b', source: 'B', pause: pause('b', 'confirm') })
    stack = pauseReducer(stack, { type: 'answered', runId: 'a' })
    expect(stack.cards.map((card) => card.runId)).toEqual(['b'])
    expect(pauseReducer(stack, { type: 'ended', runId: 'b' }).cards).toEqual([])
  })

  it('never makes a card of a pause the host does not answer', () => {
    for (const kind of ['waiting', 'done', 'stopped'] as const) {
      expect(pauseReducer(emptyStack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', kind) })).toBe(emptyStack)
    }
    expect(pauseReducer(emptyStack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'error', { retryable: false }) }).cards).toEqual([])
    expect(pauseReducer(emptyStack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'error', { retryable: true }) }).cards).toHaveLength(1)
  })

  it('a later non-answerable pause of the same run clears its card', () => {
    const stack = pauseReducer(emptyStack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'confirm') })
    expect(pauseReducer(stack, { type: 'paused', runId: 'a', source: 'A', pause: pause('a', 'waiting') }).cards).toEqual([])
  })

  it('offers exactly a confirm pause’s declared options, and the resume each kind accepts', () => {
    expect(cardChoices(pause('a', 'confirm', { options: ['today', 'week', 'ignore'] }))).toEqual([
      { label: 'today', primary: true, resume: { answer: 'today' } },
      { label: 'week', primary: false, resume: { answer: 'week' } },
      { label: 'ignore', primary: false, resume: { answer: 'ignore' } },
    ])
    expect(cardChoices(pause('a', 'confirm'))[0]).toMatchObject({ resume: { answer: 'yes' } })
    expect(cardChoices(pause('a', 'budget', { key: 'calls', used: 30, limit: 30 }))[0]).toMatchObject({ resume: { extend: { calls: 60 } } })
    expect(cardChoices(pause('a', 'escalate'))[0]).toMatchObject({ resume: { resume: true } })
    expect(cardChoices(pause('a', 'error', { retryable: true }))[0]).toMatchObject({ resume: { retry: true } })
  })
})
