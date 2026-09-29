/**
 * The toolbox server: one HTTP server that upgrades `/ws` to the page's
 * WebSocket and hands every other request to `serveStatic` (Vite in dev, the
 * built files in production). Every request the page makes is dispatched here.
 */
import { execFile } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join, resolve, sep } from 'node:path'

import { load } from 'jevscript'
import { WebSocket, WebSocketServer } from 'ws'

import type { ClientMessage, Idea, Replies, Request, ServerMessage } from '../shared/protocol.ts'
import { parseRecording } from '../shared/recording.ts'
import { annotate, chatTurn, type Model } from './claude.ts'
import { paths as defaultPaths, REPO_ROOT } from './env.ts'
import { newIdea, ToolboxStore } from './store.ts'
import { bridgeLsp, lspAvailable, lspCommand, readErrorReference } from './lsp.ts'
import { resend } from './resend.ts'
import { Jevscript } from './jevscript.ts'
import { readProfiles } from './profiles.ts'
import { RunManager } from './runs.ts'

export interface ToolboxOptions {
  env?: NodeJS.ProcessEnv
  bin?: string
  home?: string
  model?: Model | null
  modelName?: string
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
  const defaults = defaultPaths(env)
  const bin = options.bin ?? defaults.bin
  const home = options.home ?? defaults.home
  const jev = new Jevscript(bin, join(home, 'work'))
  const ideas = new ToolboxStore(home)
  const model = options.model ?? null
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
      typesafeKey: Boolean(env['TYPESAFE_API_KEY']),
      profilesOverlay: env['JEVSCRIPT_PROFILES'] ?? null,
      lsp: { command: lsp, available: lspAvailable(lsp) },
    }),
    check: (request) => jev.compile(request.fileName, request.source),
    'tools.check': (request) => jev.checkTools(request.fileName, request.source, request.manifests),
    profiles: () => readProfiles(defaults.bundledProfiles, env['JEVSCRIPT_PROFILES']),
    'ideas.list': async () => ({ ideas: await ideas.list() }),
    'ideas.save': async (request) => ({ idea: await ideas.save(request.idea) }),
    'ideas.delete': async (request) => {
      await ideas.delete(request.ideaId)
      return { ok: true }
    },
    'chat.send': async (request) => {
      const idea = await chatTurn(request.idea, request.text, { model, compile: (f, s) => jev.compile(f, s), spec })
      return { idea: await ideas.save(idea) }
    },
    annotate: async (request) => {
      const current = await jev.compile(request.idea.fileName, request.idea.source)
      const pin = await annotate(request.idea, request.target, request.query, {
        model,
        compile: (f, s) => jev.compile(f, s),
        spec,
        current,
      })
      return { pin }
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
      const program = await load(await jev.materialize(request.fileName, request.source), { bin })
      try {
        return { answers: await program.judgment(request.judgment).run(request.state, { model: request.model }) }
      } finally {
        await program.close()
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
    if (request.url === '/ws') wss.handleUpgrade(request, socket, head, (ws) => wss.emit('connection', ws, request))
    else if (request.url === '/lsp') lspServer.handleUpgrade(request, socket, head, (ws) => lspServer.emit('connection', ws, request))
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
