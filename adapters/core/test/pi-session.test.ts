/** omp 18.2 prepends a title before session metadata; reattachment still finds the turn. */
import { expect, it } from 'vitest'

import { defaultFiles, locatePiSession, parsePiSession } from '../src/index.ts'
import { FakeFiles } from '../src/testing.ts'

it('finds a Pi-family session after a title preface and reads its completed answer', async () => {
  const files = new FakeFiles()
  const root = '/home/.omp/agent/sessions'
  const path = `${root}/encoded/2026-09-26T14-44-03-237Z_01-test.jsonl`
  const cwd = '/work/task'
  await files.write(path, [
    JSON.stringify({ type: 'title', title: 'task' }),
    JSON.stringify({ type: 'session', cwd }),
    JSON.stringify({ type: 'message', message: { role: 'user', content: 'go' } }),
    JSON.stringify({ type: 'message', message: { role: 'assistant', stopReason: 'stop', content: [{ type: 'text', text: 'done' }] } }),
  ].join('\n'))
  const startedAt = Date.parse('2026-09-26T14:44:03.000Z')
  const found = await locatePiSession(root, { capability: 'dev', id: 'handle-test', cwd, started_at: startedAt }, {
    exec: async () => ({ code: 0, stdout: '', stderr: '' }), files, home: '/home', env: {}, options: {},
  })
  expect(found).toBe(path)
  expect(parsePiSession(await files.read(found as string))).toMatchObject({ idle: true, lastMessage: 'done' })
})

it('treats a non-directory candidate as absent while scanning cursor projects', async () => {
  const path = `${process.cwd()}/package.json/.workspace-trusted`
  expect(await defaultFiles.readHead(path, 256)).toBeUndefined()
  expect(await defaultFiles.read(path)).toBeUndefined()
})
