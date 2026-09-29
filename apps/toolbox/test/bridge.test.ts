import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { beforeAll, describe, expect, it } from 'vitest'

import { chatTurn, extractProgram, MAX_REPAIRS, type Model, pastedProgram } from '../server/claude.ts'
import { stubAdapter, SubprocessAdapter } from '../server/adapters.ts'
import { loadEnvFiles, paths, REPO_ROOT } from '../server/env.ts'
import { newIdea } from '../server/ideas.ts'
import { Jevscrypt } from '../server/jevscrypt.ts'
import { readProfiles, resolveProfile } from '../server/profiles.ts'

let jev: Jevscrypt
let triage = ''

beforeAll(async () => {
  jev = new Jevscrypt(paths().bin, await mkdtemp(join(tmpdir(), 'jevs-bridge-')))
  triage = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
})

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

function scripted(replies: string[]): Model & { seen: unknown[][] } {
  const seen: unknown[][] = []
  return {
    name: 'fake',
    seen,
    async complete(_system, messages) {
      seen.push(structuredClone(messages))
      const text = replies.shift()
      if (text === undefined) throw new Error('no more scripted replies')
      return { content: [{ type: 'text', text }], text }
    },
  }
}

const fenced = (source: string) => `Here it is.\n\n\`\`\`jev\n${source}\`\`\``

describe('chat drafting (every draft is checked; at most two repairs)', () => {
  it('feeds check errors back and adopts the repaired draft', async () => {
    const model = scripted([fenced(BROKEN), fenced(triage)])
    const idea = await chatTurn(newIdea(), 'Sort my inbox by urgency', { model, compile: (f, s) => jev.compile(f, s), spec: 'SPEC' })
    const reply = idea.messages.at(-1)!
    expect(reply.program).toMatchObject({ clean: true, attempts: 2, source: triage })
    expect(idea.source).toBe(triage)
    expect(idea.fileName).toBe('triage_lane.jev')
    expect(idea.title).toBe('Sort my inbox by urgency')
    const repairTurn = model.seen[1]!.at(-1) as { content: string }
    expect(repairTurn.content).toMatch(/^jevscrypt check reported:\ntriage_lane\.jev:\d+:\d+: /)
    expect(idea.claudeHistory).toHaveLength(4)
    expect(model.seen[1]!.slice(0, 2)).toEqual(idea.claudeHistory.slice(0, 2))
  })

  it('stops after two repairs and shows the draft with its diagnostics, without adopting it', async () => {
    const model = scripted([fenced(BROKEN), fenced(BROKEN), fenced(BROKEN)])
    const idea = await chatTurn(newIdea({ source: triage }), 'make it worse', { model, compile: (f, s) => jev.compile(f, s), spec: 'SPEC' })
    const program = idea.messages.at(-1)!.program!
    expect(program.clean).toBe(false)
    expect(program.attempts).toBe(1 + MAX_REPAIRS)
    expect(program.diagnostics.some((d) => d.severity === 'error')).toBe(true)
    expect(idea.source).toBe(triage)
    expect(model.seen).toHaveLength(1 + MAX_REPAIRS)
  })

  it('checks a pasted program without any model', async () => {
    const idea = await chatTurn(newIdea(), triage, { model: null, compile: (f, s) => jev.compile(f, s), spec: 'SPEC' })
    expect(idea.messages.at(-1)!.program).toMatchObject({ clean: true, attempts: 0 })
    expect(idea.source).toBe(triage)
  })

  it('without a key, explains instead of drafting', async () => {
    const idea = await chatTurn(newIdea(), 'Sort my inbox', { model: null, compile: (f, s) => jev.compile(f, s), spec: 'SPEC' })
    expect(idea.messages.at(-1)!.text).toContain('ANTHROPIC_API_KEY')
    expect(idea.messages.at(-1)!.program).toBeUndefined()
  })

  it('tells a pasted program from a request that merely contains code', () => {
    expect(pastedProgram(`\`\`\`jev\n${triage}\`\`\``)).toBe(triage)
    expect(pastedProgram(`Can you add a billing lane to this? It should also flag refunds and chargebacks.\n\`\`\`jev\n${triage}\`\`\``)).toBeNull()
    expect(extractProgram('no code here').program).toBeNull()
    expect(pastedProgram(`\`\`\`jev\n${triage}\`\`\`\n\`\`\`jev\n${BROKEN}\`\`\``)).toBeNull()
  })

  it('removes every fenced block from the prose and keeps the first program', () => {
    const reply = `First draft:\n\n\`\`\`jev\n${triage}\`\`\`\n\nOr this:\n\`\`\`jevscrypt\n${BROKEN}\`\`\`\nAnd in Python:\n\`\`\`python\nprint(1)\n\`\`\`\nDone.`
    expect(extractProgram(reply)).toEqual({ program: triage, prose: 'First draft:\n\nOr this:\nAnd in Python:\nDone.', extra: 2 })
    expect(extractProgram('Before ```jev\nprogram x\n``` after\nunclosed:\n```\ntask main:')).toEqual({
      program: 'program x\n',
      prose: 'Before\nafter\nunclosed:',
      extra: 1,
    })
    expect(extractProgram('Inline ```code``` stays.')).toEqual({ program: null, prose: 'Inline ```code``` stays.', extra: 0 })
  })

  it('shows only the checked program when a reply has two', async () => {
    const model = scripted([`Two options.\n\n\`\`\`jev\n${triage}\`\`\`\n\n\`\`\`jev\n${BROKEN}\`\`\``])
    const idea = await chatTurn(newIdea(), 'Sort my inbox', { model, compile: (f, s) => jev.compile(f, s), spec: 'SPEC' })
    const reply = idea.messages.at(-1)!
    expect(reply.program).toMatchObject({ clean: true, attempts: 1, source: triage })
    expect(reply.text).toBe('Two options.\n\n(Another code block in the reply went unchecked and is not shown.)')
    expect(reply.text).not.toContain('when = "now"')
    expect(model.seen).toHaveLength(1)
  })
})

describe('adapters', () => {
  it('speaks the section 11.6 JSONL protocol, one exchange at a time, and carries retryability', async () => {
    const script = `while read line; do
      case "$line" in
        *'"verb":"boom"'*) echo '{"error":{"message":"later","retryable":true}}';;
        *'"operation":"observe"'*) echo '{"observation":{"status":"running","last_message":"hi","tail":"t"}}';;
        *) echo "{\\"result\\": $line}";;
      esac
    done`
    const adapter = new SubprocessAdapter('tree', 'tool', script)
    const [first, second] = await Promise.all([
      adapter.call('diff', { positional: [1] }),
      adapter.call('files', { named: { all: true } }),
    ])
    expect(first).toEqual({ operation: 'call', capability: 'tree', verb: 'diff', args: { positional: [1] } })
    expect(second).toMatchObject({ verb: 'files', args: { named: { all: true } } })
    expect(await adapter.observe({ capability: 'tree', id: 'h' })).toMatchObject({ status: 'running' })
    await expect(adapter.call('boom', {})).rejects.toMatchObject({ message: 'later', retryable: true })
    await adapter.close()
  })

  it('reports an adapter that exits instead of hanging', async () => {
    const adapter = new SubprocessAdapter('tree', 'tool', 'exit 0')
    await expect(adapter.call('diff', {})).rejects.toThrow(/exited unexpectedly/)
  })

  it('the stub returns a value of each declared return type and publishes a manifest', () => {
    const stub = stubAdapter({
      name: 'tree',
      kind: 'tool',
      signatures: [
        { name: 'tests_pass', returns: 'bool' },
        { name: 'test_summary', returns: 'text' },
        { name: 'count', returns: 'number' },
      ],
    })
    expect(stub.call('tests_pass', {})).toBe(true)
    expect(stub.call('test_summary', {})).toBe('stub test_summary')
    expect(stub.call('count', {})).toBe(0)
    expect(Object.keys(stub.manifest!.verbs)).toEqual(['tests_pass', 'test_summary', 'count'])
    const agent = stubAdapter({ name: 'claude', kind: 'agent' })
    expect(agent.call('spawn', { named: { prompt: 'x' } })).toEqual({ id: 'stub-1' })
    expect(agent.call('spawn', { named: { prompt: 'y' } })).toEqual({ id: 'stub-2' })
  })
})

describe('profiles and keys', () => {
  it('layers the JEVSCRYPT_PROFILES overlay over the bundle and follows aliases', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'jevs-profiles-'))
    const overlay = join(dir, 'p.json')
    const custom = { ...JSON.parse(await readFile(paths().bundledProfiles, 'utf8'))[1], model: 'jev-custom', max_questions_per_request: 8 }
    await writeFile(overlay, JSON.stringify([custom]))
    const { profiles } = await readProfiles(paths().bundledProfiles, overlay)
    expect(profiles.map((profile) => [profile.model, profile.source])).toEqual([
      ['jev-latest', 'bundled'],
      ['jev-1.13.0', 'bundled'],
      ['jev-custom', 'overlay'],
    ])
    expect(resolveProfile(profiles, 'jev-latest')?.model).toBe('jev-1.13.0')
    expect(resolveProfile(profiles, 'jev-custom')?.max_questions_per_request).toBe(8)
  })

  it('loads .env keys without overriding the environment', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'jevs-env-'))
    await writeFile(join(dir, '.env'), 'TYPESAFE_API_KEY="from-file"\nexport OTHER=1\nEMPTY=\n')
    const env: NodeJS.ProcessEnv = { OTHER: 'kept' }
    expect(loadEnvFiles([join(dir, '.env'), join(dir, 'missing')], env)).toEqual(['TYPESAFE_API_KEY'])
    expect(env).toEqual({ OTHER: 'kept', TYPESAFE_API_KEY: 'from-file' })
  })
})
