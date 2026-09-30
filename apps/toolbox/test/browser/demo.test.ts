/**
 * `pnpm demo` as the captain runs it: one process that serves the built page
 * on 127.0.0.1 against the fixture services, replaces any real key in its
 * environment and reads no `.env`, keeps its state in SQLite across a restart, and stops its
 * fixtures with it. The page itself is driven in `toolbox.test.ts`.
 */
import { type ChildProcess, spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdtemp } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { newIdea } from '../../shared/ideas.ts'
import type { Idea, RunSummary, ServerStatus } from '../../shared/protocol.ts'
import { APP_ROOT } from '../../server/env.ts'
import { connect, pushed, until } from '../harness.ts'
import { fakeAgents } from '../agent-fixture.ts'

let home: string
let url: string
let port: number
let demo: ChildProcess | null = null
let log = ''
let agentEnv: NodeJS.ProcessEnv

async function freePort(): Promise<number> {
  const probe = createServer()
  await new Promise<void>((done) => probe.listen(0, '127.0.0.1', done))
  const { port } = probe.address() as { port: number }
  await new Promise((done) => probe.close(done))
  return port
}

/** Start `node demo/main.ts` with keys that would fail loudly if they were ever used. */
async function start(): Promise<void> {
  log = ''
  demo = spawn(process.execPath, ['demo/main.ts'], {
    cwd: APP_ROOT,
    env: {
      ...process.env,
      ...agentEnv,
      JEVS_TOOLBOX_HOME: home,
      JEVS_TOOLBOX_PORT: String(port),
      TYPESAFE_API_KEY: 'real-looking-key',
      ANTHROPIC_API_KEY: 'real-looking-key',
      ANTHROPIC_BASE_URL: 'http://127.0.0.1:9',
      JEVSCRIPT_PROFILES: '/nonexistent/profiles.json',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  demo.stdout?.on('data', (chunk: Buffer) => (log += chunk.toString()))
  demo.stderr?.on('data', (chunk: Buffer) => (log += chunk.toString()))
  await until(() => log.includes('jevs toolbox demo:') || demo?.exitCode !== null, 20_000)
  expect(log).toContain(`jevs toolbox demo: ${url}`)
}

async function stop(): Promise<number | null> {
  const child = demo!
  const exited = new Promise<number | null>((resolve) => child.once('exit', resolve))
  child.kill('SIGINT')
  demo = null
  return exited
}

beforeAll(async () => {
  // A home that does not exist yet, as `~/.jevs-toolbox-demo` on a first run.
  home = join(await mkdtemp(join(tmpdir(), 'jevs-demo-')), 'home')
  const { readFile } = await import('node:fs/promises')
  agentEnv = await fakeAgents(home, await readFile(join(APP_ROOT, 'fixtures/inbox_triage.jev'), 'utf8'))
  port = await freePort()
  url = `http://127.0.0.1:${port}`
})

afterAll(async () => {
  if (demo) await stop()
})

describe('pnpm demo', () => {
  it('serves the page on localhost against the fixtures, and keeps its state in SQLite across a restart', async () => {
    await start()
    const page = await fetch(url)
    expect(page.status).toBe(200)
    expect(await page.text()).toContain('<div id="root">')
    // The demo never loads a .env file, unlike `pnpm start`.
    expect(log).not.toContain('keys loaded from .env')

    let client = await connect({ url })
    const status = await client.request<ServerStatus>('status')
    expect(status).toMatchObject({ home, database: join(home, 'toolbox.sqlite'), typesafeKey: true, claude: { available: true } })

    const { idea: drafted } = await client.request<{ idea: Idea }>('chat.send', { idea: newIdea({ chatAgent: { harness: 'codex', model: 'codex-second' } }), text: 'Sort my inbox by urgency' })
    expect(drafted.messages.at(-1)?.program).toMatchObject({ clean: true })
    expect(drafted.messages.at(-1)?.origin).toMatchObject({ kind: 'agent', harness: 'codex', model: 'codex-second' })
    expect(drafted.messages.at(-1)?.text).toContain('Executable codex codex-second')
    await client.request('ideas.save', { idea: drafted })

    const triage = (await client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find((idea) => idea.title === 'Inbox triage by urgency')!
    const { runId, recording } = await client.request<{ runId: string; recording: string }>('run.start', {
      run: { ideaId: triage.id, title: triage.title, fileName: triage.fileName, source: triage.source, inputs: JSON.parse(triage.inputs), bindings: {}, model: triage.model, sample: false },
    })
    await until(() => pushed(client, 'run.ended', runId).length > 0)
    const summary: RunSummary = pushed(client, 'run.ended', runId)[0]!.summary
    expect(summary.outcome).toBe('done')
    client.close()

    expect(await stop()).toBe(0)
    await expect(fetch(url)).rejects.toThrow()

    await start()
    client = await connect({ url })
    const ideas = (await client.request<{ ideas: Idea[] }>('ideas.list')).ideas
    expect(ideas.find((idea) => idea.id === drafted.id)?.messages).toEqual(drafted.messages)
    expect(ideas.find((idea) => idea.id === triage.id)?.runs).toMatchObject([{ runId, recording, outcome: 'done' }])
    expect(existsSync(recording)).toBe(true)
    client.close()
    expect(await stop()).toBe(0)
  })
})
