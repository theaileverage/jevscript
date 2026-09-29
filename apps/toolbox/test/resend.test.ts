import { execFile } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { parseRecording, startInfo } from '../shared/recording.ts'
import { editedPaths, parseEditedRequest, requestEntries, requestText } from '../shared/requests.ts'
import { requestBody } from '../shared/wire.ts'
import { paths, REPO_ROOT } from '../server/env.ts'
import { Jevscrypt } from '../server/jevscrypt.ts'
import { resend, resendHistory } from '../server/resend.ts'
import { fakeJev, type FakeJev } from './fake-jev.ts'

const run = promisify(execFile)
let jev: FakeJev
let dir: string
let env: NodeJS.ProcessEnv
let triageRecording: string
let reviewRecording: string
let triageSource: string

/** Record a run with the real CLI against the fake endpoint. */
async function record(file: string, name: string, args: string[]): Promise<string> {
  const recording = join(dir, 'recordings', `${name}.jsonl`)
  await run(paths().bin, ['run', file, '--record', recording, ...args], { env })
  return recording
}

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'jevs-resend-'))
  await mkdir(join(dir, 'recordings'))
  jev = await fakeJev(['finished', 'approved', 'week'])
  env = { ...process.env, TYPESAFE_API_KEY: 'test-key', JEVSCRYPT_PROFILES: await jev.profiles(dir) }
  const triage = join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev')
  triageSource = await readFile(triage, 'utf8')
  triageRecording = await record(triage, 'triage', ['--input', JSON.stringify({ message: 'Quick one on our March invoice' })])
  const tree = join(dir, 'tree.sh')
  await writeFile(tree, `while read line; do case "$line" in *tests_pass*) echo '{"result": true}';; *) echo '{"result": "214 passed"}';; esac; done\n`)
  reviewRecording = await record(join(REPO_ROOT, 'examples/review_loop.jev'), 'review', ['--stub', 'claude', '--stub', 'me', '--bind', `tree=sh ${tree}`])
})

afterAll(async () => {
  await jev.close()
})

const sha = async (file: string) => createHash('sha256').update(await readFile(file)).digest('hex')

describe('the request list, from request and answers events', () => {
  it('names a named judgment and counts its questions', async () => {
    const events = parseRecording(await readFile(triageRecording, 'utf8'))
    const entries = requestEntries(events, startInfo(events)!.ir)
    expect(entries.map((entry) => [entry.index, entry.origin, entry.request.questions.map((q) => q.id)])).toEqual([
      [1, { kind: 'judgment', name: 'triage' }, ['urgency', 'billing']],
    ])
    expect(entries[0]!.answers.map((answer) => answer.type)).toEqual(['choice', 'noul'])
    expect(entries[0]!.latencyMs).toBeTypeOf('number')
  })

  it('names each machine step by the state it was asked in', async () => {
    const events = parseRecording(await readFile(reviewRecording, 'utf8'))
    const entries = requestEntries(events, startInfo(events)!.ir)
    expect(entries.map((entry) => [entry.step, entry.origin])).toEqual([
      [1, { kind: 'machine', machine: 'review', state: 'working' }],
      [2, { kind: 'machine', machine: 'review', state: 'reviewing' }],
    ])
  })

  it('ports the runtime wire body byte for byte', async () => {
    for (const recording of [triageRecording, reviewRecording]) {
      const events = parseRecording(await readFile(recording, 'utf8'))
      const model = startInfo(events)!.profile!.model
      for (const entry of requestEntries(events, startInfo(events)!.ir)) {
        expect(jev.bodies).toContain(JSON.stringify(requestBody(entry.request, model)))
      }
    }
  })
})

describe('editing a request', () => {
  it('marks edited paths and resets to the recorded request', async () => {
    const events = parseRecording(await readFile(reviewRecording, 'utf8'))
    const entry = requestEntries(events, startInfo(events)!.ir)[1]!
    const text = requestText(entry.request).replace('"tests": "214 passed"', '"tests": "3 failed, 211 passed"')
    const parsed = parseEditedRequest(text)
    expect('request' in parsed && editedPaths(entry.request, parsed.request)).toEqual(['state.obs.tests'])
    const reset = parseEditedRequest(requestText(entry.request))
    expect('request' in reset && editedPaths(entry.request, reset.request)).toEqual([])
    expect(parseEditedRequest('{"state": 1}')).toEqual({ error: 'a request is `{ "state": { ... }, "questions": [ ... ] }`' })
  })
})

describe('resending', () => {
  it('makes a separate live call, keeps history, and leaves the recording byte-identical', async () => {
    const before = await sha(reviewRecording)
    const events = parseRecording(await readFile(reviewRecording, 'utf8'))
    const entry = requestEntries(events, startInfo(events)!.ir)[1]!
    const edited = JSON.parse(requestText(entry.request).replace('"tests": "214 passed"', '"tests": "3 failed"'))
    jev.prefer.unshift('rejected')
    const calls = jev.bodies.length
    const home = join(dir, 'home')
    const record = await resend(
      { ideaId: 'idea-1', recording: reviewRecording, requestId: entry.requestId, request: edited },
      { home, env },
    )
    jev.prefer.shift()
    expect(record).toMatchObject({ via: 'endpoint', edited: ['state.obs.tests'], error: null })
    expect(record.answers).toMatchObject([{ type: 'choice', id: 'event', label: 'rejected', confidence: 0.8 }])
    expect(jev.bodies).toHaveLength(calls + 1)
    expect(JSON.parse(jev.bodies.at(-1)!).state.obs.tests).toBe('3 failed')
    expect(await sha(reviewRecording)).toBe(before)
    expect(await resendHistory(home, 'idea-1', reviewRecording, entry.requestId)).toEqual([record])
    const replay = await run(paths().bin, ['replay', reviewRecording], { env: { PATH: process.env['PATH'] } })
    expect(replay.stdout.trim().split('\n').at(-1)).toContain('"kind":"done"')
  })

  it('sends the recorded questions, not the current program’s, even when the shape hash still matches', async () => {
    const before = await sha(triageRecording)
    const events = parseRecording(await readFile(triageRecording, 'utf8'))
    const entry = requestEntries(events, startInfo(events)!.ir)[0]!
    // A new label description and condition: same labels and kinds, so the same shape hash (spec section 11.4).
    const reworded = triageSource
      .replace('"needs a reply before end of day"', '"is on fire"')
      .replace('feels "a customer asking about billing"', 'feels "mentions money"')
    expect(reworded).toContain('"is on fire"')
    expect(reworded).toContain('"mentions money"')
    const compiled = new Jevscrypt(paths().bin, join(dir, 'work'))
    const hash = (ir: { judgments: unknown[] } | null) => (ir?.judgments as { shape_hash: string }[] | undefined)?.[0]?.shape_hash
    const [recorded, current] = [startInfo(events)!.ir, (await compiled.compile('triage_lane.jev', reworded)).ir]
    expect(hash(current)).toBeDefined()
    expect(hash(current)).toBe(hash(recorded))
    // The browser still sends the idea's current program along; resend must not use it.
    const request = {
      ideaId: 'idea-2',
      recording: triageRecording,
      requestId: entry.requestId,
      request: { ...entry.request, state: { message: 'Standup moved to 10:30' } },
      fileName: 'triage_lane.jev',
      source: reworded,
    }
    const record = await resend(request, { home: join(dir, 'home'), env })
    expect(record).toMatchObject({ via: 'endpoint', edited: ['state.message'], error: null })
    expect(record.answers?.map((answer) => answer.type)).toEqual(['choice', 'noul'])
    const sent = JSON.parse(jev.bodies.at(-1)!)
    expect(sent).toEqual(requestBody(request.request, startInfo(events)!.profile!.model))
    expect(sent.questions.urgency.criteria.today).toBe('needs a reply before end of day')
    expect(sent.questions.billing.instructions.question).toContain('a customer asking about billing')
    expect(jev.bodies.at(-1)).not.toMatch(/on fire|mentions money/)
    expect(await sha(triageRecording)).toBe(before)
  })
})
