/** Cursor through the spec 9.1 verbs, using a fake terminal backend. */
import { describe, expect, it } from 'vitest'
import { FakeBackend, FakeClock, FakeFiles } from '@jevscript/adapter-core/testing'
import { CursorAdapter, cursor } from '../src/index.ts'

describe('cursor', () => {
  it('accepts the selected workspace trust choice before observing a turn', async () => {
    class TrustBackend extends FakeBackend {
      override async create(spec: Parameters<FakeBackend['create']>[0]) {
        const ref = await super.create(spec)
        this.pane(ref).screen = '⚠ Workspace Trust Required\n▶ [a] Trust this workspace'
        return ref
      }
    }
    const backend = new TrustBackend()
    backend.onKey = (pane, key) => {
      if (key === 'Enter') pane.screen = 'ctrl+c to stop'
    }
    const adapter = new CursorAdapter({ backend, clock: new FakeClock(), files: new FakeFiles(), home: '/fake', cwd: '/tmp', bin: '/fake/cursor' })
    const handle = await adapter.spawn({ named: { prompt: 'A trivial task', trust: true } })
    expect(backend.pane(handle).input).toContainEqual({ type: 'key', key: 'Enter' })
    expect((await adapter.observe(handle)).busy).toBe(true)
  })

  it('launches its CLI, observes its handle, sends safely and stops', async () => {
    const backend = new FakeBackend()
    const clock = new FakeClock()
    const adapter = new CursorAdapter({ backend, clock, files: new FakeFiles(), home: '/fake', cwd: '/tmp', bin: '/fake/cursor', pollMs: 1000 })
    const handle = await adapter.call('spawn', { named: { prompt: 'A trivial task', model: 'test-model', effort: 'low', trust: true } }, 'worker') as { id: string; capability: string; pane: string }
    expect(adapter.harness).toBe(cursor)
    expect(handle.capability).toBe('worker')
    expect(handle.id).toBeTruthy()
    const pane = backend.pane(handle)
    expect(pane.spec.argv[0]).toBe('/fake/cursor')
    expect(pane.spec.argv).toContain('test-model')
    expect(pane.spec.argv).toContain('--workspace')
    expect(pane.spec.argv).not.toContain('--trust')
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
