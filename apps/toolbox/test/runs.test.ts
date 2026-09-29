/**
 * The server owns a run's lifecycle: one run per idea, the run's summary saved
 * on its idea when it ends whether or not a page is listening, and a clean
 * abort on shutdown so the recording keeps its terminal events (spec 10.3).
 */
import { execFile } from 'node:child_process'
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, describe, expect, it } from 'vitest'
import type { Pause } from '../shared/pauses.ts'
import type { Idea } from '../shared/protocol.ts'
import { parseRecording } from '../shared/recording.ts'
import { startToolbox, type Toolbox } from '../server/app.ts'
import { paths } from '../server/env.ts'
import { newIdea } from '../shared/ideas.ts'
import { type Client, connect, until } from './harness.ts'

/** Pauses on a person and makes no Jev call, so no endpoint is needed. */
const ASK = `program ask_me

needs me: person

task main:
  a = me.ask "Go ahead?"
  me.notify "answered {a.answer}"
`

const start = (idea: Idea) => ({
  run: { ideaId: idea.id, title: idea.title, fileName: 'ask_me.jev', source: ASK, inputs: {}, bindings: {}, model: 'jev-latest', sample: false },
})

/** A toolbox on a fresh home, with the ask program saved as an idea through the page's own request. */
async function withIdea(): Promise<{ client: Client; idea: Idea; home: string }> {
  const home = await mkdtemp(join(tmpdir(), 'jevs-runs-'))
  toolbox = await startToolbox({ home })
  const client = await connect(toolbox)
  const { idea } = await client.request<{ idea: Idea }>('ideas.save', { idea: newIdea({ title: 'ask', source: ASK }) })
  return { client, idea, home }
}

const saved = async (client: Client, id: string) =>
  (await client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find((idea) => idea.id === id)

let toolbox: Toolbox | null = null
afterEach(async () => {
  await toolbox?.close()
  toolbox = null
})

describe('run lifecycle on the server', () => {
  it('refuses a second run for an idea while one is starting or live', async () => {
    const { client, idea } = await withIdea()
    const results = await Promise.allSettled([client.request('run.start', start(idea)), client.request('run.start', start(idea))])
    expect(results.map((result) => result.status).sort()).toEqual(['fulfilled', 'rejected'])
    expect(String((results.find((result) => result.status === 'rejected') as PromiseRejectedResult).reason)).toContain('already has a run')
    client.close()
  })

  it('saves the finished run on its idea even with no page connected', async () => {
    const { client, idea } = await withIdea()
    const { runId } = await client.request<{ runId: string }>('run.start', start(idea))
    await until(() => client.pushes.some((push) => push.type === 'run.pause' && (push.pause as Pause).kind === 'confirm'))
    await client.request('run.resume', { runId, payload: { answer: 'yes' } })
    client.close()
    const observer = await connect(toolbox!)
    let found: Idea | undefined
    await until(() => {
      void saved(observer, idea.id).then((idea) => (found = idea))
      return (found?.runs.length ?? 0) > 0
    })
    expect(found?.runs).toMatchObject([{ runId, outcome: 'done', replay: false }])
    // A page that saves its stale copy of the idea afterwards must not drop the run.
    await observer.request('ideas.save', { idea: { ...idea, title: 'renamed', runs: [] } })
    expect((await saved(observer, idea.id))?.runs.map((run) => run.runId)).toEqual([runId])
    expect((await saved(observer, idea.id))?.title).toBe('renamed')
    observer.close()
  })

  it('aborts a paused run on shutdown so its recording ends and replays', async () => {
    const { client, idea } = await withIdea()
    const { recording } = await client.request<{ recording: string }>('run.start', start(idea))
    await until(() => client.pushes.some((push) => push.type === 'run.pause' && (push.pause as Pause).kind === 'confirm'))
    client.close()
    await toolbox!.close()
    toolbox = null
    const events = parseRecording(await readFile(recording, 'utf8'))
    expect(events.slice(-3).map((event) => [event.event, event['kind'] ?? event['reason']])).toEqual([
      ['abort', 'aborted by host'],
      ['pause', 'stopped'],
      ['end', 'stopped'],
    ])
    const replay = await promisify(execFile)(paths().bin, ['replay', recording]).catch((error: { code: number; stdout: string }) => error)
    expect('code' in replay ? replay.code : 0).toBe(3)
    expect(String(replay.stdout).trim().split('\n').at(-1)).toContain('"kind":"stopped"')
  })
})
