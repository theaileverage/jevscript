/**
 * Cursor Agent (`cursor-agent`, legacy `agent`).
 *
 * Launch shape, trust flag, busy footer and transcript rules follow Cursor
 * Agent CLI 2026.08.11 behavior: positional prompt,
 * `--workspace <cwd>` (never `-w`, which makes a second worktree), `--yolo`
 * is autonomy, and there is no
 * effort flag (effort is part of the model id). A running turn shows
 * `ctrl+c to stop`; the idle input box shows `Plan, search, build anything`
 * or `Add a follow-up`. The slash popup eats the first Enter.
 *
 * Turn state comes from the per-conversation transcript
 * `~/.cursor/projects/<project>/agent-transcripts/<conv>/<conv>.jsonl`: a
 * top-level `role: "user"` line opens a turn and a `type: "turn_ended"` line
 * closes it. The project directory is the one whose `.workspace-trusted`
 * names the working directory, rather than a rebuilt slug.
 * The installed 2026.05.09 CLI rejects `--trust` outside print/headless mode,
 * so an interactive pane leaves workspace trust to its own dialog.
 */
import { join } from 'node:path'

import type { AgentHandle, Harness, HarnessContext, SpawnRequest, TranscriptSummary } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, dashModels } from './models.ts'

/** Summarise a Cursor agent transcript's JSONL text. */
export function parseCursorTranscript(jsonl: string | undefined): TranscriptSummary {
  const summary: TranscriptSummary = { lastMessage: '', idle: null, turns: 0, tokens: 0, usd: null }
  if (!jsonl) return summary
  let draft = ''
  for (const raw of jsonl.split('\n')) {
    if (!raw.trim()) continue
    let line: Record<string, unknown>
    try {
      line = JSON.parse(raw) as Record<string, unknown>
    } catch {
      continue
    }
    if (line['type'] === 'turn_ended') {
      summary.idle = true
      summary.turns += 1
      if (draft !== '') summary.lastMessage = draft
      draft = ''
    } else if (line['role'] === 'user') {
      summary.idle = false
      draft = ''
    } else if (line['role'] === 'assistant') {
      const text = textOf(line)
      if (text !== '') draft = text
    }
  }
  return summary
}

function textOf(line: Record<string, unknown>): string {
  const message = (line['message'] ?? line) as Record<string, unknown>
  const content = message['content'] ?? message['text']
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((block): block is { type: string; text: string } => typeof (block as { text?: unknown }).text === 'string')
    .filter((block) => block.type === undefined || block.type === 'text')
    .map((block) => block.text)
    .join('\n\n')
}

/** The Cursor project directory for `cwd`: the one whose `.workspace-trusted` names it. */
async function projectDir(cwd: string, ctx: HarnessContext): Promise<string | undefined> {
  const root = join(ctx.home, '.cursor', 'projects')
  for (const name of await ctx.files.list(root)) {
    const marker = await ctx.files.readHead(join(root, name, '.workspace-trusted'), 4096)
    if (!marker) continue
    try {
      if ((JSON.parse(marker) as { workspacePath?: string }).workspacePath === cwd) return join(root, name)
    } catch {
      // not a marker this adapter can read
    }
  }
  return undefined
}

const located = new Map<string, string>()

/** The newest conversation transcript begun no earlier than the spawn. */
async function locateTranscript(handle: AgentHandle, ctx: HarnessContext): Promise<string | undefined> {
  const known = located.get(handle.id)
  if (known) return known
  const startedAt = Number(handle['started_at'])
  const project = await projectDir(handle.cwd, ctx)
  if (!project || !Number.isFinite(startedAt)) return undefined
  const dir = join(project, 'agent-transcripts')
  const claimed = new Set(located.values())
  let best: { path: string; at: number } | undefined
  for (const conversation of await ctx.files.list(dir)) {
    const path = join(dir, conversation, `${conversation}.jsonl`)
    if (claimed.has(path)) continue
    const at = await ctx.files.mtime(path)
    if (at !== undefined && at >= startedAt - 2000 && (!best || at < best.at)) best = { path, at }
  }
  if (best) located.set(handle.id, best.path)
  return best?.path
}

export const cursor: Harness = {
  name: 'cursor',
  title: 'Cursor Agent',
  bins: ['cursor-agent', 'agent'],
  efforts: null,
  verified: 'Cursor Agent CLI 2026.08.11',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.yolo) argv.push('--yolo')
    if (request.model) argv.push('--model', request.model)
    argv.push('--workspace', request.cwd, request.prompt)
    return { argv, unset: foreignMarkers('CURSOR_AGENT') }
  },

  handleFields: (request) => ({ started_at: request.startedAt }),

  screen: {
    busy: [/ctrl\+c to stop/i],
    idle: [/Plan, search, build anything/, /Add a follow-up/],
    scanLines: 20,
    dialogs: [
      { kind: 'trust', all: [/Workspace Trust Required/, /▶\s*\[a\] Trust this workspace/], answer: ['Enter'] },
      { kind: 'auth', all: [/Press any key to log in\.\.\./] },
    ],
  },

  transcript: { locate: locateTranscript, parse: parseCursorTranscript },

  interrupt: { keys: ['Escape'], gapMs: 300 },
  submit: { commandPrefixes: ['/'], commandSettleMs: 1200, commandExtraEnter: true },
  models: commandModels(['--list-models'], dashModels),
}
