import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'
import { WebSocket } from 'ws'

import { startToolbox } from '../server/app.ts'
import { REPO_ROOT } from '../server/env.ts'
import { encodeMessage, lspAvailable, MessageDecoder, readErrorReference } from '../server/lsp.ts'

/** A language server that answers `initialize` and echoes every notification back as `test/echo`. */
const FAKE_SERVER = `
let buffer = Buffer.alloc(0)
const send = (m) => { const b = Buffer.from(JSON.stringify(m)); process.stdout.write('Content-Length: ' + b.length + '\\r\\n\\r\\n'); process.stdout.write(b) }
process.stdin.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk])
  for (;;) {
    const end = buffer.indexOf('\\r\\n\\r\\n'); if (end < 0) return
    const length = Number(/Content-Length: (\\d+)/.exec(buffer.subarray(0, end).toString())[1])
    if (buffer.length < end + 4 + length) return
    const message = JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString()); buffer = buffer.subarray(end + 4 + length)
    if (message.method === 'initialize') send({ jsonrpc: '2.0', id: message.id, result: { capabilities: { hoverProvider: true }, serverInfo: { name: 'fake' } } })
    else send({ jsonrpc: '2.0', method: 'test/echo', params: message })
  }
})
`

describe('LSP framing', () => {
  it('decodes messages split across chunks and several in one chunk, by byte length', () => {
    const first = JSON.stringify({ jsonrpc: '2.0', method: 'a', params: { text: 'état ✓' } })
    const second = JSON.stringify({ jsonrpc: '2.0', id: 1, result: null })
    const bytes = Buffer.concat([encodeMessage(first), encodeMessage(second)])
    const decoder = new MessageDecoder()
    expect(decoder.push(bytes.subarray(0, 10))).toEqual([])
    expect(decoder.push(bytes.subarray(10, 40))).toEqual([])
    expect(decoder.push(bytes.subarray(40))).toEqual([first, second])
  })
})

describe('the /lsp bridge', () => {
  it('starts the configured command and relays LSP messages both ways', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'jevs-lsp-'))
    const script = join(dir, 'fake-lsp.mjs')
    await writeFile(script, FAKE_SERVER)
    const command = `${process.execPath} ${script}`
    expect(lspAvailable(command)).toBe(true)
    const toolbox = await startToolbox({ home: dir, env: { ...process.env, JEVS_LSP_COMMAND: command } })
    try {
      const socket = new WebSocket(`${toolbox.url.replace('http', 'ws')}/lsp`)
      const received: Record<string, unknown>[] = []
      socket.on('message', (data) => received.push(JSON.parse(String(data))))
      await new Promise((resolve) => socket.once('open', resolve))
      socket.send(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {} }))
      socket.send(JSON.stringify({ jsonrpc: '2.0', method: 'textDocument/didOpen', params: { textDocument: { text: 'program ✓' } } }))
      const deadline = Date.now() + 5000
      while (received.length < 2 && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 20))
      expect(received[0]).toMatchObject({ id: 1, result: { serverInfo: { name: 'fake' } } })
      expect(received[1]).toMatchObject({ method: 'test/echo', params: { params: { textDocument: { text: 'program ✓' } } } })
      socket.close()
    } finally {
      await toolbox.close()
    }
  })

  it('reports a missing binary as unavailable', () => {
    expect(lspAvailable('/nonexistent/jevscript lsp')).toBe(false)
  })
})

describe('the error reference behind diagnostic hovers', () => {
  it('reads each code’s cause and correction from docs/error-reference.md', async () => {
    const reference = await readErrorReference(join(REPO_ROOT, 'docs/error-reference.md'))
    expect(reference['uncapped_field']).toEqual({
      meaning: 'A shaped or observed field may grow without a declared cap.',
      correction: 'Add `max`, or shape the value into a bounded representation.',
    })
    expect(reference['pick_no_other']?.meaning).toBe('A `pick` has no escape label.')
  })
})
