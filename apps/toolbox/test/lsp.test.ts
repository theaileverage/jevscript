/**
 * The Playground's language server: the real `jevscript lsp` from this
 * checkout, reached through the toolbox's `/lsp` WebSocket bridge, which
 * converts one-message frames to `Content-Length` framing on stdio.
 */
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { WebSocket } from 'ws'

import { decodeTokens, type SemanticLegend } from '../shared/semantic.ts'
import { startToolbox, type Toolbox } from '../server/app.ts'
import { paths, REPO_ROOT } from '../server/env.ts'
import { connect, until } from './harness.ts'

/** The client capabilities `src/lsp.ts` sends; the server requires the semantic-token lists (LSP 3.17). */
const CAPABILITIES = {
  textDocument: {
    semanticTokens: {
      requests: { full: true },
      formats: ['relative'],
      tokenTypes: ['keyword', 'comment', 'string', 'number', 'operator', 'function', 'method', 'namespace', 'interface', 'parameter', 'variable', 'property', 'enumMember', 'event', 'type'],
      tokenModifiers: ['declaration', 'readonly', 'defaultLibrary', 'unitKind', 'judgmentVerb'],
    },
    hover: { contentFormat: ['markdown', 'plaintext'] },
    publishDiagnostics: {},
  },
  general: { positionEncodings: ['utf-16'] },
}

interface Message {
  id?: number
  method?: string
  params?: { uri?: string; diagnostics?: { code: string; range: { start: { line: number } } }[] }
  result?: unknown
}

let toolbox: Toolbox

beforeAll(async () => {
  toolbox = await startToolbox({ home: await mkdtemp(join(tmpdir(), 'jevs-lsp-')), env: { ...process.env, JEVS_LSP_COMMAND: undefined } })
})

afterAll(async () => {
  await toolbox.close()
})

async function lsp(): Promise<{ send(message: object): void; received: Message[]; request(id: number, method: string, params: object): Promise<unknown>; close(): void }> {
  const socket = new WebSocket(`${toolbox.url.replace('http', 'ws')}/lsp`, { origin: toolbox.url })
  const received: Message[] = []
  socket.on('message', (data) => received.push(JSON.parse(String(data)) as Message))
  await new Promise((resolve) => socket.once('open', resolve))
  const send = (message: object) => socket.send(JSON.stringify({ jsonrpc: '2.0', ...message }))
  return {
    send,
    received,
    close: () => socket.close(),
    async request(id, method, params) {
      send({ id, method, params })
      await until(() => received.some((message) => message.id === id), 10_000)
      return received.find((message) => message.id === id)!.result
    },
  }
}

describe('the /lsp bridge to `jevscript lsp`', () => {
  it('defaults to this checkout’s binary and reports it available', async () => {
    const client = await connect(toolbox)
    const status = await client.request<{ lsp: { command: string; available: boolean } }>('status')
    client.close()
    expect(status.lsp).toEqual({ command: `${paths().bin} lsp`, available: true })
  })

  it('carries diagnostics, semantic tokens and hover both ways, multi-byte text included', async () => {
    const source = `# état ✓, before the program\n${await readFile(join(REPO_ROOT, 'examples/review_loop.jev'), 'utf8')}`
    const uri = 'file:///toolbox/playground/review_loop.jev'
    const server = await lsp()
    const initialized = (await server.request(1, 'initialize', { processId: null, rootUri: null, capabilities: CAPABILITIES })) as {
      capabilities: { semanticTokensProvider: { legend: SemanticLegend } }
      serverInfo?: { name: string }
    }
    const legend = initialized.capabilities.semanticTokensProvider.legend
    server.send({ method: 'initialized', params: {} })
    server.send({ method: 'textDocument/didOpen', params: { textDocument: { uri, languageId: 'jevscript', version: 1, text: source } } })

    await until(() => server.received.some((message) => message.method === 'textDocument/publishDiagnostics'), 10_000)
    const published = server.received.find((message) => message.method === 'textDocument/publishDiagnostics')!
    expect(published.params?.uri).toBe(uri)
    expect(published.params?.diagnostics?.map((diagnostic) => [diagnostic.code, diagnostic.range.start.line])).toEqual([['uncapped_field', 15]])

    const tokens = (await server.request(2, 'textDocument/semanticTokens/full', { textDocument: { uri } })) as { data: number[] }
    const decoded = decodeTokens(tokens.data, legend)
    expect(decoded[0]).toMatchObject({ line: 0, character: 0, type: 'comment' })
    expect(decoded.find((token) => token.line === 1)).toMatchObject({ character: 0, length: 7, type: 'keyword' })
    expect(decoded.filter((token) => token.type === 'event').length).toBeGreaterThan(0)

    // `tests_pass` in the `finished` guard on line 18 of the example, one line lower here.
    const line = source.split('\n')[18]!
    const hover = (await server.request(3, 'textDocument/hover', { textDocument: { uri }, position: { line: 18, character: line.indexOf('tests_pass') + 1 } })) as {
      contents: { value: string }
    } | null
    expect(hover?.contents.value).toContain('tests_pass')
    server.close()
  })

  it('reports a missing binary as unavailable and closes the socket', async () => {
    const missing = await startToolbox({ home: await mkdtemp(join(tmpdir(), 'jevs-lsp-')), env: { ...process.env, JEVS_LSP_COMMAND: '/nonexistent/jevscript lsp' } })
    try {
      const client = await connect(missing)
      const status = await client.request<{ lsp: { available: boolean } }>('status')
      client.close()
      expect(status.lsp.available).toBe(false)
      const socket = new WebSocket(`${missing.url.replace('http', 'ws')}/lsp`, { origin: missing.url })
      const code = await new Promise<number>((resolve) => socket.once('close', resolve))
      expect(code).toBe(1011)
    } finally {
      await missing.close()
    }
  })
})
