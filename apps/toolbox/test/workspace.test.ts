/** Workspace migration, annotations and explicit host execution through /ws and the real SDK/CLI. */
import { execFile } from 'node:child_process'
import { cp, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { PY_SUPPORT, TS_SUPPORT } from '../shared/host-support.ts'
import { sdkHost } from '../shared/sdk-host.ts'
import { newIdea } from '../shared/ideas.ts'
import type { Idea, PairCheck } from '../shared/protocol.ts'
import { fakeAgents } from './agent-fixture.ts'
import { REPO_ROOT } from '../server/env.ts'
import { open, pushed, until } from './harness.ts'

const MAIN = `program paired
use "./lib/message.jev" as helper
in name: text
out greeting
task main:
  greeting = helper.greet(name)
task alternate:
  greeting = "Alternate, {name}!"
`
const LIB = `program helper
def greet(name):
  return "Hello, {name}!"
`
const ASK = `program ask
needs me: person
out answer
task main:
  reply = me.ask "Run the pair?"
  answer = reply.answer
`

let session: Awaited<ReturnType<typeof open>> | null = null
afterEach(async () => { await session?.close(); session = null })

async function setup(source = MAIN) {
  const home = await mkdtemp(join(tmpdir(), 'jevs-workspace-'))
  const env = await fakeAgents(home, MAIN)
  session = await open({ home, env })
  const idea = newIdea({ title: 'Paired idea', fileName: 'main.jev', source, inputs: '{"name":"Ada"}', chatAgent: { harness: 'codex', model: 'codex-second' } })
  idea.workspace.files.push({ id: 'helper', name: 'lib/message.jev', kind: 'jev', source: LIB })
  return { home, env, idea, client: session.client, host: idea.workspace.files.find(file => file.kind === 'host')! }
}

describe('idea-owned Jev and host files', () => {
  it('compiles companions, annotates both kinds, and keeps sources, selections and annotations across restart', async () => {
    const { home, env, client, host, idea: initial } = await setup()
    let { idea } = await client.request<{ idea: Idea }>('ideas.save', { idea: initial })
    expect((await client.request<PairCheck>('pair.check', { idea, hostFileId: host.id })).jev.ir).not.toBeNull()
    for (const file of [idea.workspace.files[0]!, idea.workspace.files.find(file => file.kind === 'host')!]) {
      idea = (await client.request<{ idea: Idea }>('file.annotate', { idea, fileId: file.id, from: 0, to: 7, query: 'Explain this selection' })).idea
    }
    const hostId = idea.workspace.files.find(file => file.kind === 'host')!.id
    idea.workspace.activeFileId = hostId
    idea = (await client.request<{ idea: Idea }>('ideas.save', { idea })).idea
    expect(idea.workspace.annotations.map(note => [note.fileId, note.selected, note.origin.kind])).toEqual([
      [idea.workspace.entryFileId, 'program', 'agent'], [hostId, 'import ', 'agent'],
    ])
    await expect(client.request('file.annotate', { idea, fileId: hostId, from: -1, to: 8, query: 'bad selection' })).rejects.toThrow('Select text')
    await session!.close()
    session = await open({ home, env })
    expect((await session.client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(saved => saved.id === idea.id)).toEqual(idea)
  })

  it('checks without execution, runs the selected host/task/inputs, and saves a replayable recording', async () => {
    const { client, idea, host, home } = await setup()
    const untouched = join(home, 'outside-host.txt')
    await writeFile(untouched, 'preserved')
    host.source = `import { writeFileSync } from 'node:fs'
import { runIdea } from './.toolbox/host.ts'
try { writeFileSync(${JSON.stringify(untouched)}, 'changed'); throw new Error('unexpected access') }
catch (error) { if (error.code !== 'ERR_ACCESS_DENIED') throw error }
if (process.env.OPENAI_API_KEY || process.env.CLAUDE_CODE_OAUTH_TOKEN) throw new Error('unexpected credential')
const result = await runIdea({ task: 'alternate', inputs: { name: 'Host' } })
if (result.outputs.greeting !== 'Alternate, Host!') throw new Error('incorrect selected task')
`
    const checked = await client.request<PairCheck>('pair.check', { idea, hostFileId: host.id })
    expect(checked).toMatchObject({ host: { ok: true }, jev: { diagnostics: [] } })
    expect(await readFile(untouched, 'utf8')).toBe('preserved')
    expect(pushed(client, 'run.ended')).toEqual([])
    const started = await client.request<{ runId: string; recording: string }>('pair.run', { idea, hostFileId: host.id })
    await until(() => pushed(client, 'host.ended').length > 0)
    expect(pushed(client, 'host.ended').at(-1)?.ok).toBe(true)
    expect(pushed(client, 'run.ended', started.runId)[0]?.summary).toMatchObject({ outcome: 'done', outputs: { greeting: 'Alternate, Host!' } })
    expect(await readFile(untouched, 'utf8')).toBe('preserved')
    const saved = (await client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(saved => saved.id === idea.id)!
    expect(saved.workspace.files.find(file => file.id === host.id)?.source).toBe(host.source)
    expect(saved.runs[0]?.recording).toBe(started.recording)
    expect((await client.request<{ exitCode: number }>('replay', { recording: started.recording })).exitCode).toBe(0)
  })

  it('uses the chosen person binding and existing pause/resume flow for the pair', async () => {
    const { client, idea, host } = await setup(ASK)
    idea.bindings = { me: { kind: 'toolbox' } }
    const { runId } = await client.request<{ runId: string }>('pair.run', { idea, hostFileId: host.id })
    await until(() => pushed(client, 'run.pause', runId).some(push => push.pause.kind === 'confirm'))
    expect(pushed(client, 'host.ended')).toEqual([])
    await client.request('run.resume', { runId, payload: { answer: 'yes' } })
    await until(() => pushed(client, 'host.ended').length > 0)
    expect(pushed(client, 'run.ended', runId)[0]?.summary.outputs).toEqual({ answer: 'yes' })
    expect(pushed(client, 'host.ended')[0]?.ok).toBe(true)
  })

  for (const language of ['typescript', 'python'] as const) {
    it(`exports a complete ${language} file set after a declaration change, retaining the saved load target across restart`, async () => {
      const home = await mkdtemp(join(tmpdir(), 'jevs-portable-'))
      const source = 'program portable\nout greeting\ntask main:\n  greeting = "Portable SDK result"\n'
      const env = await fakeAgents(home, source, language)
      session = await open({ home, env })
      let { idea } = await session.client.request<{ idea: Idea }>('chat.send', {
        idea: newIdea({ chatAgent: { harness: 'codex', model: 'codex-second' } }),
        text: `Create a greeting with a complete ${language} host`,
      })
      expect(idea.messages.at(-1)?.origin?.kind).toBe('agent')
      const entry = idea.workspace.files.find(file => file.id === idea.workspace.entryFileId)!
      expect(entry.name).toBe('portable.jev')
      const hostName = language === 'python' ? 'host.py' : 'host.ts'
      expect(idea.workspace.files.map(file => file.name).sort()).toEqual([
        'portable.jev', hostName, language === 'python' ? 'toolbox_runtime.py' : 'runtime.ts', 'runtime.json',
      ].sort())
      await fakeAgents(home, source.replace('program portable', 'program renamed_declaration'), language)
      ;({ idea } = await session.client.request<{ idea: Idea }>('chat.send', { idea, text: `Change the declaration with the complete ${language} host; preserve the saved filename.` }))
      expect(idea.messages.at(-1)?.origin?.kind).toBe('agent')
      expect(idea.messages.at(-1)?.program?.attempts).toBe(2)
      expect(idea.fileName).toBe('portable.jev')
      expect(idea.workspace.files.find(file => file.id === entry.id)).toMatchObject({ name: entry.name, source: expect.stringContaining('program renamed_declaration') })
      expect(idea.workspace.files.find(file => file.name === hostName)?.source).toContain('load("portable.jev"')
      await session.close()
      session = await open({ home, env })
      const restored = (await session.client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(saved => saved.id === idea.id)!
      expect(restored).toEqual(idea)
      const exported = join(home, 'export')
      await mkdir(exported)
      for (const file of restored.workspace.files) await writeFile(join(exported, file.name), file.source)
      if (language === 'typescript') {
        await cp(join(REPO_ROOT, 'sdk/js/dist'), join(exported, 'node_modules/@theaileverage/jevscript/dist'), { recursive: true })
        await writeFile(join(exported, 'node_modules/@theaileverage/jevscript/package.json'), '{"type":"module","exports":"./dist/index.js"}')
        await writeFile(join(exported, 'package.json'), '{"type":"module"}')
      } else await cp(join(REPO_ROOT, 'sdk/python/src/jevscript'), join(exported, 'jevscript'), { recursive: true })
      const output = await new Promise<string>((accept, reject) => execFile(language === 'python' ? 'python3' : process.execPath, [hostName], {
        cwd: exported, timeout: 10_000, maxBuffer: 64 * 1024,
        env: { PATH: process.env['PATH'], JEVSCRIPT_BIN: join(REPO_ROOT, 'target/debug/jevscript') },
      }, (error, stdout) => error ? reject(error) : accept(stdout)))
      expect(output).toContain('Portable SDK result')
      const replay = await new Promise<string>((accept, reject) => execFile(join(REPO_ROOT, 'target/debug/jevscript'), ['replay', join(exported, 'run.jsonl')], {
        timeout: 10_000, env: { PATH: process.env['PATH'] },
      }, (error, stdout) => error ? reject(error) : accept(stdout)))
      expect(replay).toContain('Portable SDK result')
    })

    it(`executes the ${language} SDK host, preserves recording/replay and answers person pauses`, async () => {
      const { client, idea, host } = await setup(ASK)
      host.name = language === 'python' ? 'host.py' : 'host.ts'
      host.source = sdkHost(idea.fileName, language)
      idea.workspace.files.push({ id: 'support', name: language === 'python' ? 'toolbox_runtime.py' : 'runtime.ts', kind: 'host', support: true, source: language === 'python' ? PY_SUPPORT : TS_SUPPORT })
      idea.bindings = { me: { kind: 'toolbox' } }
      expect((await client.request<PairCheck>('pair.check', { idea, hostFileId: host.id })).host.ok).toBe(true)
      const started = await client.request<{ runId: string; recording: string }>('pair.run', { idea, hostFileId: host.id })
      await until(() => pushed(client, 'run.pause', started.runId).some(push => push.pause.kind === 'confirm'))
      // The browser resume may arrive as soon as the runtime pause is published.
      await client.request('run.resume', { runId: started.runId, payload: { answer: 'yes' } })
      await until(() => pushed(client, 'host.ended').length > 0)
      expect(pushed(client, 'host.ended').at(-1)?.ok).toBe(true)
      expect(pushed(client, 'run.ended', started.runId)[0]?.summary.outputs).toEqual({ answer: 'yes' })
      expect((await client.request<{ exitCode: number }>('replay', { recording: started.recording })).exitCode).toBe(0)
      const saved = (await client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(saved => saved.id === idea.id)!
      expect(saved.workspace.files.find(file => file.id === host.id)?.source).toContain('load(')
    })
  }

  it('refuses invalid selections/syntax, surfaces safe execution errors and retains the workspace', async () => {
    const { client, idea, host } = await setup()
    await expect(client.request('pair.run', { idea, hostFileId: idea.workspace.entryFileId })).rejects.toThrow('Select a JavaScript')
    host.source = 'const = broken'
    expect((await client.request<PairCheck>('pair.check', { idea, hostFileId: host.id })).host.ok).toBe(false)
    await expect(client.request('pair.run', { idea, hostFileId: host.id })).rejects.toThrow('could not parse')
    host.source = 'throw new Error("PRIVATE_PROVIDER_SECRET")'
    await expect(client.request('pair.run', { idea, hostFileId: host.id })).rejects.toThrow('exited unsuccessfully')
    expect(JSON.stringify(pushed(client, 'host.ended'))).not.toContain('PRIVATE_PROVIDER_SECRET')
    host.source = `import { runIdea } from './.toolbox/host.ts'
await runIdea()
throw new Error('PRIVATE_PROVIDER_SECRET')
`
    await client.request('pair.run', { idea, hostFileId: host.id })
    await until(() => pushed(client, 'host.ended').length > 0)
    expect(pushed(client, 'host.ended')[0]?.ok).toBe(false)
    expect(JSON.stringify(pushed(client, 'host.ended'))).not.toContain('PRIVATE_PROVIDER_SECRET')
    expect((await client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(saved => saved.id === idea.id)?.runs).toHaveLength(1)
  })
})
