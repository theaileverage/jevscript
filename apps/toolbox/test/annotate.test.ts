/**
 * The Machines annotator through `/ws`: Claude (the real SDK client against a
 * local Messages API stand-in) answers at a pin or proposes an edit, and the
 * server checks every edit with the real compiler before the page can apply
 * it. Pins are saved with their idea.
 */
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'

import type { Pin, PinTarget } from '../shared/annotator.ts'
import type { Idea } from '../shared/protocol.ts'
import { AnthropicModel } from '../server/claude.ts'
import { REPO_ROOT } from '../server/env.ts'
import { newIdea } from '../shared/ideas.ts'
import { STUCK_TO_WAITING as STUCK_EDIT } from '../demo/services.ts'
import { fakeClaude, type FakeClaude, open } from './harness.ts'

const STUCK_TO_WAITING = [STUCK_EDIT]
const STUCK: PinTarget = { kind: 'edge', machine: 'review', from: 'working', event: 'stuck' }

let claude: FakeClaude
let source = ''
let session: Awaited<ReturnType<typeof open>>

beforeAll(async () => {
  source = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')
  claude = await fakeClaude()
  process.env['ANTHROPIC_BASE_URL'] = claude.baseUrl
  session = await open({ home: await mkdtemp(join(tmpdir(), 'jevs-annotate-')), model: new AnthropicModel('claude-opus-5-5', 'test-key') })
})

afterAll(async () => {
  await session.close()
  delete process.env['ANTHROPIC_BASE_URL']
  await claude.close()
})

afterEach(() => {
  claude.requests.length = 0
})

const review = (pins: Pin[] = []) => newIdea({ title: 'Review', source, fileName: 'review_loop.jev', pins })

async function annotate(idea: Idea, target: PinTarget, query: string, reply: unknown): Promise<Pin> {
  claude.replies.push(JSON.stringify(reply))
  return (await session.client.request<{ pin: Pin }>('annotate', { idea, target, query })).pin
}

describe('an edit proposed at a pin', () => {
  it('grounds the model in the numbered source and compiled facts, then checks the edit it returns', async () => {
    const pin = await annotate(review(), STUCK, 'When it is stuck, ask me instead of nudging.', {
      kind: 'edit',
      text: 'Ask me instead.',
      edits: STUCK_TO_WAITING,
    })
    const prompt = (claude.requests[0]!.messages[0] as { content: string }).content
    expect(prompt).toContain('the event `stuck` out of state `working`')
    expect(prompt).toContain('"guard": "tree.tests_pass"')
    expect(prompt).toContain('"risky": true')
    expect(prompt).toContain(' 21      on stuck')
    expect(pin.number).toBe(1)
    expect(pin.reply).toMatchObject({ kind: 'edit', refused: null, check: { errors: [], newWarnings: [] } })
    if (pin.reply?.kind !== 'edit') return
    expect(pin.reply.check?.reachability).toEqual(['nudging is now reached by claims_done.', 'waiting_on_me is now reached by asks and stuck.'])
    expect(pin.reply.diff).toEqual([
      { op: ' ', text: '      dev.send "Tests fail:\\n{obs.tests}"' },
      { op: '-', text: '    on stuck "the agent is looping or unsure how to proceed" -> nudging:' },
      { op: '-', text: '      dev.send "Stop. In three lines, what is blocking you?"' },
      { op: '+', text: '    on stuck "the agent is looping or unsure how to proceed" -> waiting_on_me' },
      { op: ' ', text: '    on asks "the agent asks a question only a person can answer" -> waiting_on_me' },
    ])
  })

  it('carries compile errors from a broken edit', async () => {
    const pin = await annotate(review(), STUCK, 'break it', {
      kind: 'edit',
      text: 'broken',
      edits: [{ find: '-> approved when', replace: '-> nowhere when' }],
    })
    expect(pin.reply?.kind === 'edit' && pin.reply.check?.errors.map((d) => d.code)).toEqual(['machine_unknown_state'])
  })

  it('reports only the warnings the edit introduced, whatever line the old ones moved to', async () => {
    const pin = await annotate(review(), { kind: 'machine', machine: 'review' }, 'Also watch the tail.', {
      kind: 'edit',
      text: 'Observe the tail too.',
      edits: [{ find: '  observe:\n    summary', replace: '  observe:\n    tail     dev.observe.tail\n    summary' }],
    })
    if (pin.reply?.kind !== 'edit') throw new Error(`expected an edit, got ${JSON.stringify(pin.reply)}`)
    expect(pin.reply.check?.errors).toEqual([])
    expect(pin.reply.check?.newWarnings.map((d) => [d.code, d.message])).toEqual([
      ['uncapped_field', 'field `tail` has no `max`; every field that reaches Jev should say how large it may be (spec section 7.2)'],
    ])
  })

  it('refuses text that is missing or ambiguous instead of guessing', async () => {
    const missing = await annotate(review(), STUCK, 'x', { kind: 'edit', text: 'x', edits: [{ find: 'no such text', replace: 'x' }] })
    const ambiguous = await annotate(review(), STUCK, 'x', { kind: 'edit', text: 'x', edits: [{ find: 'dev.send', replace: 'x' }] })
    expect(missing.reply).toMatchObject({ kind: 'edit', check: null })
    expect(missing.reply?.kind === 'edit' && missing.reply.refused).toBeTruthy()
    expect(ambiguous.reply?.kind === 'edit' && ambiguous.reply.refused).toMatch(/more than once/)
  })
})

describe('questions and pins', () => {
  it('answers a question without touching the source', async () => {
    const pin = await annotate(review(), { kind: 'state', machine: 'review', state: 'working' }, 'What proves finished?', {
      kind: 'answer',
      text: 'The guard tree.tests_pass.',
      edits: [],
    })
    expect(pin.reply).toEqual({ kind: 'answer', text: 'The guard tree.tests_pass.' })
  })

  it('numbers pins per machine; whole-machine questions take no number', async () => {
    const earlier = [
      { id: 'a', number: 1, target: { kind: 'state', machine: 'review', state: 'working' } },
      { id: 'b', number: 2, target: STUCK },
      { id: 'c', number: null, target: { kind: 'machine', machine: 'review' } },
    ] as Pin[]
    const answer = { kind: 'answer', text: 'ok', edits: [] }
    expect((await annotate(review(earlier), { kind: 'state', machine: 'review', state: 'nudging' }, 'q', answer)).number).toBe(3)
    expect((await annotate(review(earlier), { kind: 'machine', machine: 'review' }, 'q', answer)).number).toBeNull()
  })

  it('persist with their idea across a server restart', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-pins-'))
    const pin: Pin = {
      id: 'p1',
      number: 1,
      target: STUCK,
      query: 'When it is stuck, ask me instead of nudging.',
      reply: { kind: 'answer', text: 'ok' },
      status: 'open',
      createdAt: '2026-09-29T00:00:00.000Z',
    }
    const first = await open({ home })
    const { idea } = await first.client.request<{ idea: Idea }>('ideas.save', { idea: review([pin]) })
    await first.close()
    const second = await open({ home })
    const { ideas } = await second.client.request<{ ideas: Idea[] }>('ideas.list')
    await second.close()
    expect(ideas.find((saved) => saved.id === idea.id)?.pins).toEqual([{ ...pin, fileId: idea.workspace.entryFileId }])
  })
})
