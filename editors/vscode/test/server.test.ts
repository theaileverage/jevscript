// Starting the server: the command the settings produce, and a real LSP
// handshake with the binary the workspace builds.

import { spawn } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import {
  type MessageConnection,
  StreamMessageReader,
  StreamMessageWriter,
  createMessageConnection,
} from 'vscode-jsonrpc/node'

import { initializationOptions, serverCommand } from '../src/server'

const defaults = { serverPath: 'jevscript', modulePaths: [], errorReference: '' }

describe('serverCommand', () => {
  it('runs `jevscript lsp` from PATH by default', () => {
    expect(serverCommand(defaults, '/work')).toEqual({ command: 'jevscript', args: ['lsp'] })
  })

  it('resolves a relative binary against the workspace folder', () => {
    const relative = { ...defaults, serverPath: 'target/debug/jevscript' }
    expect(serverCommand(relative, '/work').command).toBe('/work/target/debug/jevscript')
    expect(serverCommand({ ...defaults, serverPath: '/opt/jevscript' }, '/work').command).toBe('/opt/jevscript')
  })

  it('passes module paths and an error reference only when set', () => {
    expect(initializationOptions(defaults)).toEqual({ paths: [] })
    expect(initializationOptions({ ...defaults, modulePaths: ['lib'], errorReference: ' https://x/y ' })).toEqual({
      paths: ['lib'],
      errorReference: 'https://x/y',
    })
  })
})

const repo = fileURLToPath(new URL('../../..', import.meta.url))
const binary = `${repo}target/debug/jevscript`

describe('a session with jevscript lsp', () => {
  let connection: MessageConnection | undefined

  afterEach(() => {
    connection?.dispose()
  })

  it('initializes, publishes diagnostics and answers hover', async () => {
    expect(existsSync(binary), `build the server first: cargo build (${binary})`).toBe(true)
    const child = spawn(binary, ['lsp'], { stdio: ['pipe', 'pipe', 'inherit'] })
    const open = new StreamMessageReader(child.stdout)
    connection = createMessageConnection(open, new StreamMessageWriter(child.stdin))
    const published = new Promise<{ diagnostics: { severity: number; code: string }[] }>((resolve) => {
      connection?.onNotification('textDocument/publishDiagnostics', resolve)
    })
    connection.listen()

    const examples = `${repo}examples`
    const init = await connection.sendRequest('initialize', {
      processId: process.pid,
      rootUri: pathToFileURL(examples).href,
      capabilities: {},
      initializationOptions: { paths: [], errorReference: 'https://example.test/errors.md' },
    })
    expect((init as { capabilities: { semanticTokensProvider: unknown } }).capabilities.semanticTokensProvider).toBeTruthy()
    await connection.sendNotification('initialized', {})

    const file = `${examples}/review_loop.jev`
    const uri = pathToFileURL(file).href
    const text = readFileSync(file, 'utf8').replace(', max 300', '')
    await connection.sendNotification('textDocument/didOpen', {
      textDocument: { uri, languageId: 'jevscript', version: 1, text },
    })
    const { diagnostics } = await published
    // Removing the cap makes `tests` an uncapped field: a warning (spec 7.2).
    expect(diagnostics.map((d) => [d.code, d.severity])).toContainEqual(['uncapped_field', 2])

    const line = text.split('\n').findIndex((l) => l.startsWith('machine review'))
    const hover = (await connection.sendRequest('textDocument/hover', {
      textDocument: { uri },
      position: { line, character: 2 },
    })) as { contents: { value: string } }
    expect(hover.contents.value).toContain('Spec section 7.8')

    await connection.sendRequest('shutdown')
    await connection.sendNotification('exit')
    const code = await new Promise<number | null>((resolve) => child.on('exit', resolve))
    expect(code).toBe(0)
  })
})
