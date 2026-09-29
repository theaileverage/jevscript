import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { beforeAll, describe, expect, it } from 'vitest'

import { applyEdits, EditError, lineDiff, newWarnings, nextPinNumber, type Pin, revertEdits } from '../shared/annotator.ts'
import type { Diagnostic } from '../shared/protocol.ts'
import { annotate, checkEdit, type Model } from '../server/claude.ts'
import { paths, REPO_ROOT } from '../server/env.ts'
import { IdeaStore, newIdea } from '../server/ideas.ts'
import { Jevscrypt } from '../server/jevscrypt.ts'

let source = ''
let jev: Jevscrypt

/** The addendum's worked example: `stuck` goes to `waiting_on_me` and stops sending. */
const STUCK_TO_WAITING = [
  {
    find: 'on stuck "the agent is looping or unsure how to proceed" -> nudging:\n      dev.send "Stop. In three lines, what is blocking you?"\n',
    replace: 'on stuck "the agent is looping or unsure how to proceed" -> waiting_on_me\n',
  },
]

beforeAll(async () => {
  source = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')
  jev = new Jevscrypt(paths().bin, await mkdtemp(join(tmpdir(), 'jevs-annotator-')))
})

const warning = (code: string, message: string, line: number): Diagnostic => ({
  file: 'p.jev',
  line,
  column: 4,
  severity: 'warning',
  code,
  message,
})

describe('source edits', () => {
  it('apply and roll back to the exact original', () => {
    const patched = applyEdits(source, STUCK_TO_WAITING)
    expect(patched).not.toContain('Stop. In three lines')
    expect(patched).toContain('-> waiting_on_me\n\n  state nudging:')
    expect(revertEdits(patched, STUCK_TO_WAITING)).toBe(source)
  })

  it('refuse text that is missing or ambiguous instead of guessing', () => {
    expect(() => applyEdits(source, [{ find: 'no such text', replace: 'x' }])).toThrow(EditError)
    expect(() => applyEdits(source, [{ find: 'dev.send', replace: 'x' }])).toThrow(/more than once/)
    const patched = applyEdits(source, STUCK_TO_WAITING)
    const editedSince = patched.replace(STUCK_TO_WAITING[0]!.replace, '')
    expect(() => revertEdits(editedSince, STUCK_TO_WAITING)).toThrow(EditError)
  })

  it('shows a line diff of just the changed region', () => {
    expect(lineDiff(source, applyEdits(source, STUCK_TO_WAITING))).toEqual([
      { op: ' ', text: '      dev.send "Tests fail:\\n{obs.tests}"' },
      { op: '-', text: '    on stuck "the agent is looping or unsure how to proceed" -> nudging:' },
      { op: '-', text: '      dev.send "Stop. In three lines, what is blocking you?"' },
      { op: '+', text: '    on stuck "the agent is looping or unsure how to proceed" -> waiting_on_me' },
      { op: ' ', text: '    on asks "the agent asks a question only a person can answer" -> waiting_on_me' },
    ])
  })
})

describe('checking an edit against the source it patches', () => {
  it('reports only warnings the edit introduced, whatever line they moved to', () => {
    const before = [warning('uncapped_field', 'field `status` has no `max`', 15)]
    const after = [
      warning('uncapped_field', 'field `status` has no `max`', 13),
      warning('machine_unreachable_done', 'state `x` cannot reach a done state', 20),
    ]
    expect(newWarnings(before, after).map((d) => d.code)).toEqual(['machine_unreachable_done'])
    expect(newWarnings(before, [...after, warning('uncapped_field', 'field `status` has no `max`', 30)]).map((d) => d.code)).toEqual([
      'machine_unreachable_done',
      'uncapped_field',
    ])
  })

  it('passes the worked example with no errors, no new warnings, and names the reachability change', async () => {
    const current = await jev.compile('review_loop.jev', source)
    expect(current.diagnostics.map((d) => d.code)).toEqual(['uncapped_field'])
    const idea = newIdea({ source, fileName: 'review_loop.jev' })
    const reply = await checkEdit(idea, 'Ask me instead of nudging.', STUCK_TO_WAITING, { compile: (f, s) => jev.compile(f, s), current })
    expect(reply.kind).toBe('edit')
    if (reply.kind !== 'edit') return
    expect(reply.refused).toBeNull()
    expect(reply.check?.errors).toEqual([])
    expect(reply.check?.newWarnings).toEqual([])
    expect(reply.check?.reachability).toEqual(['nudging is now reached by claims_done.', 'waiting_on_me is now reached by asks and stuck.'])
  })

  it('carries compile errors from a broken edit', async () => {
    const current = await jev.compile('review_loop.jev', source)
    const idea = newIdea({ source, fileName: 'review_loop.jev' })
    const reply = await checkEdit(idea, 'broken', [{ find: '-> approved when', replace: '-> nowhere when' }], {
      compile: (f, s) => jev.compile(f, s),
      current,
    })
    expect(reply.kind === 'edit' && reply.check?.errors.map((d) => d.code)).toEqual(['machine_unknown_state'])
  })

  it('grounds the model in the source and the compiled facts, and checks the edit it returns', async () => {
    const current = await jev.compile('review_loop.jev', source)
    const idea = newIdea({ source, fileName: 'review_loop.jev' })
    let prompt = ''
    const model: Model = {
      name: 'fake',
      async complete(_system, messages) {
        prompt = (messages[0] as { content: string }).content
        return { content: [], text: JSON.stringify({ kind: 'edit', text: 'Ask me instead.', edits: STUCK_TO_WAITING }) }
      },
    }
    const pin = await annotate(idea, { kind: 'edge', machine: 'review', from: 'working', event: 'stuck' }, 'When it is stuck, ask me instead of nudging.', {
      model,
      compile: (f, s) => jev.compile(f, s),
      spec: 'SPEC',
      current,
    })
    expect(prompt).toContain('"guard": "tree.tests_pass"')
    expect(prompt).toContain('"risky": true')
    expect(prompt).toContain(' 21      on stuck')
    expect(pin.number).toBe(1)
    expect(pin.reply?.kind === 'edit' && pin.reply.check?.errors).toEqual([])
  })
})

describe('pins', () => {
  it('persist with their idea across a save and a fresh store', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-pins-'))
    const pin: Pin = {
      id: 'p1',
      number: 1,
      target: { kind: 'edge', machine: 'review', from: 'working', event: 'stuck' },
      query: 'When it is stuck, ask me instead of nudging.',
      reply: { kind: 'answer', text: 'ok' },
      status: 'open',
      createdAt: '2026-09-29T00:00:00.000Z',
    }
    const idea = await new IdeaStore(home).save(newIdea({ title: 'Review', source, pins: [pin] }))
    const reloaded = await new IdeaStore(home).get(idea.id)
    expect(reloaded?.pins).toEqual([pin])
    expect((await new IdeaStore(home).list()).map((saved) => saved.pins)).toEqual([[pin]])
  })

  it('number per machine, and whole-machine questions take no number', () => {
    const pins = [
      { number: 1, target: { kind: 'state', machine: 'review', state: 'working' } },
      { number: 2, target: { kind: 'edge', machine: 'review', from: 'working', event: 'stuck' } },
      { number: null, target: { kind: 'machine', machine: 'review' } },
      { number: 1, target: { kind: 'state', machine: 'other', state: 'a' } },
    ] as Pin[]
    expect(nextPinNumber(pins, 'review')).toBe(3)
    expect(nextPinNumber(pins, 'fresh')).toBe(1)
  })
})
