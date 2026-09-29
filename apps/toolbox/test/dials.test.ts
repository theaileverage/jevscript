import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { findDials, formatDial, setDial } from '../shared/dials.ts'
import { REPO_ROOT } from '../server/env.ts'

const review = readFileSync(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')

describe('dials over declared budgets and thresholds (spec 7.1, 7.6)', () => {
  it('finds exactly what the machine header declares', () => {
    expect(findDials(review).map((dial) => [dial.id, dial.value, dial.line])).toEqual([
      ['machine:review:budget:calls', 30, 9],
      ['machine:review:thresholds:min_confidence', 0.6, 9],
      ['machine:review:thresholds:risk_confirm', 0.5, 9],
    ])
  })

  it('offers no dial for a clause the program does not declare', () => {
    expect(findDials('task main:\n  x = 1\n')).toEqual([])
  })

  it('rewrites only the literal it points at', () => {
    const [, confidence] = findDials(review)
    const rewritten = setDial(review, confidence!, 0.75)
    expect(rewritten.split('\n')[8]).toBe(
      'machine review(dev) budget calls 30 thresholds min_confidence 0.75, risk_confirm 0.5:',
    )
    expect(rewritten.split('\n').filter((_, index) => index !== 8)).toEqual(review.split('\n').filter((_, index) => index !== 8))
  })

  it('re-finds dials at their new ranges after a rewrite that changes length', () => {
    const calls = findDials(review)[0]!
    const rewritten = setDial(review, calls, 120)
    const again = findDials(rewritten)
    expect(again.map((dial) => dial.value)).toEqual([120, 0.6, 0.5])
    expect(setDial(rewritten, again[2]!, 0.35)).toContain('min_confidence 0.6, risk_confirm 0.35:')
  })

  it('reads task headers with several budget keys, k-suffixed numbers and a trailing comment', () => {
    const source = 'task main budget calls 12, usd 0.5, steps 2k thresholds done 0.9:  # tuned\n  return\n'
    expect(findDials(source).map((dial) => [dial.key, dial.value])).toEqual([
      ['calls', 12],
      ['usd', 0.5],
      ['steps', 2000],
      ['done', 0.9],
    ])
  })

  it('writes thresholds to two places and counts whole', () => {
    expect(formatDial({ clause: 'thresholds', key: 'min_confidence' }, 0.6000001)).toBe('0.6')
    expect(formatDial({ clause: 'thresholds', key: 'risk_confirm' }, 1)).toBe('1')
    expect(formatDial({ clause: 'budget', key: 'calls' }, 29.6)).toBe('30')
    expect(formatDial({ clause: 'budget', key: 'usd' }, 0.125)).toBe('0.13')
  })
})
