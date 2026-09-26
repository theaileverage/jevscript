/** JSONL and discovery at the host boundary (spec sections 9.1 and 11.6). */
import { expect, it } from 'vitest'

import { AgentAdapter, answer, codex, discoverHarness, type Harness } from '../src/index.ts'
import { FakeBackend, FakeClock, FakeFiles } from '../src/testing.ts'

it('serves calls and observations with the binding name on a reattachable handle', async () => {
  const backend = new FakeBackend()
  const options = { backend, clock: new FakeClock(), files: new FakeFiles(), home: '/fake', cwd: '/work' }
  const adapter = new AgentAdapter(codex, options)
  const spawned = await answer(adapter, JSON.stringify({
    operation: 'call', capability: 'dev', verb: 'spawn', args: { named: { prompt: 'hello' } },
  }))
  const handle = spawned['result'] as { id: string; capability: string; pane: string }
  expect(handle.capability).toBe('dev')
  backend.pane(handle).screen = '›'
  const reattached = new AgentAdapter(codex, options)
  const observed = await answer(reattached, JSON.stringify({ operation: 'observe', capability: 'dev', handle }))
  expect(observed['observation']).toMatchObject({ status: 'waiting', backend: 'herdr' })
  expect(await answer(adapter, '{bad')).toMatchObject({ error: { retryable: false } })
  expect(await answer(adapter, JSON.stringify({ operation: 'call', verb: 'unknown' }))).toMatchObject({
    error: { retryable: false },
  })
})

it('discovers an installed CLI version and its model and effort surface', async () => {
  const harness: Harness = {
    name: 'test', title: 'Test CLI', bins: [process.execPath], efforts: ['low', 'high'], verified: 'fixture',
    launch: (request) => ({ argv: [request.bin, request.prompt] }),
    screen: { busy: [], idle: [] }, interrupt: { keys: [], gapMs: 0 },
    models: { source: 'fixture', list: async () => [{ id: 'model-a', efforts: ['low'] }] },
  }
  const result = await discoverHarness(harness, {
    models: true,
    exec: async () => ({ code: 0, stdout: 'test-cli 1.2.3\n', stderr: '' }),
  })
  expect(result).toMatchObject({
    installed: true, version: '1.2.3', efforts: ['low', 'high'],
    models: [{ id: 'model-a', efforts: ['low'] }], modelsSource: 'fixture',
  })
})
