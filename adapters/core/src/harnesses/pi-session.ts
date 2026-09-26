/**
 * Pi-family session files, shared by Pi and omp (a Pi fork).
 *
 * Both write `<agent dir>/sessions/<encoded cwd>/<ISO time>_<id>.jsonl`: a
 * a `session` line near the start naming the working directory, then one `message` line
 * per message, where an assistant message carries `stopReason` (`stop` when it
 * hands control back, `toolUse` while tools run, `aborted` on an interrupt).
 * Format checked on omp 18.x session files; Pi's is the same (omp.md: "omp is
 * a Pi fork").
 *
 * The file is found by its first line's `cwd` and a name time no earlier than
 * the spawn, so a handle needs only `cwd` and `started_at` to reattach.
 */
import { join } from 'node:path'

import type { AgentHandle, HarnessContext, TranscriptSummary } from '../harness.ts'

interface Block {
  type?: string
  text?: string
}

interface Line {
  type?: string
  message?: {
    role?: string
    content?: string | Block[]
    stopReason?: string
    usage?: { totalTokens?: number; input?: number; output?: number; cost?: { total?: number } }
  }
}

/** Summarise a Pi-family session's JSONL text. */
export function parsePiSession(jsonl: string | undefined): TranscriptSummary {
  const summary: TranscriptSummary = { lastMessage: '', idle: null, turns: 0, tokens: 0, usd: null }
  if (!jsonl) return summary
  for (const raw of jsonl.split('\n')) {
    if (!raw.trim()) continue
    let line: Line
    try {
      line = JSON.parse(raw) as Line
    } catch {
      continue // a line still being written
    }
    const message = line.message
    if (line.type !== 'message' || !message) continue
    if (message.role === 'user') {
      summary.idle = false
    } else if (message.role === 'assistant') {
      summary.turns += 1
      const usage = message.usage
      const tokens = usage?.totalTokens ?? (usage?.input ?? 0) + (usage?.output ?? 0)
      if (typeof tokens === 'number') summary.tokens += tokens
      const cost = usage?.cost?.total
      if (typeof cost === 'number') summary.usd = (summary.usd ?? 0) + cost
      const done = message.stopReason === 'stop' || message.stopReason === 'aborted' || message.stopReason === 'error'
      summary.idle = done
      const text = textOf(message.content)
      if (done && text !== '') summary.lastMessage = text
    }
  }
  return summary
}

function textOf(content: string | Block[] | undefined): string {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((block) => block.type === 'text' && typeof block.text === 'string')
    .map((block) => block.text as string)
    .join('\n\n')
}

/** `2026-08-27T08-45-40-071Z_<id>.jsonl` to milliseconds. */
function nameTime(name: string): number {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})-(\d{3})Z_/.exec(name)
  if (!match) return Number.NaN
  return Date.parse(`${match[1]}T${match[2]}:${match[3]}:${match[4]}.${match[5]}Z`)
}

const located = new Map<string, string>()

/**
 * The session file for a handle under `sessionsRoot`: the earliest one whose
 * initial metadata names the handle's working directory and that started no earlier
 * than the spawn, not already matched to another handle in this process.
 */
export async function locatePiSession(
  sessionsRoot: string,
  handle: AgentHandle,
  ctx: HarnessContext,
): Promise<string | undefined> {
  const known = located.get(`${sessionsRoot}:${handle.id}`)
  if (known) return known
  const startedAt = Number(handle['started_at'])
  if (!Number.isFinite(startedAt)) return undefined
  const candidates: { path: string; at: number }[] = []
  for (const dir of await ctx.files.list(sessionsRoot)) {
    for (const name of await ctx.files.list(join(sessionsRoot, dir))) {
      if (!name.endsWith('.jsonl')) continue
      const at = nameTime(name)
      if (!Number.isFinite(at) || at < startedAt - 2000) continue
      const path = join(sessionsRoot, dir, name)
      // omp 18.2 can prepend a short `title` line before the `session` line.
      const head = await ctx.files.readHead(path, 8 * 1024)
      for (const raw of head?.split('\n').slice(0, 8) ?? []) {
        try {
          const metadata = JSON.parse(raw) as { type?: string; cwd?: string }
          if (metadata.type === 'session' && metadata.cwd === handle.cwd) {
            candidates.push({ path, at })
            break
          }
        } catch {
          // An incomplete line does not erase earlier metadata.
        }
      }
    }
  }
  candidates.sort((a, b) => a.at - b.at)
  const claimed = new Set(located.values())
  const found = candidates.find((candidate) => !claimed.has(candidate.path))?.path
  if (found) located.set(`${sessionsRoot}:${handle.id}`, found)
  return found
}
