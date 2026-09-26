/**
 * An `agent` adapter for Claude Code (spec section 9.1).
 *
 * This package is the Claude Code member of the adapter suite: the verbs,
 * backends and transcript reading live in `@jevscript/adapter-core`, whose
 * `claude` harness holds every Claude-specific fact. What stays here is the
 * adapter's original public surface, kept exactly: the same class, options,
 * handle shape and exports, and tmux as the default backend when used as a
 * library (Herdr, cmux, Orca and Zellij are one `backend` option away). The
 * `jevscript-adapter-claude-code` executable defaults to Herdr like every
 * other adapter in the suite.
 *
 * Nothing this adapter returns is interpreted by it. `tail` and `last_message`
 * are agent-written text and go to the program as they are (README, spec
 * section 7.5).
 */
import {
  AgentAdapter,
  type AgentAdapterOptions,
  type AgentObservation,
  claude,
  type Clock,
  defaultFiles,
  type Exec,
  type Files,
  TmuxBackend,
} from '@jevscript/adapter-core'
import type { Handle } from 'jevscript'

export {
  AdapterError,
  boundTail,
  type Clock,
  type Exec,
  type ExecResult,
  claudeTranscriptPath as transcriptPath,
  parseClaudeTranscript as parseTranscript,
  promptVisible,
  TAIL_LIMIT,
  type ClaudeTranscriptSummary as TranscriptSummary,
} from '@jevscript/adapter-core'

/** How the adapter runs Claude Code. Every option of the original adapter is kept. */
export interface ClaudeCodeOptions extends AgentAdapterOptions {
  /**
   * The capability name the program bound this adapter under, when the SDK
   * does not say. A handle has to carry one (spec section 9). Defaults to `claude`.
   */
  capability?: string
  /** The tmux session to create panes in (the backend session on other backends). */
  session?: string
  /** The Claude Code binary. Defaults to `claude` on the PATH. */
  bin?: string
  /** Extra arguments passed to every spawn, before the prompt. */
  args?: string[]
  /** How long `wait idle` lets a pane go unchanged before calling it idle. */
  idleSeconds?: number
  /** How often `wait` looks at the pane, in milliseconds. */
  pollMs?: number
  /** The tmux binary. Defaults to `tmux` on the PATH. */
  tmux?: string
  /** Where Claude Code keeps its state. Defaults to `$CLAUDE_CONFIG_DIR`, then `~/.claude`. */
  configDir?: string
  /** Runs tmux (and every other command). Defaults to `node:child_process`. */
  exec?: Exec
  /** Reads a transcript; `undefined` when there is none yet. */
  readFile?: (path: string) => Promise<string | undefined>
  /** Time. Defaults to the real clock. */
  clock?: Clock
  /** Fresh Claude session ids. Defaults to `randomUUID`. */
  uuid?: () => string
}

/** What this adapter puts in a tmux handle, on top of the required fields. */
export interface PaneHandle extends Handle {
  /** The tmux session the pane lives in. */
  session: string
  /** The tmux pane id, such as `%7`. */
  pane: string
  /** The working directory the agent was started in. */
  cwd: string
  /** The Claude Code session id, which names the transcript. */
  session_id: string
}

/** What this adapter's observations carry beyond the required fields. */
export interface PaneObservation extends AgentObservation {
  session_id: string
  /** Assistant messages so far, from the transcript. */
  turns: number
  /** What the agent reported spending; `usd` is null when the transcript has no cost. */
  usage: { tokens: number; usd: number | null }
}

/**
 * The adapter, as a host binds it:
 *
 * ```ts
 * const run = program.task('main').start({
 *   bind: { claude: new ClaudeCodeAdapter({ session: 'jevscript' }) },
 * })
 * ```
 */
export class ClaudeCodeAdapter extends AgentAdapter {
  declare readonly options: ClaudeCodeOptions

  constructor(options: ClaudeCodeOptions = {}) {
    super(claude, adapterOptions(options), 'tmux')
  }

  override async spawn(...args: Parameters<AgentAdapter['spawn']>): Promise<PaneHandle> {
    return (await super.spawn(...args)) as unknown as PaneHandle
  }

  override async observe(handle: Handle): Promise<PaneObservation> {
    return (await super.observe(handle)) as PaneObservation
  }

  override async wait(handle: Handle, minutes: number, condition = 'idle'): Promise<PaneObservation> {
    return (await super.wait(handle, minutes, condition)) as PaneObservation
  }
}

/** Map the original options onto the core's: `readFile` reads transcripts, `tmux` names the tmux binary. */
function adapterOptions(options: ClaudeCodeOptions): AgentAdapterOptions {
  const files: Files | undefined = options.readFile ? { ...defaultFiles, read: options.readFile } : options.files
  const mapped: AgentAdapterOptions = { ...options, capability: options.capability ?? 'claude' }
  if (files) mapped.files = files
  if (options.tmux && options.backend === undefined) {
    mapped.backend = new TmuxBackend({
      bin: options.tmux,
      ...(options.session ? { session: options.session } : {}),
      ...(options.exec ? { exec: options.exec } : {}),
      ...(options.clock ? { clock: options.clock } : {}),
      ...(files ? { files } : {}),
    })
  }
  return mapped
}
