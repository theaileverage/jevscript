/**
 * The Playground and Adapters requests through `/ws`, each answered by the
 * real `jevscript` CLI or `jevscript serve`: check, tool manifests (spec 9.4),
 * profiles (10.6), `/judge` (11.3), the error reference, and a capability
 * bound to a JSONL subprocess (11.6) in a real run.
 */
import { execFileSync } from 'node:child_process'
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import type { Pause } from '../shared/pauses.ts'
import type { CheckResult, ProfileInfo } from '../shared/protocol.ts'
import { parseRecording } from '../shared/recording.ts'
import { paths, REPO_ROOT } from '../server/env.ts'
import { type Client, fakeJev, type FakeJev, open, pushed, until } from './harness.ts'

const BENCH = `program bench

needs tree: tool:
  diff(n) -> record
  files() -> record
  flaky() -> record

out first
out second

task main:
  first = tree.diff 1
  second = tree.files all true
  tree.flaky
`

/** Echoes each request as its result; `flaky` fails retryably the first time only. */
const ECHO = `n=0
while read line; do
  case "$line" in
    *'"verb":"flaky"'*) n=$((n+1)); if [ $n -eq 1 ]; then echo '{"error":{"message":"later","retryable":true}}'; else echo '{"result": {}}'; fi;;
    *) echo "{\\"result\\": $line}";;
  esac
done
`

let dir: string
let jev: FakeJev
let session: Awaited<ReturnType<typeof open>>
let client: Client
let triage = ''

beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), 'jevs-bench-'))
  jev = await fakeJev(['week'])
  await writeFile(join(dir, 'overlay.json'), JSON.stringify([{ ...JSON.parse(await readFile(paths().bundledProfiles, 'utf8'))[1], model: 'jev-custom', endpoint: jev.endpoint }]))
  await writeFile(join(dir, 'echo.sh'), ECHO)
  triage = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
  // `jevscript serve` inherits the process environment, as it does under `pnpm dev`.
  process.env['TYPESAFE_API_KEY'] = 'test-key'
  process.env['JEVSCRIPT_PROFILES'] = join(dir, 'overlay.json')
  session = await open({ home: join(dir, 'home') })
  client = session.client
})

afterAll(async () => {
  await session.close()
  await jev.close()
  delete process.env['TYPESAFE_API_KEY']
  delete process.env['JEVSCRIPT_PROFILES']
})

const start = (bindings: Record<string, unknown>, source = BENCH) =>
  client.request<{ runId: string; recording: string }>('run.start', {
    run: { ideaId: `bench-${Math.random()}`, title: 'bench', fileName: 'bench.jev', source, inputs: {}, bindings, model: 'jev-latest', sample: false },
  })

const pausesOf = (runId: string) => pushed(client, 'run.pause', runId).map((push) => push.pause as Pause)

describe('checking', () => {
  it('reports every diagnostic, warnings included, against the idea’s file name', async () => {
    const source = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')
    const result = await client.request<CheckResult>('check', { fileName: 'review_loop.jev', source })
    expect(result.ir?.machines.map((machine) => machine.name)).toEqual(['review'])
    expect(result.diagnostics).toEqual([
      expect.objectContaining({ file: 'review_loop.jev', line: 15, column: 4, severity: 'warning', code: 'uncapped_field' }),
    ])
    const broken = await client.request<CheckResult>('check', { fileName: 'x.jev', source: 'program x\n\ntask main:\n  when = 1\n' })
    expect(broken.ir).toBeNull()
    expect(broken.diagnostics.filter((d) => d.severity === 'error').length).toBeGreaterThan(0)
  })

  it('names each tool verb a manifest lacks (spec 9.4)', async () => {
    const manifests = { tree: { verbs: { diff: { params: ['n'], returns: 'record' }, files: { returns: 'record' } } } }
    const result = await client.request<{ missing: string[] }>('tools.check', { fileName: 'bench.jev', source: BENCH, manifests })
    expect(result.missing).toEqual(['tree.flaky'])
  })

  it('serves the error reference behind diagnostic hovers', async () => {
    const reference = await client.request<Record<string, { meaning: string; correction: string }>>('errors.reference')
    expect(reference['uncapped_field']).toEqual({
      meaning: 'A shaped or observed field may grow without a declared cap.',
      correction: 'Add `max`, or shape the value into a bounded representation.',
    })
    expect(reference['pick_no_other']?.meaning).toBe('A `pick` has no escape label.')
  })
})

describe('profiles and judging', () => {
  it('layers the JEVSCRIPT_PROFILES overlay over the bundle', async () => {
    const { profiles, overlay } = await client.request<{ profiles: ProfileInfo[]; overlay: string }>('profiles')
    expect(overlay).toBe(join(dir, 'overlay.json'))
    expect(profiles.map((profile) => [profile.model, profile.source])).toEqual([
      ['jev-latest', 'bundled'],
      ['jev-1.13.0', 'bundled'],
      ['jev-custom', 'overlay'],
    ])
    expect(profiles[0]?.aliases).toBe('jev-1.13.0')
  })

  it('runs one judgment alone with `/judge` against the chosen profile (spec 11.3)', async () => {
    const calls = jev.bodies.length
    const { answers } = await client.request<{ answers: Record<string, unknown> }>('judge', {
      fileName: 'triage_lane.jev',
      source: triage,
      judgment: 'triage',
      state: { message: 'Quick one on our March invoice' },
      model: 'jev-custom',
    })
    expect(jev.bodies).toHaveLength(calls + 1)
    expect(JSON.parse(jev.bodies.at(-1)!).state).toEqual({ message: 'Quick one on our March invoice' })
    expect(answers).toMatchObject({ urgency: { label: 'week' }, billing: { $jev: 'prob', value: 0.9 } })
  })
})

describe('a capability bound to a JSONL subprocess (spec 11.6)', () => {
  it('carries arguments both ways and pauses on a retryable failure until the host retries', async () => {
    const { runId, recording } = await start({ tree: { kind: 'subprocess', command: `sh ${join(dir, 'echo.sh')}` } })
    await until(() => pausesOf(runId).some((pause) => pause.kind === 'error'))
    expect(pausesOf(runId).at(-1)).toMatchObject({ kind: 'error', code: 'adapter_error', message: 'later', retryable: true })
    await client.request('run.resume', { runId, payload: { retry: true } })
    await until(() => pushed(client, 'run.ended', runId).length > 0)
    const { summary } = pushed(client, 'run.ended', runId)[0]!
    expect(summary.outcome).toBe('done')
    expect(summary.outputs).toEqual({
      first: { operation: 'call', capability: 'tree', verb: 'diff', args: { positional: [1] } },
      second: { operation: 'call', capability: 'tree', verb: 'files', args: { named: { all: true } } },
    })
    const effects = parseRecording(await readFile(recording, 'utf8')).filter((event) => ['call', 'effect_error'].includes(event.event))
    expect(effects.map((event) => [event.event, event['verb'] ?? (event['identity'] as { verb: string }).verb])).toEqual([
      ['call', 'diff'],
      ['call', 'files'],
      ['effect_error', 'flaky'],
      ['call', 'flaky'],
    ])
  })

  it('stops the run when the adapter exits instead of hanging', async () => {
    const { runId } = await start({ tree: { kind: 'subprocess', command: 'exit 0' } })
    await until(() => pushed(client, 'run.ended', runId).length > 0)
    const last = pausesOf(runId).find((pause) => pause.kind === 'error')
    expect(last?.message).toMatch(/adapter `tree` exited unexpectedly/)
  })

  it('refuses to start when the manifest lacks a verb the program uses', async () => {
    const manifest = JSON.stringify({ verbs: { diff: { params: ['n'], returns: 'record' }, files: { returns: 'record' } } })
    await expect(start({ tree: { kind: 'subprocess', command: `sh ${join(dir, 'echo.sh')}`, manifest } })).rejects.toThrow(/verb_missing|flaky/)
  })
})

const tmux = (() => {
  try {
    execFileSync('tmux', ['-V'], { stdio: 'ignore' })
    return true
  } catch {
    return false
  }
})()

describe('an agent pane (display only, spec 9.1)', () => {
  it.skipIf(!tmux)('tails a real tmux pane by id and refuses anything that is not a pane id', async () => {
    const previous = process.env['TMUX_TMPDIR']
    process.env['TMUX_TMPDIR'] = await mkdtemp(join(tmpdir(), 'jevs-tmux-'))
    try {
      const pane = execFileSync('tmux', ['new-session', '-d', '-P', '-F', '#{pane_id}', 'printf "agent-written text\\n"; sleep 30'], { encoding: 'utf8' }).trim()
      await until(() => execFileSync('tmux', ['capture-pane', '-p', '-t', pane], { encoding: 'utf8' }).includes('agent-written text'))
      const { text } = await client.request<{ text: string }>('pane.tail', { pane })
      expect(text).toBe('agent-written text')
      await expect(client.request('pane.tail', { pane: '%1; rm -rf /' })).rejects.toThrow('is not a tmux pane id')
    } finally {
      execFileSync('tmux', ['kill-server'], { stdio: 'ignore' })
      if (previous === undefined) delete process.env['TMUX_TMPDIR']
      else process.env['TMUX_TMPDIR'] = previous
    }
  })
})
