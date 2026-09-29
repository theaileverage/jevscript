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
import { WebSocket } from 'ws'

import type { Pause } from '../shared/pauses.ts'
import type { Idea, ServerMessage } from '../shared/protocol.ts'
import { parseRecording } from '../shared/recording.ts'
import { startToolbox, type Toolbox } from '../server/app.ts'
import { paths } from '../server/env.ts'
import { IdeaStore, newIdea } from '../server/ideas.ts'

/** Pauses on a person and makes no Jev call, so no endpoint is needed. */
const ASK = `program ask_me

needs me: person

task main:
  a = me.ask "Go ahead?"
  me.notify "answered {a.answer}"
`

interface Client {
  request<T>(type: string, payload?: Record<string, unknown>): Promise<T>
  pushes: ServerMessage[]
  close(): void
}

async function connect(toolbox: Toolbox): Promise<Client> {
  const socket = new WebSocket(`${toolbox.url.replace('http', 'ws')}/ws`)
  const pushes: ServerMessage[] = []
  const waiting = new Map<number, (message: ServerMessage) => void>()
  let next = 1
  socket.on('message', (data) => {
    const message = JSON.parse(String(data)) as ServerMessage
    if (message.type === 'reply') waiting.get(message.id)?.(message)
    else pushes.push(message)
  })
  await new Promise((resolve) => socket.once('open', resolve))
  return {
    pushes,
    close: () => socket.close(),
    request<T>(type: string, payload: Record<string, unknown> = {}) {
      const id = next++
      return new Promise<T>((resolve, reject) => {
        waiting.set(id, (message) => {
          if (message.type !== 'reply') return
          if (message.ok) resolve(message.result as T)
          else reject(new Error(message.error))
        })
        socket.send(JSON.stringify({ id, type, ...payload }))
      })
    },
  }
}

async function until(predicate: () => boolean, ms = 20_000): Promise<void> {
  const deadline = Date.now() + ms
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
}

const start = (idea: Idea) => ({
  run: { ideaId: idea.id, title: idea.title, fileName: 'ask_me.jev', source: ASK, inputs: {}, bindings: {}, model: 'jev-latest', sample: false },
})

let toolbox: Toolbox | null = null
afterEach(async () => {
  await toolbox?.close()
  toolbox = null
})

describe('run lifecycle on the server', () => {
  it('refuses a second run for an idea while one is starting or live', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-runs-'))
    const idea = await new IdeaStore(home).save(newIdea({ title: 'ask', source: ASK }))
    toolbox = await startToolbox({ home })
    const client = await connect(toolbox)
    const results = await Promise.allSettled([client.request('run.start', start(idea)), client.request('run.start', start(idea))])
    expect(results.map((result) => result.status).sort()).toEqual(['fulfilled', 'rejected'])
    expect(String((results.find((result) => result.status === 'rejected') as PromiseRejectedResult).reason)).toContain('already has a run')
    client.close()
  })

  it('saves the finished run on its idea even with no page connected', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-runs-'))
    const store = new IdeaStore(home)
    const idea = await store.save(newIdea({ title: 'ask', source: ASK }))
    toolbox = await startToolbox({ home })
    const client = await connect(toolbox)
    const { runId } = await client.request<{ runId: string }>('run.start', start(idea))
    await until(() => client.pushes.some((push) => push.type === 'run.pause' && (push.pause as Pause).kind === 'confirm'))
    await client.request('run.resume', { runId, payload: { answer: 'yes' } })
    client.close()
    let saved: Idea | undefined
    await until(() => {
      void store.get(idea.id).then((found) => (saved = found))
      return (saved?.runs.length ?? 0) > 0
    })
    expect(saved?.runs).toMatchObject([{ runId, outcome: 'done', replay: false }])
    // A page that saves its stale copy of the idea afterwards must not drop the run.
    await store.save({ ...idea, title: 'renamed', runs: [] })
    expect((await store.get(idea.id))?.runs.map((run) => run.runId)).toEqual([runId])
    expect((await store.get(idea.id))?.title).toBe('renamed')
  })

  it('aborts a paused run on shutdown so its recording ends and replays', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-runs-'))
    const idea = await new IdeaStore(home).save(newIdea({ title: 'ask', source: ASK }))
    toolbox = await startToolbox({ home })
    const client = await connect(toolbox)
    const { recording } = await client.request<{ recording: string }>('run.start', start(idea))
    await until(() => client.pushes.some((push) => push.type === 'run.pause' && (push.pause as Pause).kind === 'confirm'))
    client.close()
    await toolbox.close()
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
