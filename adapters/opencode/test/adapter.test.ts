/** OpenCode through the spec 9.1 verbs, using a fake terminal backend. */
import { describe, expect, it } from 'vitest'
import { FakeBackend, FakeClock, FakeFiles } from '@jevscript/adapter-core/testing'
import { OpenCodeAdapter, opencode } from '../src/index.ts'

describe('opencode', () => {
  it('launches its CLI, observes its handle, sends safely and stops', async () => {
    const backend = new FakeBackend()
    const clock = new FakeClock()
    const adapter = new OpenCodeAdapter({ backend, clock, files: new FakeFiles(), home: '/fake', cwd: '/tmp', bin: '/fake/opencode', pollMs: 1000 })
    const handle = await adapter.call('spawn', { named: { prompt: 'A trivial task', model: 'test-model', effort: 'low' } }, 'worker') as { id: string; capability: string; pane: string }
    expect(adapter.harness).toBe(opencode)
    expect(handle.capability).toBe('worker')
    expect(handle.id).toBeTruthy()
    const pane = backend.pane(handle)
    expect(pane.spec.argv[0]).toBe('/fake/opencode')
    expect(pane.spec.argv).toContain('test-model')
    expect(pane.spec.argv).toContain('--prompt')
    expect(pane.spec.argv).toContain('A trivial task')
    pane.screen = 'visible output'
    expect((await adapter.observe(handle)).tail).toContain('visible output')
    await adapter.call('send', { positional: [handle, 'one\ntwo'] })
    expect(pane.input).toContainEqual({ type: 'paste', text: 'one\ntwo' })
    expect(pane.input).toContainEqual({ type: 'key', key: 'Enter' })
    await adapter.call('stop', { positional: [handle] })
    expect(backend.panes.has(handle.pane)).toBe(false)
  })
})
