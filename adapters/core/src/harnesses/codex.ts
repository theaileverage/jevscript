/**
 * Codex CLI.
 *
 * Launch flags, effort values, the trust and hook dialogs and the popup
 * settle follow codex-cli 0.139 to 0.153 behavior.
 * Turn state comes from Codex's own session rollout
 * (`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`), whose `task_started`
 * and `task_complete` events open and close a turn and whose `task_complete`
 * carries the agent's last message (format checked on codex-cli 0.155.1).
 * The rollout is found by its `session_meta` working directory and start time,
 * so a handle needs nothing but its `cwd` and `started_at` to reattach.
 */
import { join } from 'node:path'

import type { AgentHandle, Harness, HarnessContext, ModelInfo, SpawnRequest, TranscriptSummary } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'

/** Summarise a Codex rollout's JSONL text. */
export function parseRollout(jsonl: string | undefined): TranscriptSummary {
  const summary: TranscriptSummary = { lastMessage: '', idle: null, turns: 0, tokens: 0, usd: null }
  if (!jsonl) return summary
  for (const raw of jsonl.split('\n')) {
    if (!raw.trim()) continue
    let line: { type?: string; payload?: Record<string, unknown> }
    try {
      line = JSON.parse(raw) as typeof line
    } catch {
      continue // a line still being written
    }
    if (line.type !== 'event_msg' || !line.payload) continue
    const payload = line.payload
    switch (payload['type']) {
      case 'task_started':
        summary.idle = false
        break
      case 'task_complete': {
        summary.idle = true
        summary.turns += 1
        const message = payload['last_agent_message']
        if (typeof message === 'string' && message !== '') summary.lastMessage = message
        break
      }
      case 'turn_aborted':
        summary.idle = true
        break
      case 'agent_message': {
        const message = payload['message']
        if (typeof message === 'string' && message !== '' && summary.idle !== false) summary.lastMessage = message
        break
      }
      case 'token_count': {
        const total = (payload['info'] as { total_token_usage?: { total_tokens?: unknown } } | undefined)
          ?.total_token_usage?.total_tokens
        if (typeof total === 'number') summary.tokens = total
        break
      }
    }
  }
  return summary
}

export function codexHome(ctx: Pick<HarnessContext, 'env' | 'home'>): string {
  return ctx.env['CODEX_HOME'] ?? join(ctx.home, '.codex')
}

/** The day directories a rollout started at `ms` can be under, local time, that day and the next. */
function dayDirs(root: string, ms: number): string[] {
  return [0, 1].map((offset) => {
    const day = new Date(ms + offset * 86_400_000)
    const pad = (n: number) => String(n).padStart(2, '0')
    return join(root, String(day.getFullYear()), pad(day.getMonth() + 1), pad(day.getDate()))
  })
}

/**
 * The rollout for a handle: the earliest one whose `session_meta` names the
 * handle's working directory and started no earlier than the spawn.
 */
export async function locateRollout(handle: AgentHandle, ctx: HarnessContext): Promise<string | undefined> {
  const known = located.get(handle.id)
  if (known) return known
  const startedAt = Number(handle['started_at'])
  if (!Number.isFinite(startedAt)) return undefined
  const root = join(codexHome(ctx), 'sessions')
  const candidates: { path: string; at: number }[] = []
  for (const dir of dayDirs(root, startedAt)) {
    for (const name of await ctx.files.list(dir)) {
      if (!name.startsWith('rollout-') || !name.endsWith('.jsonl')) continue
      const path = join(dir, name)
      // session_meta is the first line; it carries the base instructions, so read generously.
      const head = (await ctx.files.readHead(path, 16 * 1024))?.split('\n', 1)[0]
      if (!head) continue
      const meta = sessionMeta(head)
      if (!meta || meta.cwd !== handle.cwd) continue
      const at = Date.parse(meta.timestamp)
      if (Number.isFinite(at) && at >= startedAt - 2000) candidates.push({ path, at })
    }
  }
  candidates.sort((a, b) => a.at - b.at)
  const claimed = new Set(located.values())
  const found = candidates.find((candidate) => !claimed.has(candidate.path))?.path
  if (found) located.set(handle.id, found)
  return found
}

/**
 * The working directory and start time of a rollout's first line. That line
 * embeds the base instructions and can be longer than the bytes read, so a
 * truncated line falls back to the first `cwd` and `timestamp` fields, which
 * come before the instructions.
 */
export function sessionMeta(head: string): { cwd: string; timestamp: string } | undefined {
  try {
    const meta = JSON.parse(head) as { type?: string; timestamp?: string; payload?: { cwd?: string; timestamp?: string } }
    if (meta.type !== 'session_meta' || typeof meta.payload?.cwd !== 'string') return undefined
    return { cwd: meta.payload.cwd, timestamp: meta.payload.timestamp ?? meta.timestamp ?? '' }
  } catch {
    if (!head.includes('"type":"session_meta"')) return undefined
    const cwd = /"cwd":("(?:[^"\\]|\\.)*")/.exec(head)?.[1]
    const timestamp = /"timestamp":"([^"]+)"/.exec(head)?.[1]
    if (!cwd || !timestamp) return undefined
    return { cwd: JSON.parse(cwd) as string, timestamp }
  }
}

/** Rollouts already matched to a handle in this process, so two agents in one directory get one each. */
const located = new Map<string, string>()

export const codex: Harness = {
  name: 'codex',
  title: 'Codex',
  bins: ['codex'],
  efforts: ['low', 'medium', 'high', 'xhigh', 'max'],
  verified: 'codex-cli 0.139-0.153; rollout events on 0.155.1',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('-c', `model_reasoning_effort="${request.effort}"`)
    if (request.yolo) argv.push('--dangerously-bypass-approvals-and-sandbox')
    // The "Hooks need review" modal cannot be answered from the keyboard, so a
    // spawned agent runs without the hook layer unless the program asks for it.
    if (request.named['hooks'] !== true) argv.push('--disable', 'hooks')
    argv.push(request.prompt)
    return { argv, unset: foreignMarkers() }
  },

  handleFields: (request) => ({ started_at: request.startedAt }),

  screen: {
    busy: [/esc to interrupt/i],
    idle: [/^\s*›/],
    dialogs: [
      { kind: 'trust', all: [/Do you trust the contents of this directory\?/], answer: ['Enter'] },
      { kind: 'hooks', all: [/Hooks need review/] },
    ],
  },

  transcript: { locate: locateRollout, parse: parseRollout },

  interrupt: { keys: ['Escape'], gapMs: 300 },
  submit: { commandPrefixes: ['/', '$'], commandSettleMs: 1200 },

  models: {
    source: '$CODEX_HOME/models_cache.json',
    async list(_bin, ctx) {
      const text = await ctx.files.read(join(codexHome(ctx), 'models_cache.json'))
      if (!text) return []
      const cache = JSON.parse(text) as { models?: Array<Record<string, unknown>> }
      return (cache.models ?? [])
        .filter((model) => typeof model['slug'] === 'string' && model['visibility'] !== 'hide')
        .map((model): ModelInfo => {
          const levels = model['supported_reasoning_levels']
          return {
            id: model['slug'] as string,
            ...(typeof model['display_name'] === 'string' ? { name: model['display_name'] } : {}),
            ...(Array.isArray(levels)
              ? { efforts: levels.map((level) => String((level as { effort?: unknown }).effort)).filter((e) => e !== 'undefined') }
              : {}),
          }
        })
    },
  },
}
