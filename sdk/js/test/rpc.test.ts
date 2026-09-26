import { existsSync } from 'node:fs'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer, type Server } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import type {
  Adapter,
  CallArgs,
  Handle,
  LogEvent,
  Observation,
  ToolManifest,
} from '../src/index.ts'
import { LOG_LEVELS, load, logEvent, RpcClient } from '../src/index.ts'

const repoRoot = fileURLToPath(new URL('../../..', import.meta.url))
const binary =
  process.env['JEVSCRIPT_BIN'] ??
  fileURLToPath(new URL('../../../target/debug/jevscript', import.meta.url))
const itWithBinary = existsSync(binary) ? it : it.skip

let scratch = ''
let server: Server | undefined
let endpoint = ''
let choice = 'needs_me'
let confidence = 0.9
let offScope = 0.9
let priorKey: string | undefined
let machineChoices: string[] = []
let jevRequests = 0

beforeEach(async () => {
  scratch = await mkdtemp(join(tmpdir(), 'jevscript-js-'))
  priorKey = process.env['TYPESAFE_API_KEY']
  process.env['TYPESAFE_API_KEY'] = 'test-key'
  server = createServer((request, response) => {
    const chunks: Buffer[] = []
    request.on('data', (chunk: Buffer) => chunks.push(chunk))
    request.on('end', () => {
      jevRequests += 1
      const body = JSON.parse(Buffer.concat(chunks).toString()) as {
        questions: Record<string, { type: string; criteria?: Record<string, unknown> | unknown[] }>
      }
      const answers: Record<string, unknown> = {}
      for (const [id, question] of Object.entries(body.questions)) {
        if (question.type === 'noul') {
          answers[id] = {
            type: 'noul',
            noul: id.startsWith('off_scope') ? offScope : id === 'same' ? 0.1 : 0.9,
          }
        } else if (question.type === 'choice') {
          const labels = Object.keys(question.criteria as Record<string, unknown>)
          const wanted = id === 'event' ? (machineChoices.shift() ?? choice) : choice
          const selected = labels.includes(wanted) ? wanted : labels[0]
          answers[id] = {
            type: 'choice',
            choice: selected,
            confidence,
            probabilities: Object.fromEntries(
              labels.map((label) => [label, label === selected ? 0.9 : 0.1 / (labels.length - 1)]),
            ),
          }
        } else {
          const levels = question.criteria as unknown[]
          const score = id === 'progress' ? levels.length - 1 : 0
          answers[id] = {
            type: 'score',
            score,
            confidence: 1,
            probabilities: Object.fromEntries(
              levels.map((_, index) => [String(index), index === score ? 1 : 0]),
            ),
          }
        }
      }
      const json = JSON.stringify({ model: 'test', answers, usage: { input_tokens: 1, output_tokens: 0 } })
      response.writeHead(200, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(json) })
      response.end(json)
    })
  })
  await new Promise<void>((resolve) => server?.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (address === null || typeof address === 'string') throw new Error('fake server did not bind')
  endpoint = `http://127.0.0.1:${address.port}/v1/systemone`
  await writeFile(
    join(scratch, 'profiles.json'),
    JSON.stringify([
      {
        model: 'test',
        endpoint,
        total_tokens: 64000,
        state_plus_question_tokens: 32000,
        max_questions_per_request: 64,
        max_criteria_per_question: 64,
        tokenizer: 'chars4',
        price_per_million_input_usd: 0,
        price_per_million_output_usd: 0,
      },
    ]),
  )
  choice = 'needs_me'
  confidence = 0.9
  offScope = 0.9
  machineChoices = []
  jevRequests = 0
})

afterEach(async () => {
  await new Promise<void>((resolve) => server?.close(() => resolve()))
  await rm(scratch, { recursive: true, force: true })
  if (priorKey === undefined) delete process.env['TYPESAFE_API_KEY']
  else process.env['TYPESAFE_API_KEY'] = priorKey
})

class Agent implements Adapter {
  kind = 'agent' as const
  capability?: string
  seenCapability: string | undefined

  call(verb: string, _args: CallArgs, capability?: string): unknown {
    this.seenCapability = capability
    if (verb === 'spawn') return { id: 'dev-1' }
    if (verb === 'wait') return this.observation()
    return null
  }

  observe(handle: Handle, capability?: string): Observation {
    this.seenCapability = capability
    expect(handle.capability).toBe('claude')
    return this.observation()
  }

  private observation(): Observation {
    return { status: 'waiting', last_message: 'done', tail: 'done', exit_code: null }
  }
}

class Tree implements Adapter {
  kind = 'tool' as const
  capability?: string
  checks = 0
  passing = false
  readonly manifest: ToolManifest = {
    verbs: {
      create: { params: ['branch'], returns: 'handle' },
      diff: { params: [], returns: 'record' },
      test_summary: { params: [], returns: 'text' },
      tests_pass: { params: [], returns: 'bool' },
      open_pr: { params: [], returns: 'text' },
    },
  }

  call(verb: string): unknown {
    if (verb === 'create') return { id: 'tree-1' }
    if (verb === 'diff') return { files: ['src/lib.rs'] }
    if (verb === 'test_summary') return 'passing soon'
    if (verb === 'tests_pass') {
      this.checks += 1
      return this.passing
    }
    if (verb === 'open_pr') return 'https://example.test/pr/1'
    return null
  }
}

const person: Adapter = { kind: 'person', call: () => null }

describe('the runtime boundary', () => {
  itWithBinary('loads metadata and rejects unknown methods', async () => {
    const program = await load('examples/inbox_triage.jev', { bin: binary, cwd: repoRoot })
    expect(program.name).toBe('inbox_triage')
    expect(program.inputs.map((input) => input.name)).toEqual(['message'])
    expect(program.judgments.map((judgment) => judgment.name)).toContain('triage')
    expect(program.inputs[0]).not.toHaveProperty('span')
    expect(program.judgments[0]).not.toHaveProperty('span')
    expect(program.judgments[0]?.results[0]).toEqual({
      name: 'urgent',
      each: false,
      verb: 'feels',
    })
    expect(program.judgments[0]?.results[1]).toMatchObject({
      name: 'owner',
      verb: 'pick',
      labels: ['code', 'customer', 'schedule', 'other'],
    })
    expect(JSON.stringify(program.judgments)).not.toContain('request_group')
    expect(JSON.stringify(program.judgments)).not.toContain('subject')
    await program.close()

    const client = new RpcClient({ bin: binary, cwd: repoRoot })
    await expect(client.request('program.destroy', {})).rejects.toMatchObject({ code: -32601 })
    await client.close()
  })

  itWithBinary('loads paths as paths and preserves diagnostic filenames', async () => {
    const invalid = join(scratch, 'uppercase.jev')
    await writeFile(invalid, 'program INVALID\n')
    await expect(load(invalid, { bin: binary, cwd: repoRoot })).rejects.toMatchObject({
      data: {
        kind: 'compile_error',
        diagnostics: [{ code: 'uppercase_identifier', file: invalid }],
      },
    })
  })

  itWithBinary('rejects record and replay together at the SDK boundary', async () => {
    const source = 'program exclusive\n\ntask main:\n  return true\n'
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const run = program.task('main').start({
      record: join(scratch, 'new.jsonl'),
      replay: join(scratch, 'missing.jsonl'),
    })
    await expect(run.id()).rejects.toMatchObject({ data: { kind: 'type_error' } })
    await program.close()
  })

  itWithBinary('encodes only manifest-declared tool handles on the wire', async () => {
    let inspected: unknown
    const tool: Adapter = {
      kind: 'tool',
      manifest: {
        verbs: {
          create: { params: ['name'], returns: 'handle' },
          inspect: { params: ['item'], returns: 'bool' },
          metadata: { params: [], returns: 'record' },
        },
      },
      call(verb, args) {
        if (verb === 'create') return { id: 'tree-1' }
        if (verb === 'inspect') {
          inspected = args.positional?.[0]
          return true
        }
        return { id: 'plain-record' }
      },
    }
    const source = `program handle_wire

out metadata

needs tree: tool:
  create(name) -> handle
  inspect(item) -> bool
  metadata() -> record

task main:
  item = tree.create "branch"
  tree.inspect item
  metadata = tree.metadata
`
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const pause = await program.task('main').start({ bind: { tree: tool } }).next()
    expect(pause).toMatchObject({
      kind: 'done',
      outputs: { metadata: { id: 'plain-record' } },
    })
    expect(inspected).toEqual({ $jev: 'handle', capability: 'tree', id: 'tree-1' })
    await program.close()
  })

  itWithBinary('runs fix_issue through adapters, confirmation, and live events', async () => {
    const agent = new Agent()
    const tree = new Tree()
    const program = await load('examples/fix_issue.jev', { bin: binary, cwd: repoRoot })
    const run = program.task('main').start({
      inputs: { issue: { title: 'bug', body: 'fix it', branch: 'fix' } },
      bind: { claude: agent, tree, me: person },
      model: 'test',
      profiles: join(scratch, 'profiles.json'),
    })
    let sawConfirm = false
    let terminal
    for await (const pause of run) {
      terminal = pause
      if (pause.kind === 'confirm') {
        sawConfirm = true
        tree.passing = true
        await run.resume({ answer: 'yes', text: 'continue' })
      }
    }
    expect(sawConfirm).toBe(true)
    expect(terminal).toMatchObject({
      kind: 'done',
      outputs: { pr_url: 'https://example.test/pr/1' },
    })
    expect(agent.capability).toBe('claude')
    expect(agent.seenCapability).toBe('claude')
    const events = []
    for await (const event of run.events()) events.push(event)
    expect(events.some((event) => event.event === 'request')).toBe(true)
    expect(events.some((event) => event.event === 'call')).toBe(true)
    await program.close()
  })

  itWithBinary('surfaces a low-confidence fix_issue decision as escalate', async () => {
    choice = 'keep_working'
    confidence = 0.1
    offScope = 0.1
    const program = await load('examples/fix_issue.jev', { bin: binary, cwd: repoRoot })
    const run = program.task('main').start({
      inputs: { issue: { title: 'bug', body: 'fix it', branch: 'fix' } },
      bind: { claude: new Agent(), tree: new Tree(), me: person },
      model: 'test',
      profiles: join(scratch, 'profiles.json'),
    })
    let pause = await run.next()
    expect(pause.kind).toBe('waiting')
    pause = await run.next()
    expect(pause.kind).toBe('escalate')
    await run.abort()
    await program.close()
  })

  itWithBinary('runs a standalone judgment against a local profile endpoint', async () => {
    const program = await load('examples/inbox_triage.jev', { bin: binary, cwd: repoRoot })
    const answers = await program.judgment('triage').run(
      { message: 'please fix this bug' },
      { model: 'test', profiles: join(scratch, 'profiles.json') },
    )
    expect(answers['owner']).toMatchObject({ $jev: 'choice' })
    await program.close()
  })

  itWithBinary('applies module roots at load time', async () => {
    const library = join(scratch, 'library.jev')
    await writeFile(library, 'program library\n\ndef ok():\n  return true\n')
    const source = 'program rooted\n\nuse "library.jev" as lib\n\ntask main:\n  return lib.ok()\n'
    const program = await load(source, { source: true, paths: [scratch], bin: binary, cwd: repoRoot })
    expect(program.name).toBe('rooted')
    await program.close()
  })

  itWithBinary('queues concurrent run requests during an overlapping adapter reply id', async () => {
    // Spec 10.5 and 11.5: execution stays sequential, but a request already
    // accepted on the pipe must survive a capability round-trip. Both sides
    // deliberately use id 5 at the same time here.
    let slowCalls = 0
    let fastCalls = 0
    let releaseFifth: (() => void) | undefined
    const fifthStarted = new Promise<void>((resolve) => {
      releaseFifth = resolve
    })
    const slow: Adapter = {
      kind: 'tool',
      async call() {
        slowCalls += 1
        if (slowCalls === 5) {
          releaseFifth?.()
          await new Promise((resolve) => setTimeout(resolve, 30))
        }
        return null
      },
    }
    const fast: Adapter = {
      kind: 'tool',
      call() {
        fastCalls += 1
        return null
      },
    }
    const source = `program concurrent

needs slow: tool

task main:
  slow.one
  slow.two
  slow.three
  slow.four
  slow.five
`
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const first = program.task('main').start({ bind: { slow } })
    const second = program.task('main').start({ bind: { slow: fast } })
    await Promise.all([first.id(), second.id()])

    const firstPause = first.next()
    await fifthStarted
    const secondPause = second.next()
    const [a, b] = await Promise.all([firstPause, secondPause])
    expect(a.kind).toBe('done')
    expect(b.kind).toBe('done')
    expect(slowCalls).toBe(5)
    expect(fastCalls).toBe(5)
    await program.close()
  })

  itWithBinary('aborts after an in-flight call and replays the stopped boundary', async () => {
    let announceFirst: (() => void) | undefined
    let releaseFirst: (() => void) | undefined
    const firstStarted = new Promise<void>((resolve) => {
      announceFirst = resolve
    })
    const firstMayReturn = new Promise<void>((resolve) => {
      releaseFirst = resolve
    })
    const calls: string[] = []
    const tool: Adapter = {
      kind: 'tool',
      async call(verb) {
        calls.push(verb)
        if (verb === 'one') {
          announceFirst?.()
          await firstMayReturn
        }
        return null
      },
    }
    const source = `program abort_inflight

needs tree: tool

task main:
  tree.one
  tree.two
`
    const recording = join(scratch, 'abort.jsonl')
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const run = program.task('main').start({ bind: { tree: tool }, record: recording })
    const next = run.next()
    await firstStarted
    await run.abort()
    releaseFirst?.()
    await expect(next).resolves.toMatchObject({ kind: 'stopped', reason: 'aborted by host' })
    expect(calls).toEqual(['one'])

    const events = []
    for await (const event of run.events()) events.push(event.event)
    expect(events.indexOf('call')).toBeLessThan(events.indexOf('abort'))
    expect(events.indexOf('abort')).toBeLessThan(events.indexOf('pause'))
    expect(events.indexOf('pause')).toBeLessThan(events.indexOf('end'))

    const callsAfterLive = calls.length
    const replayed = []
    for await (const pause of program.task('main').start({ replay: recording })) {
      replayed.push(pause)
    }
    expect(replayed).toHaveLength(1)
    expect(replayed[0]).toMatchObject({ kind: 'stopped', reason: 'aborted by host' })
    expect(calls).toHaveLength(callsAfterLive)
    await program.close()
  })

  itWithBinary('preserves adapter retryability and streams the pause before the reply', async () => {
    const retryable = Object.assign(new Error('try later'), { retryable: true })
    const failing: Adapter = {
      kind: 'tool',
      call(_verb, _args, capability) {
        expect(capability).toBe('tree')
        throw retryable
      },
    }
    const source = 'program retry\n\nneeds tree: tool\n\ntask main:\n  tree.fail\n'
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const run = program.task('main').start({ bind: { tree: failing } })
    let sawCall = false
    const eventBeforePause = (async () => {
      for await (const event of run.events()) {
        if (event.event === 'pause') return sawCall
        if (event.event === 'call') sawCall = true
      }
      return sawCall
    })()
    const next = run.next()
    const first = await Promise.race([
      eventBeforePause.then((sawSuccessfulCall) => ({ kind: 'event', sawSuccessfulCall })),
      next.then((pause) => ({ kind: 'reply', pause })),
    ])
    expect(first).toEqual({ kind: 'event', sawSuccessfulCall: false })
    const pause = await next
    expect(pause).toMatchObject({ kind: 'error', code: 'adapter_error', retryable: true })
    expect(await eventBeforePause).toBe(false)
    // Failed calls surface directly as error pauses; there is no successful
    // call event to invent, and the pause notification still precedes reply.
    await run.abort()
    await program.close()
  })

  itWithBinary('ends iteration on default terminals and continues a resumed escalation', async () => {
    const source = `program terminals

out continued

task main:
  escalate "help"
  continued = true
`
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const unresumed = []
    for await (const pause of program.task('main').start()) unresumed.push(pause.kind)
    expect(unresumed).toEqual(['escalate'])

    const resumed = []
    const resumedRun = program.task('main').start()
    for await (const pause of resumedRun) {
      resumed.push(pause.kind)
      if (pause.kind === 'escalate') await resumedRun.resume({ resume: true })
    }
    expect(resumed).toEqual(['escalate', 'done'])
    await program.close()

    const failing: Adapter = {
      kind: 'tool',
      call() {
        throw new Error('permanent')
      },
    }
    const errorProgram = await load(
      'program error_terminal\n\nneeds tree: tool\n\ntask main:\n  tree.fail\n',
      { source: true, bin: binary, cwd: repoRoot },
    )
    const errors = []
    for await (const pause of errorProgram.task('main').start({ bind: { tree: failing } })) {
      errors.push(pause.kind)
    }
    expect(errors).toEqual(['error'])
    await errorProgram.close()
  })

  itWithBinary('awaits asynchronous adapter bind hooks before task.start', async () => {
    let ready = false
    const adapter: Adapter = {
      kind: 'tool',
      async bind() {
        await new Promise((resolve) => setTimeout(resolve, 20))
        ready = true
      },
      call() {
        if (!ready) throw new Error('called before bind completed')
        return null
      },
    }
    const source = 'program async_bind\n\nneeds tree: tool\n\ntask main:\n  tree.touch\n'
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    await expect(program.task('main').start({ bind: { tree: adapter } }).next()).resolves.toMatchObject({
      kind: 'done',
    })
    await program.close()
  })

  itWithBinary('runs and replays review_loop with verified machine completion', async () => {
    machineChoices = ['finished', 'approved']
    const calls: string[] = []
    const agent: Adapter = {
      kind: 'agent',
      call(verb, _args, capability) {
        calls.push(`${capability}.${verb}`)
        if (verb === 'spawn') return { id: 'reviewer-1' }
        if (verb === 'wait') {
          return { status: 'waiting', last_message: 'ready', tail: '', exit_code: null }
        }
        return null
      },
      observe(_handle, capability) {
        calls.push(`${capability}.observe`)
        return { status: 'waiting', last_message: 'ready', tail: '', exit_code: null }
      },
    }
    const tree: Adapter = {
      kind: 'tool',
      call(verb, _args, capability) {
        calls.push(`${capability}.${verb}`)
        if (verb === 'tests_pass') return true
        if (verb === 'test_summary') return 'passing'
        return null
      },
    }
    const me: Adapter = {
      kind: 'person',
      call(verb, _args, capability) {
        calls.push(`${capability}.${verb}`)
        return null
      },
    }
    const recording = join(scratch, 'review.jsonl')
    const program = await load('examples/review_loop.jev', { bin: binary, cwd: repoRoot })
    const run = program.task('main').start({
      bind: { claude: agent, tree, me },
      record: recording,
      model: 'test',
      profiles: join(scratch, 'profiles.json'),
    })
    const pauses = []
    for await (const pause of run) pauses.push(pause)
    expect(pauses.at(-1)).toMatchObject({ kind: 'done' })
    const events = []
    for await (const event of run.events()) events.push(event)
    const machineSteps = events.filter((event) => event.event === 'machine_step')
    expect(machineSteps).toHaveLength(2)
    expect(machineSteps.at(-1)).toMatchObject({ chosen: 'approved', to: 'approved' })
    expect(calls.at(-1)).toBe('me.notify')

    const callsAfterLive = calls.length
    const requestsAfterLive = jevRequests
    const replay = program.task('main').start({ replay: recording })
    const replayed = []
    for await (const pause of replay) replayed.push(pause)
    expect(replayed).toEqual(pauses)
    expect(calls).toHaveLength(callsAfterLive)
    expect(jevRequests).toBe(requestsAfterLive)
    await program.close()
  })

  const logSource =
    'program logs\n\nin message: text\nout n\n\ntask main:\n' +
    '  log info "routed request" { message }\n' +
    '  n = log debug len(message)\n'

  itWithBinary('hands each log to onLog and a replay does not emit them again', async () => {
    // Spec 5.8: every log is a typed line the host can route anywhere; a
    // replay checks the recorded lines without emitting them to the host.
    const recording = join(scratch, 'logs.jsonl')
    const seen: LogEvent[] = []
    const program = await load(logSource, { source: true, bin: binary, cwd: repoRoot })
    const run = program.task('main').start({
      inputs: { message: 'hello' },
      record: recording,
      onLog: (line) => seen.push(line),
    })
    const pauses = []
    for await (const pause of run) pauses.push(pause)
    expect(pauses.at(-1)).toMatchObject({ kind: 'done', outputs: { n: 5 } })
    expect(seen.map((line) => [line.level, line.message])).toEqual([
      ['info', 'routed request'],
      ['debug', '5'],
    ])
    expect(seen[0]?.fields).toEqual({ message: 'hello' })
    expect(seen[1]?.fields).toEqual({})
    expect(seen.every((line) => line.task === 'main' && line.run_id !== undefined)).toBe(true)
    expect(seen[0]?.source.start.line).toBe(7)
    const streamed: LogEvent[] = []
    for await (const line of run.logs()) streamed.push(line)
    expect(streamed).toEqual(seen)

    const replayed: LogEvent[] = []
    const replay = program.task('main').start({
      replay: recording,
      onLog: (line) => replayed.push(line),
    })
    for await (const pause of replay) expect(pause.kind).toBe('done')
    expect(replayed).toEqual([])
    const replayLogs: LogEvent[] = []
    for await (const line of replay.logs()) replayLogs.push(line)
    expect(replayLogs).toEqual([])
    await program.close()
  })

  itWithBinary('hands a standalone judgment\'s logs to onLog', async () => {
    // Spec 5.8 and 11.3: no run and no recording, so the lines come back
    // with the answers, named after the judgment.
    const source =
      'program triage\n\njudgment classify(message):\n' +
      '  urgent = message feels "is urgent"\n' +
      '  log warn "answered" { urgent }\n'
    const program = await load(source, { source: true, bin: binary, cwd: repoRoot })
    const seen: LogEvent[] = []
    const answers = await program.judgment('classify').run(
      { message: 'now' },
      {
        model: 'test',
        profiles: join(scratch, 'profiles.json'),
        onLog: (line) => seen.push(line),
      },
    )
    expect(answers['urgent']).toEqual({ $jev: 'prob', value: 0.9 })
    expect(seen).toHaveLength(1)
    expect(seen[0]).toMatchObject({
      level: 'warn',
      message: 'answered',
      task: 'classify',
      fields: { urgent: { $jev: 'prob', value: 0.9 } },
    })
    expect(seen[0]?.run_id).toBeUndefined()
    await program.close()
  })

  it('reads a log line off a recording event and lists the levels in order', () => {
    // Spec 5.8: debug, info, warn, error; any other event is not a log.
    expect(LOG_LEVELS).toEqual(['debug', 'info', 'warn', 'error'])
    const source = { start: { line: 1, column: 0, offset: 0 }, end: { line: 1, column: 5, offset: 5 } }
    expect(
      logEvent({ ts: '', run_id: 'r', seq: 4, event: 'log', level: 'error', message: 'm', task: 'main', source }),
    ).toEqual({ level: 'error', message: 'm', fields: {}, task: 'main', source, run_id: 'r', seq: 4 })
    expect(logEvent({ ts: '', run_id: 'r', seq: 1, event: 'call' })).toBeUndefined()
  })
})
