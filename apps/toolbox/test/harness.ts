/**
 * What every toolbox test drives: the server's `/ws` boundary. The TypeSafe and
 * Anthropic stand-ins come from `demo/services.ts`.
 */
import { WebSocket } from 'ws'

import type { ServerMessage } from '../shared/protocol.ts'
import { startToolbox, type Toolbox, type ToolboxOptions } from '../server/app.ts'

export interface Client {
  request<T>(type: string, payload?: Record<string, unknown>): Promise<T>
  pushes: ServerMessage[]
  close(): void
}

/** A page's connection to `/ws`: requests with replies, and every push kept in order. */
export async function connect(toolbox: Pick<Toolbox, 'url'>): Promise<Client> {
  const socket = new WebSocket(`${toolbox.url.replace('http', 'ws')}/ws`, { origin: toolbox.url })
  const pushes: ServerMessage[] = []
  const waiting = new Map<number, (message: ServerMessage) => void>()
  let next = 1
  socket.on('message', (data) => {
    const message = JSON.parse(String(data)) as ServerMessage
    if (message.type === 'reply') waiting.get(message.id)?.(message)
    else pushes.push(message)
  })
  await new Promise((resolve) => socket.once('open', resolve))
  return {
    pushes,
    close: () => socket.close(),
    request<T>(type: string, payload: Record<string, unknown> = {}) {
      const id = next++
      return new Promise<T>((resolve, reject) => {
        waiting.set(id, (message) => {
          if (message.type !== 'reply') return
          if (message.ok) resolve(message.result as T)
          else reject(new Error(message.error))
        })
        socket.send(JSON.stringify({ id, type, ...payload }))
      })
    },
  }
}

/** Start a toolbox and connect one page to it. */
export async function open(options: ToolboxOptions): Promise<{ toolbox: Toolbox; client: Client; close(): Promise<void> }> {
  const toolbox = await startToolbox(options)
  const client = await connect(toolbox)
  return {
    toolbox,
    client,
    async close() {
      client.close()
      await toolbox.close()
    },
  }
}

export async function until(predicate: () => boolean, ms = 30_000): Promise<void> {
  const deadline = Date.now() + ms
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
}

/** Every push of one type for one run, in order. */
export function pushed<K extends ServerMessage['type']>(client: Client, type: K, runId?: string): Extract<ServerMessage, { type: K }>[] {
  return client.pushes.filter(
    (push): push is Extract<ServerMessage, { type: K }> => push.type === type && (runId === undefined || !('runId' in push) || push.runId === runId),
  )
}

export { fakeClaude, type FakeClaude, fakeJev, type FakeJev } from '../demo/services.ts'
