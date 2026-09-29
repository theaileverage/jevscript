/**
 * End to end through the toolbox server: `examples/review_loop.jev` under stub
 * bindings, over the page's WebSocket, against the real `jevscrypt serve` and
 * a local stand-in for the TypeSafe endpoint. Then the recording is replayed
 * and must make no live call of any kind (spec section 10.4).
 */
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { WebSocket } from 'ws'

import { machineGraph } from '../shared/graph.ts'
import type { Pause } from '../shared/pauses.ts'
import type { CheckResult, ReplayResult, ServerMessage } from '../shared/protocol.ts'
import { machineSteps, type RecordingEvent } from '../shared/recording.ts'
import { startToolbox, type Toolbox } from '../server/app.ts'
import { paths, REPO_ROOT } from '../server/env.ts'

/** The labels the stand-in prefers, in order; anything else gets `stay`. */
const PREFERRED = ['finished', 'approved']

let jevHits = 0
let jev: Server
let toolbox: Toolbox
let socket: WebSocket
const pushes: ServerMessage[] = []
let nextId = 1
const waiting = new Map<number, (message: ServerMessage) => void>()

function request<T>(type: string, payload: Record<string, unknown> = {}): Promise<T> {
  const id = nextId++
  return new Promise((resolve, reject) => {
    waiting.set(id, (message) => {
      if (message.type !== 'reply') return
      if (message.ok) resolve(message.result as T)
      else reject(new Error(message.error))
    })
    socket.send(JSON.stringify({ id, type, ...payload }))
  })
}

async function until(predicate: () => boolean, ms = 30_000): Promise<void> {
  const deadline = Date.now() + ms
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('timed out waiting for the run')
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
}

beforeAll(async () => {
  jev = createServer((req, res) => {
    let body = ''
    req.on('data', (chunk: Buffer) => (body += chunk))
    req.on('end', () => {
      jevHits += 1
      const sent = JSON.parse(body) as { model: string; questions: Record<string, { type: string; criteria: unknown }> }
      const answers: Record<string, unknown> = {}
      for (const [id, question] of Object.entries(sent.questions)) {
        if (question.type !== 'choice') {
          answers[id] = { noul: 0.9 }
          continue
        }
        const labels = Object.keys(question.criteria as Record<string, unknown>)
        const pick = PREFERRED.find((label) => labels.includes(label)) ?? 'stay'
        const probabilities = Object.fromEntries(
          labels.map((label) => [label, label === pick ? 0.87 : 0.13 / (labels.length - 1)]),
        )
        answers[id] = { choice: pick, probabilities, confidence: 0.87 }
      }
      res.setHeader('content-type', 'application/json')
      res.end(JSON.stringify({ model: sent.model, answers, usage: { input_tokens: 120, output_tokens: 0 } }))
    })
  })
  await new Promise<void>((done) => jev.listen(0, '127.0.0.1', done))
  const home = await mkdtemp(join(tmpdir(), 'jevs-toolbox-e2e-'))
  const profiles = join(home, 'profiles.json')
  const bundled = JSON.parse(await readFile(paths().bundledProfiles, 'utf8')) as Record<string, unknown>[]
  const endpoint = `http://127.0.0.1:${(jev.address() as AddressInfo).port}/v1/systemone`
  await writeFile(profiles, JSON.stringify(bundled.map((profile) => ({ ...profile, endpoint }))))
  process.env['TYPESAFE_API_KEY'] = 'test-key'
  process.env['JEVSCRYPT_PROFILES'] = profiles

  toolbox = await startToolbox({ home, env: process.env })
  socket = new WebSocket(`${toolbox.url.replace('http', 'ws')}/ws`)
  socket.on('message', (data) => {
    const message = JSON.parse(String(data)) as ServerMessage
    if (message.type === 'reply') waiting.get(message.id)?.(message)
    else pushes.push(message)
  })
  await new Promise((resolve) => socket.once('open', resolve))
})

afterAll(async () => {
  socket?.close()
  await toolbox?.close()
  await new Promise<void>((done) => jev.close(() => done()))
  delete process.env['TYPESAFE_API_KEY']
  delete process.env['JEVSCRYPT_PROFILES']
})

describe('review_loop.jev under stub bindings, through the server', () => {
  it('draws the graph, records the steps, and replays with zero live calls', async () => {
    const source = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')

    const checked = await request<CheckResult>('check', { fileName: 'review_loop.jev', source })
    expect(checked.diagnostics.filter((d) => d.severity === 'error')).toEqual([])
    expect(checked.diagnostics.map((d) => d.code)).toEqual(['uncapped_field'])
    const machine = checked.ir?.machines.find((candidate) => candidate.name === 'review')
    expect(machine).toBeDefined()
    const graph = machineGraph(machine!)
    expect(graph.nodes.map((node) => node.id)).toEqual(['working', 'nudging', 'waiting_on_me', 'reviewing', 'approved'])
    expect(graph.nodes.find((node) => node.id === 'approved')?.terminal).toBe(true)
    expect(graph.nodes.find((node) => node.id === 'working')?.initial).toBe(true)
    const style = (id: string) => graph.edges.find((edge) => edge.id === id)?.style
    expect(style('working.finished')).toBe('guarded')
    expect(style('working.stuck')).toBe('picked')
    expect(style('reviewing.rejected')).toBe('risky')
    expect(graph.edges.find((edge) => edge.id === 'reviewing.approved')?.guard).toBe('tree.tests_pass')

    const started = await request<{ runId: string; recording: string }>('run.start', {
      run: {
        ideaId: 'e2e',
        title: 'review loop',
        fileName: 'review_loop.jev',
        source,
        inputs: {},
        bindings: { claude: { kind: 'stub' }, tree: { kind: 'stub' }, me: { kind: 'toolbox' } },
        model: 'jev-latest',
        sample: false,
      },
    })
    await until(() => pushes.some((push) => push.type === 'run.ended' && push.runId === started.runId))

    const pauses = pushes.flatMap((push) => (push.type === 'run.pause' && push.runId === started.runId ? [push.pause] : []))
    expect(pauses.at(-1)?.kind).toBe('done')
    expect(pushes.filter((push) => push.type === 'run.failed')).toEqual([])
    const notified = pushes.flatMap((push) => (push.type === 'notify' ? [push.message] : []))
    expect(notified).toEqual(['Ready: 2 steps'])

    const { events } = await request<{ events: RecordingEvent[] }>('recording.read', { recording: started.recording })
    const streamed = pushes.flatMap((push) => (push.type === 'run.event' && push.runId === started.runId ? [push.event] : []))
    expect(streamed.map((event) => event.seq)).toEqual(events.map((event) => event.seq))
    const steps = machineSteps(events, checked.ir)
    expect(steps.map((step) => [step.from, step.chosen, step.to, step.verdict])).toEqual([
      ['working', 'finished', 'reviewing', 'proceed'],
      ['reviewing', 'approved', 'approved', 'proceed'],
    ])
    expect(steps[0]?.observed).toEqual({ summary: 'built-in stub observation', tests: 'stub test_summary', status: 'waiting' })
    expect(steps[1]?.menu.map((entry) => entry.event)).toEqual(['approved', 'rejected', 'stay'])
    expect(steps[1]?.menu.find((entry) => entry.event === 'rejected')?.risky).toBe(true)
    expect(steps[0]?.checks.every((check) => check.pass)).toBe(true)

    const hitsBeforeReplay = jevHits
    expect(hitsBeforeReplay).toBe(2)
    const callsBeforeReplay = events.filter((event) => event.event === 'call').length
    const replay = await request<ReplayResult>('replay', { recording: started.recording })
    expect(replay.exitCode).toBe(0)
    expect(jevHits).toBe(hitsBeforeReplay)
    expect((replay.pauses.at(-1) as Pause).kind).toBe('done')
    expect(replay.stderr).not.toContain('stub')
    expect(replay.events.filter((event) => event.event === 'call')).toHaveLength(callsBeforeReplay)
    expect(pushes.filter((push) => push.type === 'notify')).toHaveLength(1)
  })
})
