/**
 * The Requests tab's resend, through `/ws`. Recordings come from real runs
 * started through the toolbox against a local TypeSafe stand-in, and a resend
 * must post exactly the body `jevscript serve` posted for that request (spec
 * section 10.3 keeps the request; the wire mapping is a port of `jev.rs`, see
 * SPEC-GAPS.md item 3). It is a separate live call: the recording is never
 * written, and it still replays.
 */
import { createHash } from 'node:crypto'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import type { Pause } from '../shared/pauses.ts'
import type { CheckResult, ReplayResult } from '../shared/protocol.ts'
import { type RecordingEvent, startInfo } from '../shared/recording.ts'
import { type RequestEntry, requestEntries, type ResendRecord } from '../shared/requests.ts'
import { REPO_ROOT } from '../server/env.ts'
import { type Client, fakeJev, type FakeJev, open, pushed, until } from './harness.ts'

let jev: FakeJev
let session: Awaited<ReturnType<typeof open>>
let client: Client
let triageSource: string
let triage: { recording: string; bodies: string[] }
let review: { recording: string; bodies: string[] }

const MESSAGE = { message: 'Quick one on our March invoice' }

/** Run a program through the toolbox to its end, and keep the bodies the runtime posted while it ran. */
async function record(fileName: string, source: string, inputs: Record<string, unknown>, bindings: Record<string, unknown>) {
  const from = jev.bodies.length
  const { runId, recording } = await client.request<{ runId: string; recording: string }>('run.start', {
    run: { ideaId: `resend-${fileName}-${Math.random()}`, title: fileName, fileName, source, inputs, bindings, model: 'jev-latest', sample: false },
  })
  const ended = () => pushed(client, 'run.ended', runId).length > 0
  await until(() => ended() || pushed(client, 'run.pause', runId).some((push) => (push.pause as Pause).kind === 'error'))
  if (!ended()) await client.request('run.abort', { runId })
  await until(ended)
  return { runId, recording, bodies: jev.bodies.slice(from) }
}

async function entries(recording: string): Promise<RequestEntry[]> {
  const { events } = await client.request<{ events: RecordingEvent[] }>('recording.read', { recording })
  return requestEntries(events, startInfo(events)!.ir)
}

const resend = (recording: string, entry: RequestEntry, request = entry.request, ideaId = 'idea-1') =>
  client.request<ResendRecord>('resend', { ideaId, recording, requestId: entry.requestId, request })

const sha = async (file: string) => createHash('sha256').update(await readFile(file)).digest('hex')

beforeAll(async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jevs-resend-'))
  jev = await fakeJev(['finished', 'approved', 'week'])
  process.env['TYPESAFE_API_KEY'] = 'test-key'
  process.env['JEVSCRIPT_PROFILES'] = await jev.profiles(dir)
  session = await open({ home: join(dir, 'home') })
  client = session.client
  triageSource = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
  triage = await record('triage_lane.jev', triageSource, MESSAGE, {})
  const tree = join(dir, 'tree.sh')
  await writeFile(tree, `while read line; do case "$line" in *tests_pass*) echo '{"result": true}';; *) echo '{"result": "214 passed"}';; esac; done\n`)
  const reviewSource = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')
  review = await record('review_loop.jev', reviewSource, {}, {
    claude: { kind: 'stub' },
    tree: { kind: 'subprocess', command: `sh ${tree}` },
    me: { kind: 'toolbox' },
  })
})

afterAll(async () => {
  await session.close()
  await jev.close()
  delete process.env['TYPESAFE_API_KEY']
  delete process.env['JEVSCRIPT_PROFILES']
})

describe('resending a recorded request', () => {
  it('posts the body the runtime posted, byte for byte, for named judgments and machine steps', async () => {
    const origins = []
    for (const run of [triage, review]) {
      const recorded = await entries(run.recording)
      expect(recorded).toHaveLength(run.bodies.length)
      for (const [index, entry] of recorded.entries()) {
        origins.push(entry.origin)
        const record = await resend(run.recording, entry)
        expect(record).toMatchObject({ via: 'endpoint', edited: [], error: null })
        expect(jev.bodies.at(-1)).toBe(run.bodies[index])
      }
    }
    expect(origins).toEqual([
      { kind: 'judgment', name: 'triage' },
      { kind: 'machine', machine: 'review', state: 'working' },
      { kind: 'machine', machine: 'review', state: 'reviewing' },
    ])
  })

  it('makes a separate live call, keeps history, and leaves the recording byte-identical and replayable', async () => {
    const before = await sha(review.recording)
    const entry = (await entries(review.recording))[1]!
    const edited = { ...entry.request, state: { ...entry.request.state, obs: { ...(entry.request.state['obs'] as object), tests: '3 failed' } } }
    jev.prefer.unshift('rejected')
    const calls = jev.bodies.length
    const record = await resend(review.recording, entry, edited, 'idea-2')
    jev.prefer.shift()
    expect(record).toMatchObject({ via: 'endpoint', edited: ['state.obs.tests'], error: null })
    expect(record.answers).toMatchObject([{ type: 'choice', id: 'event', label: 'rejected', confidence: 0.8 }])
    expect(jev.bodies).toHaveLength(calls + 1)
    expect(JSON.parse(jev.bodies.at(-1)!).state.obs.tests).toBe('3 failed')
    expect(await sha(review.recording)).toBe(before)
    const { history } = await client.request<{ history: ResendRecord[] }>('resends.list', { ideaId: 'idea-2', recording: review.recording, requestId: entry.requestId })
    expect(history).toEqual([record])
    const replay = await client.request<ReplayResult>('replay', { recording: review.recording })
    expect((replay.pauses.at(-1) as Pause).kind).toBe('done')
    expect(jev.bodies).toHaveLength(calls + 1)
  })

  it('sends the recorded questions, not the current program’s, even when the shape hash still matches', async () => {
    const entry = (await entries(triage.recording))[0]!
    // A new label description and condition: same labels and kinds, so the same shape hash (spec section 11.4).
    const reworded = triageSource
      .replace('"needs a reply before end of day"', '"is on fire"')
      .replace('feels "a customer asking about billing"', 'feels "mentions money"')
    const hash = (result: CheckResult) => (result.ir?.judgments as { shape_hash: string }[] | undefined)?.[0]?.shape_hash
    const [original, current] = await Promise.all(
      [triageSource, reworded].map((source) => client.request<CheckResult>('check', { fileName: 'triage_lane.jev', source })),
    )
    expect(hash(current!)).toBeDefined()
    expect(hash(current!)).toBe(hash(original!))
    const record = await resend(triage.recording, entry, { ...entry.request, state: { message: 'Standup moved to 10:30' } })
    expect(record).toMatchObject({ edited: ['state.message'], error: null })
    const sent = JSON.parse(jev.bodies.at(-1)!)
    expect(sent.questions.urgency.criteria.today).toBe('needs a reply before end of day')
    expect(sent.questions.billing.instructions.question).toContain('a customer asking about billing')
    expect(jev.bodies.at(-1)).not.toMatch(/on fire|mentions money/)
  })

  it('keeps a deleted idea’s resend history removed when a late resend tries to save', async () => {
    const entry = (await entries(triage.recording))[0]!
    const ideaId = 'deleted-resend-idea'
    await resend(triage.recording, entry, entry.request, ideaId)
    await client.request('ideas.delete', { ideaId })
    await expect(resend(triage.recording, entry, entry.request, ideaId)).rejects.toThrow('deleted')
    expect((await client.request<{ history: ResendRecord[] }>('resends.list', { ideaId, recording: triage.recording, requestId: entry.requestId })).history).toEqual([])
    expect((await client.request<ReplayResult>('replay', { recording: triage.recording })).exitCode).toBe(0)
  })
})

describe('a malformed endpoint response', () => {
  it('fails a resend with the message the runtime gives for the same response', async () => {
    const malformed: unknown[] = [
      null,
      { answers: [] },
      { answers: {} },
      { answers: { urgency: { choice: 'today', probabilities: { today: 'bad', week: 0.2, ignore: 0 } }, billing: { noul: 0.5 } } },
      { answers: { urgency: { choice: 1, probabilities: { today: 1, week: 0, ignore: 0 } }, billing: { noul: 0.5 } } },
      { answers: { urgency: { choice: 'today', probabilities: { today: 1, week: 0, ignore: 0 } }, billing: { noul: '0.5' } } },
    ]
    try {
      for (const body of malformed) {
        jev.respond = () => body
        const run = await record('triage_lane.jev', triageSource, MESSAGE, {})
        const { events } = await client.request<{ events: RecordingEvent[] }>('recording.read', { recording: run.recording })
        const pause = events.find((event) => event.event === 'pause' && event['kind'] === 'error')?.['payload'] as Pause | undefined
        const record_ = await resend(run.recording, (await entries(run.recording))[0]!)
        expect(record_.answers).toBeNull()
        expect(record_.error).toBeTruthy()
        expect(pause?.message).toContain(record_.error!)
      }
    } finally {
      jev.respond = null
    }
  })
})
