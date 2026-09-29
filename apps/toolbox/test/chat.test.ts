/**
 * Chat drafting through `/ws`, with the real Anthropic SDK client pointed at a
 * local stand-in for the Messages API (a fixture, not the live service). Every
 * draft is compiled by the real `jevscript compile` before the page sees it,
 * and compile errors go back to Claude for at most two repairs.
 */
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'

import type { Idea } from '../shared/protocol.ts'
import { AnthropicModel, MAX_REPAIRS } from '../server/claude.ts'
import { REPO_ROOT } from '../server/env.ts'
import { newIdea } from '../shared/ideas.ts'
import { fakeClaude, type FakeClaude, open } from './harness.ts'

/** The design's first draft: `when` is reserved, the pick lacks `other`, and it uses an inline if/else. */
const BROKEN = `program triage_lane

in message: text

judgment triage(message):
  urgency = message pick:
    today   "needs a reply before end of day"
    week    "can wait a few days"
    ignore  "no reply expected"

task main:
  t = triage(message)
  when = "now"
`

const fenced = (source: string) => `Here it is.\n\n\`\`\`jev\n${source}\`\`\``

let claude: FakeClaude
let triage = ''
let session: Awaited<ReturnType<typeof open>> | null = null

beforeAll(async () => {
  triage = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
  claude = await fakeClaude()
  process.env['ANTHROPIC_BASE_URL'] = claude.baseUrl
})

afterAll(async () => {
  delete process.env['ANTHROPIC_BASE_URL']
  await claude.close()
})

afterEach(async () => {
  await session?.close()
  session = null
  claude.requests.length = 0
  claude.replies.length = 0
})

async function chat(text: string, idea: Idea = newIdea(), model: 'claude' | 'none' = 'claude'): Promise<Idea> {
  session ??= await open({
    home: await mkdtemp(join(tmpdir(), 'jevs-chat-')),
    model: model === 'claude' ? new AnthropicModel('claude-opus-5-5', 'test-key') : null,
  })
  return (await session.client.request<{ idea: Idea }>('chat.send', { idea, text })).idea
}

describe('chat drafting (every draft is checked; at most two repairs)', () => {
  it('feeds check errors back and adopts the repaired draft', async () => {
    claude.replies.push(fenced(BROKEN), fenced(triage))
    const idea = await chat('Sort my inbox by urgency')
    expect(idea.messages.at(-1)!.program).toMatchObject({ clean: true, attempts: 2, source: triage })
    expect(idea.messages.at(-1)!.text).toBe('Here it is. (1 repair after check)')
    expect(idea).toMatchObject({ source: triage, fileName: 'triage_lane.jev', title: 'Sort my inbox by urgency' })
    expect(claude.requests.map((request) => request.model)).toEqual(['claude-opus-5-5', 'claude-opus-5-5'])
    expect(claude.requests[0]!.system[0]!.text).toContain('<specification>')
    const repairTurn = claude.requests[1]!.messages.at(-1) as { content: string }
    expect(repairTurn.content).toMatch(/^jevscript check reported:\ntriage_lane\.jev:\d+:\d+: /)
    // History is append-only, so the first turn resends unchanged.
    expect(claude.requests[1]!.messages[0]).toEqual(claude.requests[0]!.messages[0])
    expect(idea.claudeHistory).toHaveLength(4)
  })

  it('stops after two repairs and shows the draft with its diagnostics, without adopting it', async () => {
    claude.replies.push(fenced(BROKEN), fenced(BROKEN), fenced(BROKEN))
    const idea = await chat('make it worse', newIdea({ source: triage }))
    const program = idea.messages.at(-1)!.program!
    expect(program).toMatchObject({ clean: false, attempts: 1 + MAX_REPAIRS })
    expect(program.diagnostics.some((d) => d.severity === 'error')).toBe(true)
    expect(idea.source).toBe(triage)
    expect(claude.requests).toHaveLength(1 + MAX_REPAIRS)
  })

  it('checks a pasted program without calling the model', async () => {
    const bare = await chat(triage)
    expect(bare.messages.at(-1)!.program).toMatchObject({ clean: true, attempts: 0 })
    expect(bare.source).toBe(triage)
    const inFence = await chat(`\`\`\`jev\n${triage}\`\`\``)
    expect(inFence.messages.at(-1)!.program).toMatchObject({ clean: true, attempts: 0 })
    expect(claude.requests).toHaveLength(0)
  })

  it('sends a request that merely contains code to the model', async () => {
    claude.replies.push('Noted.', 'Noted.')
    await chat(`Can you add a billing lane to this? It should also flag refunds and chargebacks.\n\`\`\`jev\n${triage}\`\`\``)
    await chat(`\`\`\`jev\n${triage}\`\`\`\n\`\`\`jev\n${BROKEN}\`\`\``)
    expect(claude.requests).toHaveLength(2)
  })

  it('without a key, explains instead of drafting', async () => {
    const idea = await chat('Sort my inbox', newIdea(), 'none')
    expect(idea.messages.at(-1)!.text).toContain('ANTHROPIC_API_KEY')
    expect(idea.messages.at(-1)!.program).toBeUndefined()
  })

  it('shows only the checked program: other fenced blocks leave the prose', async () => {
    claude.replies.push(
      `First draft:\n\n\`\`\`jev\n${triage}\`\`\`\n\nOr this:\n\`\`\`jevscript\n${BROKEN}\`\`\`\nAnd in Python:\n\`\`\`python\nprint(1)\n\`\`\`\nDone.`,
    )
    const idea = await chat('Sort my inbox')
    const reply = idea.messages.at(-1)!
    expect(reply.program).toMatchObject({ clean: true, attempts: 1, source: triage })
    expect(reply.text).toBe('First draft:\n\nOr this:\nAnd in Python:\nDone.\n\n(2 other code blocks in the reply went unchecked and are not shown.)')
    expect(reply.text).not.toContain('when = "now"')
    expect(claude.requests).toHaveLength(1)
  })

  it('treats an unclosed fence as the end of the program', async () => {
    claude.replies.push(`Before \`\`\`jev\n${triage}\`\`\` after\nunclosed:\n\`\`\`\ntask main:`)
    const reply = (await chat('Sort my inbox')).messages.at(-1)!
    expect(reply.program?.source).toBe(triage)
    expect(reply.text).toBe('Before\nafter\nunclosed:\n\n(Another code block in the reply went unchecked and is not shown.)')
  })
})
