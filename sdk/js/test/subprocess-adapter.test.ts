/** The actual JSONL process boundary shared with the CLI (spec section 11.6). */
import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

import { SubprocessAdapterError, load, subprocessAgent } from '../src/index.ts'

const fixture = fileURLToPath(new URL('./fixtures/agent-adapter.mjs', import.meta.url))
const root = fileURLToPath(new URL('../../../', import.meta.url))
const binary = join(root, 'target/debug/jevscript')

describe('subprocessAgent', () => {
  it('serializes calls, forwards capability and observes a reattachable handle', async () => {
    const adapter = subprocessAgent(process.execPath, [fixture])
    adapter.capability = 'worker'
    try {
      const [handle, sent] = await Promise.all([
        adapter.call('spawn', { named: { prompt: 'hello' } }),
        adapter.call('send', { positional: [{ id: 'fake-agent-1' }, 'more'] }),
      ])
      expect(handle).toEqual({ id: 'fake-agent-1', capability: 'worker' })
      expect(sent).toMatchObject({ verb: 'send', capability: 'worker' })
      expect(await adapter.observe(handle as { id: string; capability: string })).toMatchObject({
        status: 'waiting', id_seen: 'fake-agent-1', last_message: 'finished',
      })
      await expect(adapter.call('fail', {})).rejects.toMatchObject({ retryable: true, message: 'try later' })
      expect(await adapter.call('stop', {})).toBeNull()
    } finally {
      await adapter.close()
    }
    await expect(adapter.call('stop', {})).rejects.toBeInstanceOf(Error)
    expect(new SubprocessAdapterError('x', true).retryable).toBe(true)
  })

  it.skipIf(!existsSync(binary))('forwards spawn, observe and stop through the runtime', async () => {
    const adapter = subprocessAgent(process.execPath, [fixture])
    const program = await load(join(root, 'adapters/core/test/fixtures/sdk_subprocess.jev'), { bin: binary })
    try {
      const pause = await program.task('main').start({ bind: { dev: adapter } }).next()
      expect(pause).toMatchObject({ kind: 'done', outputs: { message: 'finished' } })
    } finally {
      await program.close()
      await adapter.close()
    }
  })
})
