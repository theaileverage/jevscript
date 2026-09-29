import { describe, expect, it } from 'vitest'

import { type JevRequest, parseResponse } from '../shared/wire.ts'

const choice: JevRequest = {
  state: { message: 'hi' },
  questions: [{ type: 'choice', id: 'urgency', path: 'message', labels: [{ name: 'today' }, { name: 'week' }] }],
}
const noul: JevRequest = { state: { message: 'hi' }, questions: [{ type: 'noul', id: 'billing', path: 'message', condition: 'is about billing' }] }
const score: JevRequest = {
  state: { draft: 'x' },
  questions: [{ type: 'score', id: 'quality', path: 'draft', levels: [{ name: 'poor', situation: 'bad' }, { name: 'good', situation: 'fine' }] }],
}

describe('parseResponse, as wire::parse_response', () => {
  it('reads each answer type and names score levels', () => {
    expect(parseResponse({ answers: { urgency: { choice: 'today', probabilities: { today: 0.9, week: 0.1 } } } }, choice)).toEqual([
      { type: 'choice', id: 'urgency', label: 'today', confidence: 0.8, probabilities: { today: 0.9, week: 0.1 } },
    ])
    expect(parseResponse({ answers: { billing: { noul: 0.25 } } }, noul)).toEqual([{ type: 'noul', id: 'billing', prob: 0.25 }])
    expect(parseResponse({ answers: { quality: { score: 0.6, confidence: 0.5, probabilities: { '0': 0.4, '1': 0.6 } } } }, score)).toEqual([
      { type: 'score', id: 'quality', level: 1, score: 0.6, confidence: 0.5, probabilities: { poor: 0.4, good: 0.6 } },
    ])
  })

  it('rejects a malformed response with the runtime’s message', () => {
    const cases: [unknown, JevRequest, string][] = [
      [null, choice, 'the response has no `answers` object'],
      [{ answers: [] }, choice, 'the response has no `answers` object'],
      [{ answers: {} }, choice, 'the response has no answer for `urgency`'],
      [{ answers: { urgency: { choice: 'today', probabilities: { today: 'bad', week: 0.2 } } } }, choice, 'answer `urgency` probability `today` is not a number'],
      [{ answers: { urgency: { choice: 'today', probabilities: { week: null, today: 'bad' } } } }, choice, 'answer `urgency` probability `today` is not a number'],
      [{ answers: { urgency: { choice: 'today', probabilities: [0.5, 0.5] } } }, choice, 'answer `urgency` has no `probabilities`'],
      [{ answers: { urgency: { choice: 1, probabilities: { today: 1 } } } }, choice, 'answer `urgency` has no `choice`'],
      [{ answers: { urgency: null } }, choice, 'answer `urgency` has no `probabilities`'],
      [{ answers: { billing: { noul: '0.5' } } }, noul, 'answer `billing` has no numeric `noul`'],
      [{ answers: { billing: { noul: Number.POSITIVE_INFINITY } } }, noul, 'answer `billing` has no numeric `noul`'],
      [{ answers: { quality: { score: Number.NaN, probabilities: {} } } }, score, 'answer `quality` has no numeric `score`'],
      [{ answers: { quality: { score: 1, probabilities: { '0': 0.1, '1': Number.NaN } } } }, score, 'answer `quality` probability `1` is not a number'],
    ]
    for (const [body, request, message] of cases) expect(() => parseResponse(body, request)).toThrow(message)
  })

  it('falls back to the distribution’s confidence when the response’s is not a number', () => {
    const [answer] = parseResponse({ answers: { urgency: { choice: 'today', confidence: 'high', probabilities: { today: 1, week: 0 } } } }, choice)
    expect(answer).toMatchObject({ confidence: 1 })
  })
})
