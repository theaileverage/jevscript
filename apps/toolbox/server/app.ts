/**
 * The toolbox server: one HTTP server that upgrades `/ws` to the page's
 * WebSocket and hands every other request to `serveStatic` (Vite in dev, the
 * built files in production). Every request the page makes is dispatched here.
 */
import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { readFile, rm } from 'node:fs/promises'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join, resolve, sep } from 'node:path'

import { load } from 'jevscript'
import { WebSocket, WebSocketServer } from 'ws'

import type { ChatOrigin, ClientMessage, FileAnnotation, Idea, Replies, Request, ServerMessage } from '../shared/protocol.ts'
import { parseRecording } from '../shared/recording.ts'
import { annotate, chatTurn, type Model } from './claude.ts'
import { LocalAgents } from './agents.ts'
import { paths as defaultPaths, REPO_ROOT } from './env.ts'
import { newIdea, normalize, ToolboxStore } from './store.ts'
import { bridgeLsp, lspAvailable, lspCommand, readErrorReference } from './lsp.ts'
import { resend } from './resend.ts'
import { Jevscript } from './jevscript.ts'
import { readProfiles } from './profiles.ts'
import { RunManager } from './runs.ts'
import { HostRunner } from './hosts.ts'

export interface ToolboxOptions {
  env?: NodeJS.ProcessEnv
  bin?: string
  home?: string
  model?: Model | null
  modelName?: string
  services?: 'standard' | 'demo'
  agentTimeoutMs?: number
  port?: number
  host?: string
  serveStatic?: (request: IncomingMessage, response: ServerResponse) => void
}

export interface Toolbox {
  url: string
  port: number
  /** The SQLite file holding the toolbox's own state. */
  database: string
  close(): Promise<void>
}

export async function startToolbox(options: ToolboxOptions = {}): Promise<Toolbox> {
  const env = options.env ?? process.env
  const allowedOrigins = new Set((env['JEVS_TOOLBOX_ALLOWED_ORIGINS'] ?? '').split(',').filter(Boolean).map((origin) => {
    const exact = origin.trim()
    if (!localOrigin(exact)) throw new Error(`invalid toolbox allowed origin: ${exact}`)
    return exact
  }))
  const defaults = defaultPaths(env)
  const bin = options.bin ?? defaults.bin
  const home = options.home ?? defaults.home
  const jev = new Jevscript(bin, join(home, 'work'))
  const ideas = new ToolboxStore(home)
  const model = options.model ?? null
  const agents = new LocalAgents({ env, home, ...(options.agentTimeoutMs ? { timeoutMs: options.agentTimeoutMs } : {}) })
  let agentStatuses = await agents.discover()
  const spec = await readFile(join(REPO_ROOT, 'spec/jevscript-language-specification.md'), 'utf8')
  const sockets = new Set<WebSocket>()
  const broadcast = (message: ServerMessage) => {
    const text = JSON.stringify(message)
    for (const socket of sockets) if (socket.readyState === WebSocket.OPEN) socket.send(text)
  }
  const runs = new RunManager(jev, home, broadcast, async (ideaId, summary) => {
    const idea = await ideas.appendRun(ideaId, summary)
    if (idea) broadcast({ type: 'idea.updated', idea })
  })
  const hosts = new HostRunner(jev, runs, home, broadcast)
  await seedIdeas(ideas, defaults.examples)
  const lsp = lspCommand(env, bin)
  const errorReference = await readErrorReference(join(REPO_ROOT, 'docs/error-reference.md'))

  const recordingPath = (path: string) => {
    const full = resolve(path)
    if (!full.startsWith(resolve(runs.recordingsDir) + sep)) {
      throw new Error('recordings are read only from the toolbox recordings folder')
    }
    return full
  }

  const handlers: { [K in Request['type']]: (request: Extract<Request, { type: K }>) => Promise<Replies[K]> } = {
    status: async () => ({
      bin,
      home,
      database: ideas.path,
      claude: { available: model !== null, model: model?.name ?? options.modelName ?? 'claude-opus-5-5' },
      agents: agentStatuses,
      services: options.services ?? 'standard',
      typesafeKey: Boolean(env['TYPESAFE_API_KEY']),
      profilesOverlay: env['JEVSCRIPT_PROFILES'] ?? null,
      lsp: { command: lsp, available: lspAvailable(lsp) },
    }),
    check: (request) => jev.compile(request.fileName, request.source, request.files),
    'tools.check': (request) => jev.checkTools(request.fileName, request.source, request.manifests, request.files),
    profiles: () => readProfiles(defaults.bundledProfiles, env['JEVSCRIPT_PROFILES']),
    'ideas.list': async () => ({ ideas: await ideas.list() }),
    'ideas.save': async (request) => ({ idea: await ideas.save(request.idea) }),
    'ideas.delete': async (request) => {
      await ideas.delete(request.ideaId)
      return { ok: true }
    },
    'chat.send': async (request) => {
      const selected = request.idea.chatAgent
      let chatModel = model
      let unavailable: string | undefined
      if (selected) {
        try { chatModel = agents.model(selected) }
        catch (error) { chatModel = null; unavailable = error instanceof Error ? error.message : 'The local agent is unavailable.' }
      }
      const idea = await chatTurn(request.idea, request.text, {
        model: chatModel, compile: (f, s) => jev.compile(f, s, request.idea.workspace?.files), spec,
        origin: selected
          ? unavailable ? { kind: 'error', ...selected } : { kind: 'agent', ...selected, requestedModel: selected.model }
          : { kind: options.services === 'demo' ? 'fixture' : 'api', model: model?.name ?? '' },
        ...(unavailable ? { unavailable } : {}),
      })
      return { idea: await ideas.save(idea) }
    },
    'agents.refresh': async () => {
      agentStatuses = await agents.discover()
      return { agents: agentStatuses }
    },
    annotate: async (request) => {
      const current = await jev.compile(request.idea.fileName, request.idea.source, request.idea.workspace?.files)
      const pin = await annotate(request.idea, request.target, request.query, {
        model,
        compile: (f, s) => jev.compile(f, s, request.idea.workspace?.files),
        spec,
        current,
      })
      return { pin: { ...pin, fileId: request.idea.workspace?.entryFileId } }
    },
    'file.annotate': async request => {
      const idea = normalize(request.idea)
      const file = idea.workspace.files.find(file => file.id === request.fileId)
      if (!file) throw new Error('The selected file no longer belongs to this idea.')
      if (!Number.isInteger(request.from) || !Number.isInteger(request.to) || request.from < 0 || request.to <= request.from || request.to > file.source.length) throw new Error('Select text from the current file before annotating.')
      const selection = idea.chatAgent
      if (!selection) throw new Error('Choose a Harness model before annotating.')
      const selected = file.source.slice(request.from, request.to)
      let reply: string
      let origin: ChatOrigin = { kind: 'error', ...selection }
      try {
        const model = agents.model(selection)
        const completion = await model.complete(
          file.kind === 'jev' ? `Answer questions about Jevscript from this language specification.\n${spec}` : 'Answer questions about this idea’s host source file. The toolbox provides runIdea({inputs?, task?}) from .toolbox/host.ts, backed by the Jevscript SDK with selected bindings, profile, pauses and recordings. Do not claim that the source has been executed or checked.',
          [{ role: 'user', content: `File ${file.name}:\n${file.source}\n\nSelected text:\n${selected}\n\nQuestion:\n${request.query}\n\nReply with a concise annotation. Do not change files or invoke tools.` }],
        )
        reply = completion.text
        origin = { kind: 'agent', ...selection, model: completion.model ?? selection.model, requestedModel: selection.model }
      } catch (error) { reply = error instanceof Error ? error.message : 'The local agent failed.' }
      const annotation: FileAnnotation = { id: randomUUID(), fileId: file.id, from: request.from, to: request.to, selected, query: request.query, reply, origin, at: new Date().toISOString() }
      const saved = await ideas.save({ ...idea, workspace: { ...idea.workspace, annotations: [...idea.workspace.annotations, annotation] } })
      return { idea: saved, annotation }
    },
    'pair.check': request => hosts.check(normalize(request.idea), request.hostFileId),
    'pair.run': async request => {
      const idea = await ideas.save(request.idea)
      return hosts.start(idea, request.hostFileId)
    },
    'run.start': (request) => runs.start(request.run),
    'run.resume': async (request) => {
      runs.resume(request.runId, request.payload)
      return { ok: true }
    },
    'run.abort': async (request) => {
      await runs.abort(request.runId)
      return { ok: true }
    },
    'run.inject': async (request) => {
      await runs.inject(request.runId, request.capability, request.message)
      return { ok: true }
    },
    replay: (request) => jev.replay(recordingPath(request.recording), env),
    'recording.read': async (request) => ({
      events: parseRecording(await readFile(recordingPath(request.recording), 'utf8')),
    }),
    'pane.tail': (request) => paneTail(request.pane),
    resend: (request) =>
      resend({ ...request, recording: recordingPath(request.recording) }, { store: ideas, env }),
    'resends.list': async (request) => ({
      history: await ideas.resendHistory(request.ideaId, recordingPath(request.recording), request.requestId),
    }),
    'errors.reference': async () => errorReference,
    'runs.live': async () => ({ runs: runs.live() }),
    judge: async (request) => {
      const { path, dir } = await jev.materialize(request.fileName, request.source, request.files)
      try {
        const program = await load(path, { bin })
        try {
          return { answers: await program.judgment(request.judgment).run(request.state, { model: request.model }) }
        } finally {
          await program.close()
        }
      } finally {
        await rm(dir, { recursive: true, force: true })
      }
    },
  }

  const server = createServer((request, response) => {
    if (options.serveStatic) options.serveStatic(request, response)
    else {
      response.statusCode = 404
      response.end()
    }
  })
  const wss = new WebSocketServer({ noServer: true })
  const lspServer = new WebSocketServer({ noServer: true })
  lspServer.on('connection', (ws: WebSocket) => bridgeLsp(ws, lsp))
  server.on('upgrade', (request, socket, head) => {
    const origin = request.headers.origin
    const ownOrigin = `http://${request.headers.host ?? ''}`
    if (!origin || !localOrigin(origin) || (origin !== ownOrigin && !allowedOrigins.has(origin))) {
      socket.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n')
      return
    }
    if (request.url === '/ws') wss.handleUpgrade(request, socket, head, (ws) => wss.emit('connection', ws, request))
    else if (request.url === '/lsp') lspServer.handleUpgrade(request, socket, head, (ws) => lspServer.emit('connection', ws, request))
    else socket.end('HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n')
  })
  wss.on('connection', (socket: WebSocket) => {
    sockets.add(socket)
    socket.on('close', () => sockets.delete(socket))
    socket.on('message', async (data) => {
      let message: ClientMessage
      try {
        message = JSON.parse(String(data)) as ClientMessage
      } catch {
        return
      }
      const reply = (payload: ServerMessage) => socket.readyState === WebSocket.OPEN && socket.send(JSON.stringify(payload))
      try {
        const handler = handlers[message.type] as (request: Request) => Promise<unknown>
        if (!handler) throw new Error(`unknown request \`${message.type}\``)
        reply({ type: 'reply', id: message.id, ok: true, result: await handler(message) })
      } catch (error) {
        reply({ type: 'reply', id: message.id, ok: false, error: error instanceof Error ? error.message : String(error) })
      }
    })
  })

  await new Promise<void>((done) => server.listen(options.port ?? 0, options.host ?? '127.0.0.1', done))
  const port = (server.address() as AddressInfo).port
  return {
    url: `http://${options.host ?? '127.0.0.1'}:${port}`,
    port,
    database: ideas.path,
    async close() {
      await hosts.close()
      await runs.closeAll()
      for (const socket of sockets) socket.terminate()
      wss.close()
      for (const client of lspServer.clients) client.terminate()
      lspServer.close()
      await new Promise<void>((done) => server.close(() => done()))
      ideas.close()
    },
  }
}

function localOrigin(origin: string): boolean {
  if (!/^https?:\/\/(?:localhost|127\.0\.0\.1|\[::1\]):[0-9]+$/.test(origin)) return false
  try {
    const url = new URL(origin)
    return url.origin === origin && Number(url.port) > 0 && Number(url.port) <= 65535
  } catch {
    return false
  }
}

/**
 * The last screen of an agent's tmux pane. Display only: this text is
 * agent-written and the page never acts on it (spec section 9.1).
 */
function paneTail(pane: string): Promise<{ text: string }> {
  if (!/^%\d+$/.test(pane)) return Promise.reject(new Error(`\`${pane}\` is not a tmux pane id`))
  return new Promise((done, fail) => {
    execFile('tmux', ['capture-pane', '-p', '-J', '-t', pane, '-S', '-40'], (error, stdout) => {
      if (error) fail(new Error(`tmux capture-pane failed: ${error.message}`))
      else done({ text: stdout.replace(/\s+$/, '') })
    })
  })
}

/** A first launch starts with the repository's review loop and the brief's inbox triage. */
async function seedIdeas(store: ToolboxStore, examples: string): Promise<void> {
  if ((await store.list()).length > 0) return
  const seeds: Partial<Idea>[] = [
    {
      title: 'Review loop for Claude',
      fileName: 'review_loop.jev',
      source: await readFile(join(examples, 'review_loop.jev'), 'utf8'),
      bindings: { claude: { kind: 'stub' }, tree: { kind: 'stub' }, me: { kind: 'toolbox' } },
    },
    {
      title: 'Inbox triage by urgency',
      fileName: 'triage_lane.jev',
      source: await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8'),
      inputs: JSON.stringify({ message: "Can we move Thursday's sync to 3pm?" }, null, 2),
    },
  ]
  for (const seed of seeds) await store.save(newIdea(seed))
}
