/**
 * Reading Claude Code's own transcript, so that `last_message` and the
 * `waiting` status come from what the agent actually said rather than from a
 * screen that redraws (spec section 9.1, README "The observation record").
 *
 * Claude Code appends one JSONL line per content block of each API message, so
 * one assistant message is several lines sharing a `message.id`; lines for a
 * message are written once it has finished, so `stop_reason` marks it as
 * complete. Tool results come back as `user` lines whose content is a list of
 * `tool_result` blocks. Everything here is agent-written text and is passed
 * through untouched: nothing in it is ever interpreted as an instruction.
 */

/** What one transcript says about the agent, as far as the observation needs. */
export interface TranscriptSummary {
  /** The text of the most recent complete assistant message that had any text. */
  lastMessage: string
  /**
   * True when the last turn is a complete assistant message with no tool use
   * left pending, i.e. the agent has handed control back to the user.
   */
  idle: boolean
  /** How many assistant messages the transcript holds. */
  turns: number
  /** Input, output and cache tokens summed over every assistant message. */
  tokens: number
  /** Dollars, only if the transcript reports them; Claude Code 2.x does not. */
  usd: number | null
}

interface ContentBlock {
  type?: string
  text?: string
  id?: string
  tool_use_id?: string
}

interface Line {
  type?: string
  isSidechain?: boolean
  isMeta?: boolean
  costUSD?: number
  message?: {
    id?: string
    role?: string
    content?: string | ContentBlock[]
    stop_reason?: string | null
    usage?: Record<string, unknown>
  }
}

interface AssistantMessage {
  id: string
  texts: string[]
  toolUses: Set<string>
  stopReason: string | null
  tokens: number
}

const EMPTY: TranscriptSummary = { lastMessage: '', idle: false, turns: 0, tokens: 0, usd: null }

/** Summarise a transcript's JSONL text. A missing transcript is an empty one. */
export function parseTranscript(jsonl: string | undefined): TranscriptSummary {
  if (!jsonl) return EMPTY

  const messages: AssistantMessage[] = []
  const byId = new Map<string, AssistantMessage>()
  const resolvedToolUses = new Set<string>()
  let last: 'assistant' | 'user' | 'none' = 'none'
  let usd: number | null = null

  for (const raw of jsonl.split('\n')) {
    if (!raw.trim()) continue
    let line: Line
    try {
      line = JSON.parse(raw) as Line
    } catch {
      continue // a partial line still being written, or not ours to read
    }
    if (line.isSidechain) continue // a subagent's turns, not the agent's own
    if (typeof line.costUSD === 'number') usd = (usd ?? 0) + line.costUSD

    const message = line.message
    if (line.type === 'assistant' && message) {
      const id = message.id ?? `line:${messages.length}`
      let current = byId.get(id)
      if (!current) {
        current = { id, texts: [], toolUses: new Set(), stopReason: null, tokens: 0 }
        byId.set(id, current)
        messages.push(current)
        current.tokens = tokensOf(message.usage)
      }
      if (message.stop_reason) current.stopReason = message.stop_reason
      for (const block of blocksOf(message.content)) {
        if (block.type === 'text' && typeof block.text === 'string') current.texts.push(block.text)
        if (block.type === 'tool_use' && block.id) current.toolUses.add(block.id)
      }
      last = 'assistant'
    } else if (line.type === 'user' && message) {
      const blocks = blocksOf(message.content)
      const results = blocks.filter((b) => b.type === 'tool_result')
      for (const result of results) if (result.tool_use_id) resolvedToolUses.add(result.tool_use_id)
      const isPrompt = typeof message.content === 'string' || blocks.length > results.length
      if (isPrompt && !line.isMeta) last = 'user'
    }
  }

  const latest = messages.at(-1)
  const pending = latest ? [...latest.toolUses].some((id) => !resolvedToolUses.has(id)) : false
  const idle =
    last === 'assistant' &&
    latest !== undefined &&
    latest.stopReason !== null &&
    latest.stopReason !== 'tool_use' &&
    !pending

  const spoken = messages.filter((m) => m.stopReason !== null && m.texts.length > 0).at(-1)

  return {
    lastMessage: spoken ? spoken.texts.join('\n\n') : '',
    idle,
    turns: messages.length,
    tokens: messages.reduce((sum, m) => sum + m.tokens, 0),
    usd,
  }
}

function blocksOf(content: string | ContentBlock[] | undefined): ContentBlock[] {
  if (typeof content === 'string') return [{ type: 'text', text: content }]
  return Array.isArray(content) ? content : []
}

function tokensOf(usage: Record<string, unknown> | undefined): number {
  if (!usage) return 0
  let total = 0
  for (const key of [
    'input_tokens',
    'output_tokens',
    'cache_creation_input_tokens',
    'cache_read_input_tokens',
  ]) {
    const n = usage[key]
    if (typeof n === 'number') total += n
  }
  return total
}

/**
 * Where Claude Code keeps the transcript for a session started in `cwd`: every
 * character of the directory that is not alphanumeric becomes a dash
 * (verified against Claude Code 2.1: `/Users/x/.codex` becomes `-Users-x--codex`).
 */
export function transcriptPath(configDir: string, cwd: string, sessionId: string): string {
  const encoded = cwd.replace(/[^a-zA-Z0-9]/g, '-')
  return `${configDir}/projects/${encoded}/${sessionId}.jsonl`
}
