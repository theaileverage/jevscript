/**
 * The toolbox's own state lives in `<home>/toolbox.sqlite` and survives a
 * server restart; runtime recordings stay JSONL files next to it. State an
 * earlier build kept as JSON files is imported once and the files are left
 * alone. Everything goes through `/ws`, as the page does.
 */
import { createHash } from 'node:crypto'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import type { Pin } from '../shared/annotator.ts'
import { newIdea } from '../shared/ideas.ts'
import type { Pause } from '../shared/pauses.ts'
import type { Idea, ReplayResult, ServerStatus } from '../shared/protocol.ts'
import { type RecordingEvent, startInfo } from '../shared/recording.ts'
import { requestEntries, type ResendRecord } from '../shared/requests.ts'
import { startToolbox } from '../server/app.ts'
import { REPO_ROOT } from '../server/env.ts'
import { fakeJev, type FakeJev, open, pushed, until } from './harness.ts'

const PIN: Pin = {
  id: 'p1',
  number: 1,
  target: { kind: 'state', machine: 'review', state: 'working' },
  query: 'What proves finished?',
  reply: { kind: 'answer', text: 'The guard tree.tests_pass.' },
  status: 'open',
  createdAt: '2026-09-29T00:00:00.000Z',
}

let jev: FakeJev
let triage: string

beforeAll(async () => {
  jev = await fakeJev(['week'])
  process.env['TYPESAFE_API_KEY'] = 'test-key'
  process.env['JEVSCRIPT_PROFILES'] = await jev.profiles(await mkdtemp(join(tmpdir(), 'jevs-store-')))
  triage = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
})

afterAll(async () => {
  await jev.close()
  delete process.env['TYPESAFE_API_KEY']
  delete process.env['JEVSCRIPT_PROFILES']
})

const find = async (session: Awaited<ReturnType<typeof open>>, id: string) =>
  (await session.client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find((idea) => idea.id === id)

describe('the toolbox database', () => {
  it('keeps ideas, pins, the run index and resend history across a server restart', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-store-'))
    const first = await open({ home })
    const status = await first.client.request<ServerStatus>('status')
    expect(status.database).toBe(join(home, 'toolbox.sqlite'))
    const { idea } = await first.client.request<{ idea: Idea }>('ideas.save', {
      idea: newIdea({ title: 'Triage', fileName: 'triage_lane.jev', source: triage, inputs: '{"message": "Invoice?"}', pins: [PIN] }),
    })
    const { runId, recording } = await first.client.request<{ runId: string; recording: string }>('run.start', {
      run: { ideaId: idea.id, title: idea.title, fileName: idea.fileName, source: triage, inputs: { message: 'Invoice?' }, bindings: {}, model: 'jev-latest', sample: false },
    })
    await until(() => pushed(first.client, 'run.ended', runId).length > 0)
    const { events } = await first.client.request<{ events: RecordingEvent[] }>('recording.read', { recording })
    const [entry] = requestEntries(events, startInfo(events)!.ir)
    const record = await first.client.request<ResendRecord>('resend', { ideaId: idea.id, recording, requestId: entry!.requestId, request: entry!.request })
    await first.close()

    expect(existsSync(status.database)).toBe(true)
    expect(recording.endsWith('.jsonl')).toBe(true)
    const second = await open({ home })
    try {
      const reloaded = await find(second, idea.id)
      expect(reloaded).toMatchObject({ title: 'Triage', source: triage, inputs: '{"message": "Invoice?"}', pins: [PIN] })
      expect(reloaded?.runs).toMatchObject([{ runId, recording, outcome: 'done', replay: false }])
      const { history } = await second.client.request<{ history: ResendRecord[] }>('resends.list', { ideaId: idea.id, recording, requestId: entry!.requestId })
      expect(history).toEqual([record])
      const replay = await second.client.request<ReplayResult>('replay', { recording })
      expect((replay.pauses.at(-1) as Pause).kind).toBe('done')
    } finally {
      await second.close()
    }
  })

  it('imports an earlier build’s JSON files once, and leaves them in place', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-store-'))
    const legacy = newIdea({ id: 'legacy-1', title: 'From JSON', source: triage, pins: [PIN] })
    const run = { runId: 'run_1', recording: join(home, 'recordings', 'r.jsonl'), startedAt: '2026-09-28T00:00:00.000Z', replay: false, outcome: 'done', verified: false, outputs: {}, usage: null }
    const resent = { at: '2026-09-28T00:00:01.000Z', recording: run.recording, requestId: 'r1', via: 'endpoint', edited: [], request: { state: {}, questions: [] }, answers: [], error: null, latencyMs: 3 }
    await mkdir(join(home, 'ideas', 'legacy-1', 'resends', 'r-abcdef12'), { recursive: true })
    await writeFile(join(home, 'ideas', 'legacy-1.json'), JSON.stringify({ ...legacy, runs: [run] }, null, 2))
    await writeFile(join(home, 'ideas', 'legacy-1', 'resends', 'r-abcdef12', 'r1.json'), JSON.stringify([resent]))
    const sha = async (file: string) => createHash('sha256').update(await readFile(file)).digest('hex')
    const before = await sha(join(home, 'ideas', 'legacy-1.json'))

    const first = await open({ home })
    expect(await find(first, 'legacy-1')).toMatchObject({ title: 'From JSON', pins: [PIN], runs: [run] })
    const { history } = await first.client.request<{ history: ResendRecord[] }>('resends.list', { ideaId: 'legacy-1', recording: run.recording, requestId: 'r1' })
    expect(history).toEqual([resent])
    await first.client.request('ideas.delete', { ideaId: 'legacy-1' })
    await first.close()
    expect(await sha(join(home, 'ideas', 'legacy-1.json'))).toBe(before)

    // The files are still there, but a deleted idea stays deleted: the import ran once.
    const second = await open({ home })
    expect(await find(second, 'legacy-1')).toBeUndefined()
    await second.close()
  })

  it('refuses a database written by a newer toolbox instead of guessing at it', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-store-'))
    const db = new DatabaseSync(join(home, 'toolbox.sqlite'))
    db.exec('PRAGMA user_version = 99')
    db.close()
    await expect(startToolbox({ home })).rejects.toThrow(/schema version 99, newer than this toolbox/)
  })
})
