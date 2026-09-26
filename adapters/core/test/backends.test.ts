/**
 * Each terminal backend against a scripted `exec`: the exact commands it runs
 * and how it reads their replies. Each test names the backend behavior it
 * checks.
 */
import { describe, expect, it } from 'vitest'

import {
  CmuxBackend,
  compareVersions,
  type ExecResult,
  HerdrBackend,
  launchScript,
  OrcaBackend,
  TmuxBackend,
  ZellijBackend,
} from '../src/index.ts'
import { FakeClock, FakeFiles } from '../src/testing.ts'

type Reply = ExecResult | string | Record<string, unknown> | unknown[]

/** An `exec` that records every call and answers from a function of its argv. */
function scripted(answer: (args: string[]) => Reply | undefined) {
  const calls: { file: string; args: string[]; env?: Record<string, string> | undefined }[] = []
  const exec = async (file: string, args: string[], options?: { env?: Record<string, string> }) => {
    calls.push({ file, args, env: options?.env })
    const reply = answer(args)
    if (reply === undefined) return { code: 0, stdout: '', stderr: '' }
    if (typeof reply === 'string') return { code: 0, stdout: reply, stderr: '' }
    if ('code' in reply && 'stdout' in reply) return reply as ExecResult
    return { code: 0, stdout: JSON.stringify(reply), stderr: '' }
  }
  return { calls, exec }
}

const spec = {
  cwd: '/work/repo',
  argv: ['codex', '--model', 'gpt', "fix it 'now'"],
  env: { OMP_SKIP_SETUP: '1' },
  unset: ['CLAUDECODE'],
  title: 'jev-codex-12345678',
}

describe('launch scripts', () => {
  it('sets the environment, runs the argv word by word and records the exit status', () => {
    // backends without a pane exit status learn it from this file (spec 9.1 `exit_code`).
    const script = launchScript(spec, '/state/x/exit')
    expect(script).toContain("cd '/work/repo' ||")
    expect(script).toContain("unset 'CLAUDECODE'")
    expect(script).toContain("export OMP_SKIP_SETUP='1'")
    expect(script).toContain(`'codex' '--model' 'gpt' 'fix it '\\''now'\\'''`)
    expect(script.trim().endsWith(`printf '%s\\n' "$?" > '/state/x/exit'`)).toBe(true)
  })

  it('never writes an environment name that is not a shell identifier', () => {
    expect(launchScript({ ...spec, env: { 'BAD NAME': 'x', GOOD: 'y' } }, '/e')).not.toContain('BAD')
  })
})

describe('herdr', () => {
  function herdr(env: Record<string, string | undefined> = { HERDR_ENV: '1', HERDR_PANE_ID: 'w1:p1' }) {
    const files = new FakeFiles()
    const { calls, exec } = scripted((args) => {
      const [noun, verb, target] = args
      if (noun === 'pane' && verb === 'get' && target === 'w1:p1') {
        return { result: { pane: { pane_id: 'w1:p1', workspace_id: 'w1', tab_id: 'w1:t1' } } }
      }
      if (noun === 'tab' && verb === 'create') {
        return { result: { root_pane: { pane_id: 'w1:p9' }, tab: { tab_id: 'w1:t9' } } }
      }
      if (noun === 'workspace' && verb === 'create') return { result: { workspace: { workspace_id: 'w5' } } }
      if (noun === 'pane' && verb === 'get' && target === 'w1:p9') return { result: { pane: { pane_id: 'w1:p9' } } }
      if (noun === 'pane' && verb === 'get') {
        return { code: 1, stdout: '{"error":{"code":"pane_not_found","message":"gone"}}', stderr: '' }
      }
      if (noun === 'pane' && verb === 'read') return 'screen text\n'
      if (noun === 'agent') return { result: { agent: { agent: 'codex', agent_status: 'working' } } }
      if (noun === 'status') return { server: { running: true } }
      return undefined
    })
    const backend = new HerdrBackend({ exec, files, clock: new FakeClock(), stateDir: '/state', env })
    return { backend, calls, files }
  }

  it('opens an unfocused tab in the workspace of the host pane, read live, and runs the launch script', async () => {
    // herdr.sh: tab create --no-focus in the live workspace; the injected HERDR_WORKSPACE_ID goes stale.
    const { backend, calls, files } = herdr()
    const ref = await backend.create(spec)
    expect(calls[0]?.args).toEqual(['pane', 'get', 'w1:p1'])
    expect(calls[1]?.args).toEqual([
      'tab', 'create', '--workspace', 'w1', '--cwd', '/work/repo', '--label', 'jev-codex-12345678', '--no-focus',
    ])
    const run = calls[2]?.args ?? []
    expect(run.slice(0, 3)).toEqual(['pane', 'run', 'w1:p9'])
    expect(run[3]).toMatch(/^sh '\/state\/[0-9a-f]{16}\/launch\.sh'$/)
    expect(ref).toMatchObject({ backend: 'herdr', pane: 'w1:p9', tab: 'w1:t9', workspace: 'w1' })
    expect(ref['exit_file']).toMatch(/^\/state\/[0-9a-f]{16}\/exit$/)
    const script = [...files.files.entries()].find(([path]) => path.endsWith('launch.sh'))
    expect(script?.[1]).toContain("'codex' '--model' 'gpt'")
    expect(files.modes.get(script?.[0] ?? '')).toBe(0o700)
  })

  it('creates one workspace of its own, once, when the host is not inside Herdr', async () => {
    const { backend, calls } = herdr({})
    await backend.create(spec)
    await backend.create(spec)
    expect(calls.filter((call) => call.args[0] === 'workspace').map((call) => call.args)).toEqual([
      ['workspace', 'create', '--cwd', '/work/repo', '--label', 'jevscript', '--no-focus'],
    ])
    expect(calls.find((call) => call.args[0] === 'tab')?.args).toContain('w5')
  })

  it('puts --session before any -- so it stays a Herdr option', async () => {
    // Keep the Herdr session option outside the launch command's arguments.
    const { calls, exec } = scripted(() => ({ result: { root_pane: { pane_id: 'w1:p9' }, workspace: { workspace_id: 'w2' } } }))
    const backend = new HerdrBackend({ exec, files: new FakeFiles(), clock: new FakeClock(), session: 'lab', env: {} })
    await backend.create(spec)
    for (const call of calls) {
      expect(call.args.slice(-2)).toEqual(['--session', 'lab'])
      expect(call.env).toEqual({ HERDR_SESSION: 'lab' })
    }
  })

  it('reports a pane that is gone as exited, and an exit file as the exit status', async () => {
    const { backend, files } = herdr()
    expect(await backend.state({ backend: 'herdr', pane: 'w1:gone' })).toEqual({ exists: false, exited: true, exitCode: null })
    expect(await backend.state({ backend: 'herdr', pane: 'w1:p9', exit_file: '/state/a/exit' })).toEqual({
      exists: true,
      exited: false,
      exitCode: null,
    })
    await files.write('/state/a/exit', '3\n')
    expect(await backend.state({ backend: 'herdr', pane: 'w1:p9', exit_file: '/state/a/exit' })).toEqual({
      exists: true,
      exited: true,
      exitCode: 3,
    })
  })

  it('reads the visible screen, maps keys to Herdr names and reads the native agent status', async () => {
    const { backend, calls } = herdr()
    const ref = { backend: 'herdr', pane: 'w1:p9' }
    expect(await backend.capture(ref)).toBe('screen text\n')
    expect(calls.at(-1)?.args).toEqual(['pane', 'read', 'w1:p9', '--source', 'visible'])
    for (const key of ['Enter', 'Escape', 'C-c', 'C-u'] as const) await backend.key(ref, key)
    expect(calls.slice(-4).map((call) => call.args[3])).toEqual(['enter', 'esc', 'ctrl+c', 'ctrl+u'])
    await backend.type(ref, 'hello')
    expect(calls.at(-1)?.args).toEqual(['pane', 'send-text', 'w1:p9', 'hello'])
    expect(await backend.nativeStatus(ref)).toBe('working')
    await backend.close(ref)
    expect(calls.at(-1)?.args).toEqual(['pane', 'close', 'w1:p9'])
  })

  it('probes read-only: status --json, never a server or session command', async () => {
    const { backend, calls } = herdr()
    const probe = await backend.probe()
    if (probe.installed) expect(probe.ready).toBe(true)
    for (const call of calls) expect(['server', 'session']).not.toContain(call.args[0])
  })
})

describe('tmux', () => {
  it('clears inherited markers with env -u and passes the environment with -e', async () => {
    const { calls, exec } = scripted((args) => (args[0] === 'new-window' ? '%3\n' : args[0] === 'has-session' ? { code: 1, stdout: '', stderr: "can't find session" } : undefined))
    const ref = await new TmuxBackend({ exec, session: 's' }).create(spec)
    expect(ref).toEqual({ session: 's', pane: '%3' })
    const respawn = calls.find((call) => call.args[0] === 'respawn-pane')?.args ?? []
    expect(respawn).toContain('OMP_SKIP_SETUP=1')
    expect(respawn.at(-1)).toBe(`'env' '-u' 'CLAUDECODE' 'codex' '--model' 'gpt' 'fix it '\\''now'\\'''`)
  })
})

describe('cmux', () => {
  it('creates an unfocused workspace running the launch script and finds it by its unique title', async () => {
    const { calls, exec } = scripted((args) => {
      if (args[0] === 'workspace') {
        const title = calls.find((call) => call.args[0] === 'new-workspace')?.args[2]
        return { workspaces: [{ id: 'other', title: 'x' }, { id: 'ws-1', title }] }
      }
      if (args[0] === 'list-panes') return { panes: [{ selected_surface_id: 'sf-1', surface_ids: ['sf-1'] }] }
      return undefined
    })
    const backend = new CmuxBackend({ exec, files: new FakeFiles(), stateDir: '/state' })
    const ref = await backend.create(spec)
    const create = calls[0]?.args ?? []
    expect(create[0]).toBe('new-workspace')
    expect(create).toContain('--focus')
    expect(create[create.indexOf('--focus') + 1]).toBe('false')
    expect(create[create.indexOf('--command') + 1]).toMatch(/^sh '\/state\/.*\/launch\.sh'$/)
    expect(calls[0]?.env).toMatchObject({ CMUX_QUIET: '1' })
    expect(ref).toMatchObject({ backend: 'cmux', workspace: 'ws-1', surface: 'sf-1' })

    await backend.type(ref, 'one\ntwo')
    expect(calls.at(-1)?.args).toEqual(['send', '--workspace', 'ws-1', '--surface', 'sf-1', '--', 'one two'])
    await backend.key(ref, 'C-c')
    expect(calls.at(-1)?.args).toEqual(['send-key', '--workspace', 'ws-1', '--surface', 'sf-1', 'ctrl-c'])
    await backend.paste(ref, 'a\nb')
    expect(calls.slice(-2).map((call) => call.args[0])).toEqual(['set-buffer', 'paste-buffer'])
  })

  it('says why it is not ready when the socket refuses outside processes', async () => {
    const { exec } = scripted((args) =>
      args[0] === 'ping' ? { code: 1, stdout: '', stderr: 'Error: only processes started inside cmux can connect' } : 'cmux 0.64.25 (106)',
    )
    const probe = await new CmuxBackend({ exec, bin: process.execPath }).probe()
    expect(probe).toMatchObject({ ready: false, version: '0.64.25' })
    expect(probe.detail).toMatch(/Automation/)
  })
})

describe('orca', () => {
  it('creates a terminal in the worktree of the cwd and reads the rendered screen', async () => {
    const { calls, exec } = scripted((args) => {
      if (args[1] === 'create') return { ok: true, result: { terminal: { handle: 'term-7' } } }
      if (args[1] === 'read') return { ok: true, result: { terminal: { tail: ['line 1', 'line 2'] } } }
      return { ok: true, result: {} }
    })
    const backend = new OrcaBackend({ exec, files: new FakeFiles(), stateDir: '/state' })
    const ref = await backend.create(spec)
    expect(calls[0]?.args.slice(0, 4)).toEqual(['terminal', 'create', '--worktree', 'path:/work/repo'])
    expect(ref).toMatchObject({ backend: 'orca', terminal: 'term-7' })
    expect(await backend.capture(ref)).toBe('line 1\nline 2')
    expect(calls.at(-1)?.args).toContain('--screen')
    await backend.key(ref, 'Enter')
    expect(calls.at(-1)?.args).toEqual(['terminal', 'send', '--terminal', 'term-7', '--text', '', '--enter', '--json'])
    await backend.key(ref, 'C-c')
    expect(calls.at(-1)?.args).toContain('--interrupt')
    // orca.sh: "Escape is not supported".
    await expect(backend.key(ref, 'Escape')).rejects.toMatchObject({ retryable: false })
  })
})

describe('zellij', () => {
  it('opens a tab, refocuses the tab that was active and types the launch line into the pane', async () => {
    const { calls, exec } = scripted((args) => {
      const action = args[3]
      if (args[0] === 'list-sessions') return 'jevscript\n'
      if (action === 'list-tabs') return [{ tab_id: 0, active: true }]
      if (action === 'new-tab') return '4\n'
      if (action === 'list-panes') return [{ id: 0, tab_id: 4, is_plugin: true }, { id: 2, tab_id: 4, is_plugin: false }]
      return undefined
    })
    const backend = new ZellijBackend({ exec, files: new FakeFiles(), stateDir: '/state' })
    const ref = await backend.create(spec)
    expect(ref).toMatchObject({ backend: 'zellij', zellij_session: 'jevscript', pane: 2, tab: 4 })
    const actions = calls.filter((call) => call.args[2] === 'action').map((call) => call.args.slice(3))
    expect(actions[1]).toEqual(['new-tab', '--cwd', '/work/repo', '--name', 'jev-codex-12345678'])
    expect(actions[3]).toEqual(['go-to-tab-by-id', '0'])
    expect(actions[4]?.slice(0, 4)).toEqual(['paste', '--pane-id', '2', '--'])
    expect(actions[5]).toEqual(['send-keys', '--pane-id', '2', 'Enter'])
    // zellij.sh: `Ctrl c` is one argument with a space.
    await backend.key(ref, 'C-c')
    expect(calls.at(-1)?.args.slice(3)).toEqual(['send-keys', '--pane-id', '2', 'Ctrl c'])
  })

  it('is not ready below 0.44, which lacks --pane-id', async () => {
    const { exec } = scripted(() => 'zellij 0.43.1')
    expect(await new ZellijBackend({ exec, bin: process.execPath }).probe()).toMatchObject({ ready: false, version: '0.43.1' })
    expect(compareVersions('0.44.0', '0.43.1')).toBeGreaterThan(0)
  })
})
