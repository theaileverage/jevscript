import { mkdtemp, readdir, readFile, writeFile, mkdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'
import { WebSocket } from 'ws'

import type { CheckResult } from '../shared/protocol.ts'
import { startInfo, usageSoFar, type RecordingEvent } from '../shared/recording.ts'
import { requestEntries } from '../shared/requests.ts'
import { startToolbox, type Toolbox } from '../server/app.ts'
import { REPO_ROOT } from '../server/env.ts'
import { serveBuilt } from '../server/static.ts'
import { callsLimit, confidenceThreshold } from '../src/runinfo.ts'
import { connect, fakeJev, pushed, until } from './harness.ts'

let toolbox: Toolbox | null = null
afterEach(async () => {
  await toolbox?.close()
  toolbox = null
})

function upgrade(url: string, origin?: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url, origin === undefined ? undefined : { origin })
    socket.once('open', () => {
      socket.close()
      resolve(101)
    })
    socket.once('unexpected-response', (_request, response) => {
      response.resume()
      resolve(response.statusCode ?? 0)
    })
    socket.once('error', reject)
  })
}

describe('reviewed toolbox boundaries', () => {
  it('accepts only its own origin or an explicitly listed exact local origin on both WebSockets', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-access-'))
    toolbox = await startToolbox({ home, env: { ...process.env, JEVS_TOOLBOX_ALLOWED_ORIGINS: 'http://localhost:53123' } })
    for (const route of ['/ws', '/lsp']) {
      const url = `${toolbox.url.replace('http', 'ws')}${route}`
      expect(await upgrade(url, toolbox.url)).toBe(101)
      expect(await upgrade(url, 'http://localhost:53123')).toBe(101)
      for (const origin of [undefined, 'http://localhost:53124', 'https://localhost:53123', 'http://127.0.0.1:53123', 'http://evil.example:53123', '*', 'not-an-origin']) {
        expect(await upgrade(url, origin)).toBe(403)
      }
    }
  })

  it('removes compile, manifest and judgment inputs, and retains a run source only while live', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-inputs-'))
    toolbox = await startToolbox({ home })
    const client = await connect(toolbox)
    const source = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')
    const work = join(home, 'work')
    expect((await client.request<CheckResult>('check', { fileName: 'review_loop.jev', source })).ir).not.toBeNull()
    expect(await readdir(work)).toEqual([])
    await client.request('tools.check', { fileName: 'review_loop.jev', source, manifests: {} })
    expect(await readdir(work)).toEqual([])
    await expect(client.request('judge', { fileName: 'review_loop.jev', source, judgment: 'missing', state: {}, model: 'jev-latest' })).rejects.toThrow()
    expect(await readdir(work)).toEqual([])
    const ask = 'program ask_me\n\nneeds me: person\n\ntask main:\n  a = me.ask "Go ahead?"\n'
    const { runId } = await client.request<{ runId: string }>('run.start', {
      run: { ideaId: 'inputs', title: 'inputs', fileName: 'ask.jev', source: ask, inputs: {}, bindings: {}, model: 'jev-latest', sample: false },
    })
    await until(() => pushed(client, 'run.pause', runId).some((push) => push.pause.kind === 'confirm'))
    expect((await readdir(work)).length).toBe(1)
    await client.request('run.abort', { runId })
    await until(() => pushed(client, 'run.ended', runId).length > 0)
    expect(await readdir(work)).toEqual([])
    client.close()
  })

  it('uses recorded machine ownership and the main budget for run-wide call usage', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-owners-'))
    const jev = await fakeJev(['finished', 'approved'])
    const previousKey = process.env['TYPESAFE_API_KEY']
    const previousProfiles = process.env['JEVSCRIPT_PROFILES']
    process.env['TYPESAFE_API_KEY'] = 'test-key'
    process.env['JEVSCRIPT_PROFILES'] = await jev.profiles(home)
    try {
      toolbox = await startToolbox({ home })
      const client = await connect(toolbox)
      const original = await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')
      const at = original.indexOf('machine review(dev)')
      const task = original.indexOf('task main:')
      const machine = original.slice(at, task)
      const source = `${original.slice(0, at)}${machine.replace('machine review(dev) budget calls 30 thresholds min_confidence 0.6', 'machine first(dev) budget calls 2 thresholds min_confidence 0.9')}${machine.replace('machine review(dev) budget calls 30', 'machine second(dev) budget calls 2')}task main budget calls 10:\n  dev = claude.spawn prompt "Fix the test"\n  r = second(dev, max 20)\n`
      const checked = await client.request<CheckResult>('check', { fileName: 'owners.jev', source })
      expect(checked.ir).not.toBeNull()
      const { runId, recording } = await client.request<{ runId: string; recording: string }>('run.start', {
        run: { ideaId: 'owners', title: 'owners', fileName: 'owners.jev', source, inputs: {}, bindings: { claude: { kind: 'stub' }, tree: { kind: 'stub' }, me: { kind: 'toolbox' } }, model: 'jev-latest', sample: false },
      })
      await until(() => pushed(client, 'run.ended', runId).length > 0)
      const { events } = await client.request<{ events: RecordingEvent[] }>('recording.read', { recording })
      const ir = startInfo(events)!.ir!
      const entries = requestEntries(events, ir)
      expect(entries.map((entry) => entry.origin)).toEqual([
        { kind: 'machine', machine: 'second', state: 'working' },
        { kind: 'machine', machine: 'second', state: 'reviewing' },
      ])
      expect(confidenceThreshold(ir, entries[0]!.origin)).toEqual({ owner: 'machine second', value: 0.6 })
      expect(usageSoFar(events).calls).toBe(2)
      expect(callsLimit(ir)).toBe(10)
      client.close()
    } finally {
      await jev.close()
      if (previousKey === undefined) delete process.env['TYPESAFE_API_KEY']
      else process.env['TYPESAFE_API_KEY'] = previousKey
      if (previousProfiles === undefined) delete process.env['JEVSCRIPT_PROFILES']
      else process.env['JEVSCRIPT_PROFILES'] = previousProfiles
    }
  })

  it('responds to malformed and directory requests while keeping the built page available', async () => {
    const home = await mkdtemp(join(tmpdir(), 'jevs-static-'))
    const dist = join(home, 'dist')
    await mkdir(join(dist, 'assets'), { recursive: true })
    await writeFile(join(dist, 'index.html'), '<main>toolbox</main>')
    toolbox = await startToolbox({ home, serveStatic: serveBuilt(dist) })
    expect((await fetch(`${toolbox.url}/%`)).status).toBe(400)
    expect((await fetch(`${toolbox.url}/assets`)).status).toBe(404)
    expect(await (await fetch(toolbox.url)).text()).toContain('toolbox')
  })
})
