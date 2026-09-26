/**
 * The adapter against a real tmux server, with a shell script standing in for
 * Claude Code. Skipped when tmux is not on the PATH.
 *
 * What it checks: `spawn` starts the command in a pane of the adapter's own
 * session, `send` delivers one line typed and two lines pasted as one message,
 * `wait idle` returns once the screen settles, and `stop` leaves no pane.
 */
import { spawnSync } from 'node:child_process'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterAll, describe, expect, it } from 'vitest'

import { ClaudeCodeAdapter, type PaneHandle } from '../src/index.ts'

const hasTmux = spawnSync('tmux', ['-V']).status === 0
const describeWithTmux = hasTmux ? describe : describe.skip

const session = `jevscript-test-${process.pid}`
const agent = fileURLToPath(new URL('./fixtures/agent.sh', import.meta.url))

describeWithTmux('a pane in a real tmux server', () => {
  afterAll(() => {
    spawnSync('tmux', ['kill-session', '-t', `=${session}`])
  })

  it('spawns, sends, observes, waits idle and stops', async () => {
    const adapter = new ClaudeCodeAdapter({
      capability: 'dev',
      session,
      bin: agent,
      // No transcript will ever appear here, so status stays `running` until the pane dies.
      configDir: mkdtempSync(join(tmpdir(), 'jevscript-adapter-')),
      idleSeconds: 1,
      pollMs: 200,
    })

    const handle = (await adapter.call('spawn', { named: { prompt: 'hello agent' } })) as PaneHandle
    expect(handle).toMatchObject({ capability: 'dev', session, cwd: process.cwd() })
    expect(handle.pane).toMatch(/^%\d+$/)
    expect(handle.session_id).toMatch(/^[0-9a-f-]{36}$/)

    await adapter.call('send', { positional: [handle, 'first line'] })
    await adapter.call('send', { positional: [handle, 'second line\nthird line'] })

    const settled = await adapter.call('wait', { positional: [handle, 'idle'], named: { minutes: 1 } })
    expect(settled).toMatchObject({ status: 'running', waited: 'idle', exit_code: null, last_message: '' })
    const tail = (settled as { tail: string }).tail
    expect(tail).toContain(`--session-id ${handle.session_id}`)
    expect(tail).toContain('hello agent')
    // `cat` echoes each line it was sent, so both messages arrived whole.
    expect(tail).toMatch(/first line[\s\S]*second line[\s\S]*third line/)

    await adapter.call('stop', { positional: [handle] })
    await adapter.call('stop', { positional: [handle] })
    expect(await adapter.observe(handle)).toMatchObject({ status: 'exited', exit_code: null, tail: '' })
  })
})
