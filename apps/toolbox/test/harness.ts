/**
 * What every toolbox test drives: the server's `/ws` boundary, and local
 * stand-ins for the two services the toolbox calls out to (the TypeSafe
 * endpoint and the Anthropic Messages API). The stand-ins are fixtures, never
 * evidence about the live services.
 */
import { readFile, writeFile } from 'node:fs/promises'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'

import { WebSocket } from 'ws'

import type { ServerMessage } from '../shared/protocol.ts'
import { startToolbox, type Toolbox, type ToolboxOptions } from '../server/app.ts'
import { paths } from '../server/env.ts'

export interface Client {
  request<T>(type: string, payload?: Record<string, unknown>): Promise<T>
  pushes: ServerMessage[]
  close(): void
}

/** A page's connection to `/ws`: requests with replies, and every push kept in order. */
export async function connect(toolbox: Toolbox): Promise<Client> {
  const socket = new WebSocket(`${toolbox.url.replace('http', 'ws')}/ws`)
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

async function listen(server: Server): Promise<string> {
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done))
  return `http://127.0.0.1:${(server.address() as AddressInfo).port}`
}

function close(server: Server): Promise<void> {
  return new Promise((done) => server.close(() => done()))
}

/**
 * A stand-in for the TypeSafe endpoint. It keeps every raw body it receives
 * and answers a Choice with the first of `prefer` it was offered (else
 * `stay`, else the first label), a Noul with `noul`. `respond`, when set,
 * replaces the answer body outright.
 */
export interface FakeJev {
  endpoint: string
  bodies: string[]
  prefer: string[]
  noul: number
  respond: ((sent: unknown) => unknown) | null
  close(): Promise<void>
  /** A profiles file pointing every bundled model at this endpoint. */
  profiles(dir: string): Promise<string>
}

export async function fakeJev(prefer: string[] = []): Promise<FakeJev> {
  const fake = { bodies: [] as string[], prefer, noul: 0.9, respond: null as FakeJev['respond'] }
  const server = createServer((req, res) => {
    let body = ''
    req.on('data', (chunk: Buffer) => (body += chunk.toString('utf8')))
    req.on('end', () => {
      fake.bodies.push(body)
      const sent = JSON.parse(body) as { model: string; questions: Record<string, { type: string; criteria?: unknown }> }
      res.setHeader('content-type', 'application/json')
      if (fake.respond) {
        res.end(JSON.stringify(fake.respond(sent)))
        return
      }
      const answers: Record<string, unknown> = {}
      for (const [id, question] of Object.entries(sent.questions)) {
        if (question.type === 'noul') {
          answers[id] = { noul: fake.noul }
          continue
        }
        if (question.type === 'score') {
          const levels = (question.criteria as unknown[]).length
          answers[id] = { score: 0, probabilities: Object.fromEntries(Array.from({ length: levels }, (_, i) => [String(i), i === 0 ? 1 : 0])), confidence: 1 }
          continue
        }
        const labels = Object.keys(question.criteria as Record<string, unknown>)
        const pick = fake.prefer.find((label) => labels.includes(label)) ?? (labels.includes('stay') ? 'stay' : labels[0]!)
        const probabilities = Object.fromEntries(labels.map((label) => [label, label === pick ? 0.8 : 0.2 / (labels.length - 1)]))
        answers[id] = { choice: pick, probabilities, confidence: 0.8 }
      }
      res.end(JSON.stringify({ model: sent.model, answers, usage: { input_tokens: 100, output_tokens: 0 } }))
    })
  })
  const endpoint = `${await listen(server)}/v1/systemone`
  return Object.assign(fake, {
    endpoint,
    close: () => close(server),
    async profiles(dir: string) {
      const bundled = JSON.parse(await readFile(paths().bundledProfiles, 'utf8')) as Record<string, unknown>[]
      const file = join(dir, 'profiles.json')
      await writeFile(file, JSON.stringify(bundled.map((profile) => ({ ...profile, endpoint }))))
      return file
    },
  })
}

/**
 * A stand-in for the Anthropic Messages API, reached through
 * `ANTHROPIC_BASE_URL` by the real SDK client. Each request takes the next
 * scripted reply text, or asks `fallback` once the script runs out, and
 * streams it back as one text block.
 */
export interface FakeClaude {
  baseUrl: string
  replies: string[]
  fallback: ((request: FakeClaude['requests'][number]) => string) | null
  /** Every request body, parsed. */
  requests: { model: string; system: { text: string }[]; messages: { role: string; content: unknown }[]; output_config?: { format?: unknown } }[]
  close(): Promise<void>
}

export async function fakeClaude(replies: string[] = []): Promise<FakeClaude> {
  const fake = { replies, requests: [] as FakeClaude['requests'], fallback: null as FakeClaude['fallback'] }
  const server = createServer((req, res) => {
    let body = ''
    req.on('data', (chunk: Buffer) => (body += chunk.toString('utf8')))
    req.on('end', () => {
      const sent = JSON.parse(body) as FakeClaude['requests'][number]
      fake.requests.push(sent)
      const text = fake.replies.shift() ?? fake.fallback?.(sent)
      if (text === undefined) {
        res.statusCode = 500
        res.setHeader('content-type', 'application/json')
        res.end(JSON.stringify({ type: 'error', error: { type: 'api_error', message: 'no more scripted replies' } }))
        return
      }
      const usage = { input_tokens: 10, output_tokens: 10 }
      const events: [string, unknown][] = [
        ['message_start', { type: 'message_start', message: { id: `msg_${fake.requests.length}`, type: 'message', role: 'assistant', model: sent.model, content: [], stop_reason: null, stop_sequence: null, usage } }],
        ['content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }],
        ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text } }],
        ['content_block_stop', { type: 'content_block_stop', index: 0 }],
        ['message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage }],
        ['message_stop', { type: 'message_stop' }],
      ]
      res.setHeader('content-type', 'text/event-stream')
      res.end(events.map(([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`).join(''))
    })
  })
  const baseUrl = await listen(server)
  return Object.assign(fake, { baseUrl, close: () => close(server) })
}
