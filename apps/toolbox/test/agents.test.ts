/** Chat selection and failure behavior through /ws, with real compiler/SQLite and fixture executables. */
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { afterEach, describe, expect, it } from 'vitest'

import type { Idea, ServerStatus } from '../shared/protocol.ts'
import { newIdea } from '../shared/ideas.ts'
import { REPO_ROOT } from '../server/env.ts'
import { agentCalls, fakeAgents } from './agent-fixture.ts'
import { open } from './harness.ts'

let session: Awaited<ReturnType<typeof open>> | null = null
afterEach(async () => { await session?.close(); session = null })

async function setup(timeoutMs?: number) {
  const home = await mkdtemp(join(tmpdir(), 'jevs-agents-'))
  const source = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
  const env = await fakeAgents(home, source)
  session = await open({ home, env: { ...env, ANTHROPIC_API_KEY: 'must-not-reach-cli', OPENAI_API_KEY: 'must-not-reach-cli' }, ...(timeoutMs ? { agentTimeoutMs: timeoutMs } : {}) })
  return { home, env, source, client: session.client }
}

describe('local agent drafting through executable CLIs', () => {
  it('discovers models, selects both harnesses, checks drafts and keeps selection/conversation across a restart', async () => {
    const { home, env, source, client } = await setup()
    const status = await client.request<ServerStatus>('status')
    expect(status.agents.map(agent => [agent.harness, agent.state, agent.models.map(model => model.id)])).toEqual([
      ['claude-code', 'available', ['default', 'sonnet']], ['codex', 'available', ['codex-first', 'codex-second']],
    ])
    let idea = newIdea({ chatAgent: { harness: 'claude-code', model: 'sonnet' } })
    idea = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Sort my inbox' })).idea
    expect(idea.messages.at(-1)).toMatchObject({ program: { clean: true, source }, origin: { kind: 'agent', harness: 'claude-code', model: 'resolved-sonnet', requestedModel: 'sonnet' } })
    idea.chatAgent = { harness: 'codex', model: 'codex-second' }
    idea = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Keep the urgency lanes' })).idea
    expect(idea.messages.at(-1)).toMatchObject({ origin: { kind: 'agent', harness: 'codex', model: 'codex-second' }, program: { clean: true } })
    const calls = await agentCalls(home)
    expect(calls.map(call => [call.harness, call.model])).toEqual([['claude-code', 'sonnet'], ['codex', 'codex-second']])
    expect(calls[1]?.input).toContain('Sort my inbox')
    const canonicalHome = await realpath(home)
    expect(calls.every(call => call.providerEnvironment.length === 0 && call.cwd.startsWith(join(canonicalHome, 'agent-work')))).toBe(true)
    expect(calls[0]?.args).toEqual(expect.arrayContaining(['--tools', '', '--safe-mode', '--strict-mcp-config', '--no-session-persistence']))
    expect(calls[1]?.args).toEqual(expect.arrayContaining(['--ignore-user-config', '--ignore-rules', '--ephemeral', 'read-only', 'shell_tool', 'hooks', 'plugins', 'approval_policy="never"']))
    await session!.close()
    session = await open({ home, env })
    expect((await session.client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(item => item.id === idea.id)).toEqual(idea)
  })

  it('checks pasted programs with an unavailable agent and never invokes a CLI', async () => {
    const { home, source, client } = await setup()
    const idea = newIdea({ chatAgent: { harness: 'codex', model: 'not-advertised' } })
    const reply = (await client.request<{ idea: Idea }>('chat.send', { idea, text: source })).idea
    expect(reply.messages.at(-1)).toMatchObject({ origin: { kind: 'check' }, program: { clean: true, attempts: 0 } })
    expect(await agentCalls(home)).toEqual([])
  })

  it('persists safe failures without fixture fallback, and recovers on the same idea', async () => {
    const { home, client } = await setup()
    await writeFile(join(home, 'fake-agents/failure'), '')
    let idea = newIdea({ chatAgent: { harness: 'codex', model: 'codex-first' } })
    idea = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Draft a real program' })).idea
    expect(idea.messages.at(-1)?.origin?.kind).toBe('error')
    expect(idea.messages.at(-1)?.text).toContain('CLI failed')
    expect(JSON.stringify(idea)).not.toContain('PRIVATE_PROVIDER_SECRET')
    expect(idea.messages.at(-1)?.program).toBeUndefined()
    expect((await client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(item => item.id === idea.id)?.messages).toEqual(idea.messages)
    await rm(join(home, 'fake-agents/failure'))
    idea = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Try again' })).idea
    expect(idea.messages.at(-1)?.origin?.kind).toBe('agent')
  })

  it('refuses stale model IDs and signed-out/unavailable CLIs without losing the user request', async () => {
    const { home, client } = await setup()
    let idea = newIdea({ chatAgent: { harness: 'claude-code', model: 'invented-model' } })
    idea = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Please draft' })).idea
    expect(idea.messages.at(-1)?.text).toContain('no longer advertised')
    expect(await agentCalls(home)).toEqual([])
    await writeFile(join(home, 'fake-agents/signed-out'), '')
    await client.request('agents.refresh')
    expect((await client.request<ServerStatus>('status')).agents.every(agent => agent.state === 'unavailable')).toBe(true)
    idea.chatAgent = { harness: 'claude-code', model: 'default' }
    idea = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Still here' })).idea
    expect(idea.messages.at(-2)?.text).toBe('Still here')
    expect(idea.messages.at(-1)).toMatchObject({ origin: { kind: 'error' } })
  })

  it('terminates a hung executable within the response budget', async () => {
    const { home, client } = await setup(150)
    await writeFile(join(home, 'fake-agents/timeout'), '')
    const idea = newIdea({ chatAgent: { harness: 'codex', model: 'codex-first' } })
    const reply = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Respond' })).idea
    expect(reply.messages.at(-1)?.text).toContain('timed out')
    expect(reply.messages.at(-1)?.origin?.kind).toBe('error')
  })

  it('bounds oversized CLI output and input before adopting a draft', async () => {
    const { home, client } = await setup()
    await writeFile(join(home, 'fake-agents/oversized'), '')
    const idea = newIdea({ chatAgent: { harness: 'codex', model: 'codex-first' } })
    const reply = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'Respond' })).idea
    expect(reply.messages.at(-1)?.text).toContain('response size limit')
    expect(reply.messages.at(-1)?.program).toBeUndefined()
    const count = (await agentCalls(home)).length
    const large = (await client.request<{ idea: Idea }>('chat.send', { idea, text: 'x'.repeat(512 * 1024) })).idea
    expect(large.messages.at(-1)?.text).toContain('input limit')
    expect((await agentCalls(home)).length).toBe(count)
  })

  it('upgrades the SQLite migration base without losing an earlier conversation', async () => {
    const { home, env, client } = await setup()
    const idea = newIdea({ title: 'Earlier idea', source: 'program earlier\ntask main:\n  log info "saved"\n', messages: [{ id: 'old', role: 'assistant', text: 'Earlier answer', at: '2026-09-29T00:00:00Z' }], pins: [{ id: 'old-pin', number: 1, target: { kind: 'state', machine: 'review', state: 'waiting' }, query: 'Earlier note', reply: { kind: 'answer', text: 'Saved answer' }, status: 'open', createdAt: '2026-09-29T00:00:00Z' }] })
    await client.request('ideas.save', { idea })
    await session!.close()
    session = null
    const db = new DatabaseSync(join(home, 'toolbox.sqlite'))
    db.exec('ALTER TABLE ideas DROP COLUMN chat_agent; ALTER TABLE ideas DROP COLUMN workspace; ALTER TABLE ideas DROP COLUMN description; ALTER TABLE pins DROP COLUMN file_id; PRAGMA user_version = 1;')
    db.close()
    session = await open({ home, env })
    expect((await session.client.request<{ ideas: Idea[] }>('ideas.list')).ideas.find(item => item.id === idea.id)).toMatchObject({ chatAgent: null, source: idea.source, workspace: { files: [{ kind: 'jev', source: idea.source }, { kind: 'host' }], annotations: [] }, pins: idea.pins, messages: idea.messages, title: 'Earlier idea' })
  })
})
