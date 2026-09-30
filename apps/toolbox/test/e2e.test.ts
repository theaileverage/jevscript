/**
 * End to end through the toolbox server: `examples/review_loop.jev` under stub
 * bindings, over the page's WebSocket, against the real `jevscript serve` and
 * a local stand-in for the TypeSafe endpoint. Then the recording is replayed
 * and must make no live call of any kind (spec section 10.4).
 */
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { machineGraph } from '../shared/graph.ts'
import type { Pause } from '../shared/pauses.ts'
import type { CheckResult, ReplayResult, ServerMessage } from '../shared/protocol.ts'
import { machineSteps, type RecordingEvent } from '../shared/recording.ts'
import { REPO_ROOT } from '../server/env.ts'
import { type Client, fakeJev, type FakeJev, open, until } from './harness.ts'

let jev: FakeJev
let session: Awaited<ReturnType<typeof open>>
let client: Client
let pushes: ServerMessage[]
const request = <T>(type: string, payload: Record<string, unknown> = {}) => client.request<T>(type, payload)

beforeAll(async () => {
  const home = await mkdtemp(join(tmpdir(), 'jevs-toolbox-e2e-'))
  jev = await fakeJev(['finished', 'approved'])
  process.env['TYPESAFE_API_KEY'] = 'test-key'
  process.env['JEVSCRIPT_PROFILES'] = await jev.profiles(home)
  session = await open({ home, env: process.env })
  client = session.client
  pushes = client.pushes
})

afterAll(async () => {
  await session.close()
  await jev.close()
  delete process.env['TYPESAFE_API_KEY']
  delete process.env['JEVSCRIPT_PROFILES']
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

    const hitsBeforeReplay = jev.bodies.length
    expect(hitsBeforeReplay).toBe(2)
    const callsBeforeReplay = events.filter((event) => event.event === 'call').length
    const replay = await request<ReplayResult>('replay', { recording: started.recording })
    expect(replay.exitCode).toBe(0)
    expect(jev.bodies.length).toBe(hitsBeforeReplay)
    expect((replay.pauses.at(-1) as Pause).kind).toBe('done')
    expect(replay.stderr).not.toContain('stub')
    expect(replay.events.filter((event) => event.event === 'call')).toHaveLength(callsBeforeReplay)
    expect(pushes.filter((push) => push.type === 'notify')).toHaveLength(1)
  })
})
