/**
 * The bridge to the Jevscrypt language server that lives in the separate
 * `jevscript` repository. It is started as a subprocess, one per page
 * connection, from `JEVS_LSP_COMMAND`; nothing of it is copied here. The page
 * speaks LSP JSON messages over the `/lsp` WebSocket, one message per frame,
 * and this module converts them to and from the `Content-Length` framing the
 * server reads on stdio.
 */
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

import { WebSocket } from 'ws'

export const DEFAULT_LSP_COMMAND = `${join(homedir(), '.treehouse/jevscript-4c52f4/1/jevscript/target/debug/jevscript')} lsp`

export function lspCommand(env: NodeJS.ProcessEnv = process.env): string {
  return env['JEVS_LSP_COMMAND'] ?? DEFAULT_LSP_COMMAND
}

/** Whether the command's program exists, so the page can show `disconnected` without trying. */
export function lspAvailable(command: string): boolean {
  const program = command.trim().split(/\s+/)[0] ?? ''
  if (!program) return false
  if (program.includes('/')) return existsSync(program)
  return (process.env['PATH'] ?? '').split(':').some((dir) => existsSync(join(dir, program)))
}

export function encodeMessage(message: string): Buffer {
  const body = Buffer.from(message, 'utf8')
  return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'ascii'), body])
}

/** Incremental decoder for `Content-Length` framed messages; bytes may arrive split anywhere. */
export class MessageDecoder {
  #buffer = Buffer.alloc(0)

  push(chunk: Buffer): string[] {
    this.#buffer = Buffer.concat([this.#buffer, chunk])
    const messages: string[] = []
    for (;;) {
      const end = this.#buffer.indexOf('\r\n\r\n')
      if (end === -1) break
      const header = this.#buffer.subarray(0, end).toString('ascii')
      const length = /Content-Length:\s*(\d+)/i.exec(header)?.[1]
      if (length === undefined) {
        this.#buffer = this.#buffer.subarray(end + 4)
        continue
      }
      const total = end + 4 + Number(length)
      if (this.#buffer.length < total) break
      messages.push(this.#buffer.subarray(end + 4, total).toString('utf8'))
      this.#buffer = this.#buffer.subarray(total)
    }
    return messages
  }
}

/** Pipe one WebSocket to a fresh language server process until either side closes. */
export function bridgeLsp(socket: WebSocket, command: string): void {
  const child = spawn('sh', ['-c', `exec ${command}`], { stdio: ['pipe', 'pipe', 'pipe'] })
  const decoder = new MessageDecoder()
  child.stdout.on('data', (chunk: Buffer) => {
    for (const message of decoder.push(chunk)) if (socket.readyState === WebSocket.OPEN) socket.send(message)
  })
  child.stderr.resume()
  const stop = () => {
    if (child.exitCode === null) child.kill()
  }
  child.on('exit', () => socket.readyState === WebSocket.OPEN && socket.close(1011, 'language server exited'))
  child.on('error', () => socket.close(1011, 'language server failed to start'))
  socket.on('message', (data) => {
    if (child.stdin.writable) child.stdin.write(encodeMessage(String(data)))
  })
  socket.on('close', stop)
}

/** What `docs/error-reference.md` says about a code: its meaning (the cause) and the usual correction. */
export interface ErrorReference {
  meaning: string
  correction: string
}

export async function readErrorReference(path: string): Promise<Record<string, ErrorReference>> {
  const reference: Record<string, ErrorReference> = {}
  for (const line of (await readFile(path, 'utf8')).split('\n')) {
    const match = /^\|\s*<a id="([a-z_]+)"><\/a>`[a-z_]+`\s*\|\s*(.*?)\s*\|\s*(.*?)\s*\|$/.exec(line)
    if (match) reference[match[1] as string] = { meaning: match[2] as string, correction: match[3] as string }
  }
  return reference
}
