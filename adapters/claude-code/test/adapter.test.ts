/**
 * The adapter against a fake tmux, a fixture transcript and a fake clock.
 *
 * Each test names the rule of spec section 9.1 (or the README) it checks. The
 * fixture transcript is hand-written in the shape Claude Code 2.1 emits: one
 * line per content block, lines of one message sharing `message.id`.
 */
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import {
  AdapterError,
  boundTail,
  ClaudeCodeAdapter,
  type Clock,
  type ExecResult,
  type PaneHandle,
  parseTranscript,
  promptVisible,
  TAIL_LIMIT,
  transcriptPath,
} from '../src/index.ts'

const fixture = readFileSync(fileURLToPath(new URL('./fixtures/transcript.jsonl', import.meta.url)), 'utf8')
const fixtureLines = fixture.split('\n').filter((line) => line !== '')
const SESSION_ID = '0f8e9c2a-5b1d-4c3e-8a7f-2d6b4e1c9a05'

const IDLE_SCREEN = [
  '❯ Fix the failing test in src/lib.rs',
  '',
  '⏺ Fixed and green now.',
  '',
  '────────',
  '❯ ',
  '────────',
  '  ~/work/repo | main | Fable 5.1',
].join('\n')

const BUSY_SCREEN = IDLE_SCREEN.replace('❯ ', '✻ Crunching… (esc to interrupt)')

/** Enough of tmux to run every verb: sessions, panes, screens and buffers. */
class FakeTmux {
  sessions = new Set<string>()
  panes = new Map<string, { dead: boolean; status: string; screen: string; cwd: string; command: string }>()
  buffers = new Map<string, string>()
  calls: string[][] = []
  next = 1
  /** When set, answers every call instead of the fake server: for failures. */
  override: ((file: string, args: string[]) => Promise<ExecResult>) | null = null
  /** When set, is what `capture-pane` shows: for screens that keep changing. */
  frame: (() => string) | null = null

  exec = async (file: string, args: string[]): Promise<ExecResult> => {
    expect(file).toBe('tmux')
    this.calls.push(args)
    if (this.override) return this.override(file, args)
    const ok = (stdout = ''): ExecResult => ({ code: 0, stdout, stderr: '' })
    const fail = (stderr: string): ExecResult => ({ code: 1, stdout: '', stderr })
    const target = (): string => args[args.indexOf('-t') + 1] ?? ''
    switch (args[0]) {
      case 'has-session':
        return this.sessions.has(target().replace(/^=/, '')) ? ok() : fail(`can't find session: ${target()}`)
      case 'new-session':
        this.sessions.add(args[args.indexOf('-s') + 1] ?? '')
        return ok()
      case 'new-window': {
        const id = `%${this.next++}`
        this.panes.set(id, { dead: false, status: '', screen: '', cwd: args[args.indexOf('-c') + 1] ?? '', command: '' })
        return ok(`${id}\n`)
      }
      case 'set-option':
        return ok()
      case 'respawn-pane': {
        const pane = this.panes.get(target())
        if (!pane) return fail(`can't find pane: ${target()}`)
        pane.command = args.at(-1) ?? ''
        return ok()
      }
      case 'list-panes':
        return ok(
          [...this.panes.entries()]
            .map(([id, pane]) => `${id}\t${pane.dead ? '1' : '0'}\t${pane.status}`)
            .join('\n'),
        )
      case 'capture-pane': {
        const pane = this.panes.get(target())
        return pane ? ok(this.frame ? this.frame() : pane.screen) : fail(`can't find pane: ${target()}`)
      }
      case 'send-keys':
      case 'paste-buffer':
        return this.panes.has(target()) ? ok() : fail(`can't find pane: ${target()}`)
      case 'set-buffer':
        this.buffers.set(args[args.indexOf('-b') + 1] ?? '', args.at(-1) ?? '')
        return ok()
      case 'kill-pane':
        return this.panes.delete(target()) ? ok() : fail(`can't find pane: ${target()}`)
      default:
        return fail(`unknown command: ${args[0]}`)
    }
  }

  /** The calls of one tmux command, without the command name. */
  of(command: string): string[][] {
    return this.calls.filter((call) => call[0] === command).map((call) => call.slice(1))
  }
}

/** A clock that only moves when something sleeps. */
class FakeClock implements Clock {
  time = 1_000_000
  slept: number[] = []
  now(): number {
    return this.time
  }
  async sleep(ms: number): Promise<void> {
    this.slept.push(ms)
    this.time += ms
  }
}

function build(overrides: { transcript?: string | undefined; screen?: string } = {}) {
  const tmux = new FakeTmux()
  const clock = new FakeClock()
  const transcripts = new Map<string, string>()
  const adapter = new ClaudeCodeAdapter({
    capability: 'dev',
    session: 'jev-test',
    bin: 'claude',
    configDir: '/home/me/.claude',
    idleSeconds: 2,
    pollMs: 500,
    exec: tmux.exec,
    clock,
    uuid: () => SESSION_ID,
    readFile: async (path) => transcripts.get(path),
  })
  const handle: PaneHandle = {
    capability: 'dev',
    id: SESSION_ID,
    session: 'jev-test',
    pane: '%7',
    cwd: '/work/repo',
    session_id: SESSION_ID,
  }
  tmux.panes.set('%7', { dead: false, status: '', screen: overrides.screen ?? IDLE_SCREEN, cwd: '/work/repo', command: '' })
  if (overrides.transcript !== undefined) {
    transcripts.set(transcriptPath('/home/me/.claude', '/work/repo', SESSION_ID), overrides.transcript)
  }
  return { adapter, tmux, clock, handle, transcripts }
}

describe('the transcript', () => {
  it('takes last_message from the last complete assistant message with text', () => {
    // 9.1: last_message is "the agent's most recent complete message to the user".
    const summary = parseTranscript(fixture)
    expect(summary.lastMessage).toBe('The test failed because the span was off by one.\n\nFixed and green now.')
    expect(summary.idle).toBe(true)
  })

  it('ignores sidechain lines and counts turns and tokens per message, not per line', () => {
    const summary = parseTranscript(fixture)
    expect(summary.turns).toBe(2)
    expect(summary.tokens).toBe(110)
    expect(summary.usd).toBeNull()
    expect(summary.lastMessage).not.toContain('subagent')
  })

  it('is not idle while a tool use is pending', () => {
    // The tool_use message is complete, but the agent has not handed back control.
    const pending = parseTranscript(fixtureLines.slice(0, 5).join('\n'))
    expect(pending.idle).toBe(false)
    expect(pending.lastMessage).toBe("I'll look at the failing test first.")
  })

  it('is not idle once the tool result is in and the next message has not started', () => {
    expect(parseTranscript(fixtureLines.slice(0, 6).join('\n')).idle).toBe(false)
  })

  it('is not idle after a new user prompt, and keeps the previous last_message', () => {
    const prompt = JSON.stringify({ type: 'user', message: { role: 'user', content: 'Now run clippy' } })
    const summary = parseTranscript(`${fixture}${prompt}\n`)
    expect(summary.idle).toBe(false)
    expect(summary.lastMessage).toContain('Fixed and green now.')
  })

  it('skips a partial line still being written', () => {
    const summary = parseTranscript(`${fixture}{"type":"assistant","message":{"id":"msg_C","content":[{"type":"te`)
    expect(summary.idle).toBe(true)
    expect(summary.turns).toBe(2)
  })

  it('treats a missing transcript as empty', () => {
    expect(parseTranscript(undefined)).toEqual({ lastMessage: '', idle: false, turns: 0, tokens: 0, usd: null })
  })

  it('encodes the working directory the way Claude Code does', () => {
    expect(transcriptPath('/home/me/.claude', '/Users/me/.codex/wt_1', SESSION_ID)).toBe(
      `/home/me/.claude/projects/-Users-me--codex-wt-1/${SESSION_ID}.jsonl`,
    )
  })
})

describe('the screen', () => {
  it('bounds tail to roughly 4k tokens from the end and drops trailing blank lines', () => {
    // 9.1: tail is "the last screen or ~4k tokens".
    const long = Array.from({ length: 2000 }, (_, i) => `line ${i} `).join('\n')
    const tail = boundTail(`${long}\n\n\n`)
    expect(tail.length).toBe(TAIL_LIMIT)
    expect(tail.endsWith('line 1999')).toBe(true)
    expect(boundTail('a  \nb\n\n\n')).toBe('a\nb')
  })

  it('sees the input box only when the last prompt marker is empty', () => {
    expect(promptVisible(IDLE_SCREEN)).toBe(true)
    expect(promptVisible(BUSY_SCREEN)).toBe(false)
    expect(promptVisible('❯ a draft the user typed')).toBe(false)
    expect(promptVisible('> ')).toBe(true)
  })
})

describe('spawn', () => {
  it('creates the session on demand, a pane with the cwd of `in`, and a handle', async () => {
    const { adapter, tmux } = build()
    const handle = (await adapter.call('spawn', {
      named: { prompt: "Fix it 'now'", in: { capability: 'tree', id: 't1', path: '/work/tree' }, model: 'opus' },
    })) as PaneHandle

    expect(handle).toEqual({
      capability: 'dev',
      id: SESSION_ID,
      session: 'jev-test',
      pane: '%1',
      cwd: '/work/tree',
      session_id: SESSION_ID,
    })
    expect(tmux.of('new-session')[0]).toContain('jev-test')
    expect(tmux.of('new-window')[0]).toEqual(['-d', '-P', '-F', '#{pane_id}', '-t', 'jev-test:', '-c', '/work/tree'])
    expect(tmux.of('set-option')[0]).toEqual(['-p', '-t', '%1', 'remain-on-exit', 'on'])
    // The command is one shell word per argument, the prompt last and quoted.
    expect(tmux.panes.get('%1')?.command).toBe(
      `'claude' '--session-id' '${SESSION_ID}' '--model' 'opus' 'Fix it '\\''now'\\'''`,
    )
  })

  it('reuses an existing session and falls back to the process cwd', async () => {
    const { adapter, tmux } = build()
    tmux.sessions.add('jev-test')
    const handle = (await adapter.call('spawn', { named: { prompt: 'hi' } })) as PaneHandle
    expect(tmux.of('new-session')).toEqual([])
    expect(handle.cwd).toBe(process.cwd())
  })

  it('refuses to spawn without a prompt', async () => {
    const { adapter } = build()
    await expect(adapter.call('spawn', { named: {} })).rejects.toMatchObject({
      name: 'AdapterError',
      retryable: false,
    })
  })
})

describe('observe', () => {
  it('is waiting when the transcript is idle and the input box shows', async () => {
    const { adapter, handle } = build({ transcript: fixture })
    const observation = await adapter.observe(handle)
    expect(observation).toMatchObject({
      status: 'waiting',
      last_message: 'The test failed because the span was off by one.\n\nFixed and green now.',
      exit_code: null,
      session_id: SESSION_ID,
      turns: 2,
      usage: { tokens: 110, usd: null },
    })
    expect(observation.tail).toBe(boundTail(IDLE_SCREEN))
  })

  it('is running while the screen shows no input box, whatever the transcript says', async () => {
    const { adapter, handle } = build({ transcript: fixture, screen: BUSY_SCREEN })
    expect((await adapter.observe(handle)).status).toBe('running')
  })

  it('is running while the transcript has a tool use pending, whatever the screen shows', async () => {
    const { adapter, handle } = build({ transcript: fixtureLines.slice(0, 5).join('\n') })
    expect((await adapter.observe(handle)).status).toBe('running')
  })

  it('is running before there is a transcript at all', async () => {
    const { adapter, handle } = build()
    const observation = await adapter.observe(handle)
    expect(observation.status).toBe('running')
    expect(observation.last_message).toBe('')
  })

  it('is exited with the exit code when the pane is dead', async () => {
    const { adapter, tmux, handle } = build({ transcript: fixture })
    tmux.panes.set('%7', { dead: true, status: '130', screen: 'bye', cwd: '/work/repo', command: '' })
    expect(await adapter.observe(handle)).toMatchObject({ status: 'exited', exit_code: 130, tail: 'bye' })
  })

  it('is exited with no exit code when the pane is gone, and still has last_message', async () => {
    const { adapter, tmux, handle } = build({ transcript: fixture })
    tmux.panes.delete('%7')
    expect(await adapter.observe(handle)).toMatchObject({
      status: 'exited',
      exit_code: null,
      tail: '',
      last_message: expect.stringContaining('green'),
    })
  })

  it('rejects a handle another adapter made', async () => {
    const { adapter } = build()
    await expect(adapter.observe({ capability: 'tree', id: 't1' })).rejects.toBeInstanceOf(AdapterError)
  })
})

describe('send', () => {
  it('types one line literally and submits it', async () => {
    const { adapter, tmux, handle } = build()
    await adapter.call('send', { positional: [handle, '-n Tests fail: fix them'] })
    expect(tmux.of('send-keys')).toEqual([
      ['-t', '%7', '-l', '--', '-n Tests fail: fix them'],
      ['-t', '%7', 'Enter'],
    ])
  })

  it('pastes several lines as one message and then submits', async () => {
    const { adapter, tmux, handle } = build()
    await adapter.send(handle, 'first\nsecond')
    expect(tmux.buffers.get('jevscript-%7')).toBe('first\nsecond')
    expect(tmux.of('paste-buffer')).toEqual([['-p', '-d', '-b', 'jevscript-%7', '-t', '%7']])
    expect(tmux.of('send-keys')).toEqual([['-t', '%7', 'Enter']])
  })
})

describe('wait idle', () => {
  it('returns as soon as the status is no longer running', async () => {
    const { adapter, handle, clock } = build({ transcript: fixture })
    const observation = await adapter.call('wait', { positional: [handle, 'idle'], named: { minutes: 5 } })
    expect(observation).toMatchObject({ status: 'waiting', waited: 'status' })
    expect(clock.slept).toEqual([])
  })

  it('returns once the screen has not changed for idleSeconds', async () => {
    const { adapter, handle, clock } = build()
    const observation = await adapter.wait(handle, 5)
    expect(observation).toMatchObject({ status: 'running', waited: 'idle' })
    // idleSeconds is 2 and the poll is 500ms: four polls of no change.
    expect(clock.slept).toEqual([500, 500, 500, 500])
  })

  it('times out after `minutes` while the screen keeps changing', async () => {
    const { adapter, tmux, handle, clock } = build()
    let tick = 0
    tmux.frame = () => `frame ${tick++}`
    const started = clock.now()
    const observation = await adapter.wait(handle, 1)
    expect(observation).toMatchObject({ status: 'running', waited: 'timeout' })
    expect(observation.tail).toMatch(/^frame \d+$/)
    expect(clock.now() - started).toBe(60_000)
  })

  it('knows only the idle condition', async () => {
    const { adapter, handle } = build()
    await expect(adapter.call('wait', { positional: [handle, 'done'] })).rejects.toMatchObject({ retryable: false })
  })
})

describe('stop', () => {
  it('interrupts twice with a gap, then kills the pane', async () => {
    const { adapter, tmux, handle, clock } = build()
    await adapter.call('stop', { positional: [handle] })
    expect(tmux.of('send-keys')).toEqual([
      ['-t', '%7', 'C-c'],
      ['-t', '%7', 'C-c'],
    ])
    expect(clock.slept).toEqual([300, 300])
    expect(tmux.of('kill-pane')).toEqual([['-t', '%7']])
    expect(tmux.panes.has('%7')).toBe(false)
  })

  it('is idempotent: a second stop finds nothing and does nothing', async () => {
    const { adapter, tmux, handle } = build()
    await adapter.stop(handle)
    tmux.calls = []
    await expect(adapter.stop(handle)).resolves.toBeUndefined()
    expect(tmux.of('send-keys')).toEqual([])
    expect(tmux.of('kill-pane')).toEqual([])
  })

  it('does not interrupt a pane that is already dead, only removes it', async () => {
    const { adapter, tmux, handle } = build()
    tmux.panes.set('%7', { dead: true, status: '0', screen: '', cwd: '/work/repo', command: '' })
    await adapter.stop(handle)
    expect(tmux.of('send-keys')).toEqual([])
    expect(tmux.of('kill-pane')).toEqual([['-t', '%7']])
  })
})

describe('errors', () => {
  it('rejects a verb the agent kind does not have', async () => {
    const { adapter } = build()
    await expect(adapter.call('click', {})).rejects.toMatchObject({ name: 'AdapterError', retryable: false })
  })

  it('marks a tmux failure retryable unless the target is gone', async () => {
    const { adapter, tmux, handle } = build()
    tmux.override = async () => ({ code: 1, stdout: '', stderr: 'server exited unexpectedly' })
    await expect(adapter.send(handle, 'x')).rejects.toMatchObject({ retryable: true })
    tmux.override = async () => ({ code: 1, stdout: '', stderr: "can't find pane: %7" })
    await expect(adapter.send(handle, 'x')).rejects.toMatchObject({ retryable: false })
  })
})
