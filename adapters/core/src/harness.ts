/**
 * A harness: everything that is specific to one agent CLI.
 *
 * The `agent` verbs of spec section 9.1 are the same for every CLI; what
 * differs is how the CLI is launched, what its screen looks like when it is
 * busy or waiting for input, which dialogs can stop it before it starts, and
 * where (if anywhere) it writes a transcript. A {@link Harness} is that
 * knowledge as data plus a few small functions, and {@link AgentAdapter}
 * (`agent.ts`) is the one implementation of the verbs that reads it.
 *
 * The facts in `harnesses/*.ts` describe versioned CLI launch, busy-state and
 * composer behavior; each harness records its version and evidence scope.
 */
import type { Key } from './backends/types.ts'
import type { Exec, Files } from './process.ts'

/**
 * How an adapter treats a folder-trust gate:
 *
 * - `off` (the default): nothing. A dialog on screen is reported as the
 *   observation's `dialog` and the agent stays `running`.
 * - `dialog`: use the CLI's verified launch flag that suppresses the gate
 *   (Cursor's `--trust`), and answer a trust dialog whose safe choice is
 *   preselected (Codex, Pi, Antigravity, Grok, Kimi) with its verified key.
 * - `register`: `dialog`, and first record the directory in the CLI's own
 *   trust store where that is verified (Claude Code's `~/.claude.json`,
 *   Antigravity's `settings.json`). This writes the user's configuration, so it
 *   is never the default.
 */
export type TrustMode = 'off' | 'dialog' | 'register'

export const TRUST_MODES: readonly TrustMode[] = ['off', 'dialog', 'register']

/** One `spawn`, normalised: what a harness builds its command line from. */
export interface SpawnRequest {
  /** The resolved binary. */
  bin: string
  prompt: string
  cwd: string
  /** A fresh id the adapter minted; harnesses that can name their session use it. */
  sessionId: string
  model: string | undefined
  /** The effort level, already checked against {@link Harness.efforts}. */
  effort: string | undefined
  /** Approve the agent's tool use without asking (each CLI's verified autonomy flag). */
  yolo: boolean
  trust: TrustMode
  /** Extra flags: the adapter's `args` option, then the spawn's `args`. */
  args: string[]
  /** Every named argument of the spawn, for harness-specific ones. */
  named: Record<string, unknown>
  /** When the spawn happened, in milliseconds. */
  startedAt: number
}

/** What a harness wants started. */
export interface LaunchPlan {
  argv: string[]
  env?: Record<string, string>
  unset?: string[]
  /**
   * The CLI takes no prompt on its command line (Kimi, Rovo): the adapter
   * waits for {@link Harness.ready} and types the prompt instead.
   */
  typePrompt?: boolean
}

/** A dialog that can stop an agent before or during a turn. */
export interface DialogRule {
  /** What the program sees in the observation's `dialog` field. */
  kind: 'trust' | 'auth' | 'permission' | 'imports' | 'hooks' | 'setup'
  /** Every pattern must match the screen for the dialog to count as showing. */
  all: readonly RegExp[]
  /**
   * The verified keys that accept it, only for a dialog whose safe choice is
   * preselected. Pressed only when trust handling is on and `kind` is `trust`.
   */
  answer?: readonly Key[]
}

/** How to read a harness's screen. */
export interface ScreenRules {
  /** Any match in the last {@link scanLines} non-blank lines means a turn is running. */
  busy: readonly RegExp[]
  /**
   * The input box is showing and empty: patterns matched in the last
   * {@link scanLines} non-blank lines, or a function over the whole screen.
   */
  idle: readonly RegExp[] | ((screen: string) => boolean)
  /** Defaults to 12 rendered lines when scanning for busy markers. */
  scanLines?: number
  dialogs?: readonly DialogRule[]
}

/** What a transcript says, as far as an observation needs. */
export interface TranscriptSummary {
  /** The agent's most recent complete message to the user (spec section 9.1). */
  lastMessage: string
  /**
   * True when the agent has handed control back, false while a turn is open,
   * null when the transcript cannot tell.
   */
  idle: boolean | null
  turns: number
  tokens: number
  /** Dollars, only if the transcript reports them. */
  usd: number | null
}

/** Everything a harness hook may touch. */
export interface HarnessContext {
  exec: Exec
  files: Files
  home: string
  env: Record<string, string | undefined>
  /** The adapter's options, for harness-specific ones such as Claude's `configDir`. */
  options: Record<string, unknown>
}

/** A handle as a harness sees it: the adapter's fields and its own. */
export type AgentHandle = Record<string, unknown> & { capability: string; id: string; cwd: string }

/** Where a harness's transcript lives and how to read it. */
export interface TranscriptSource {
  /** The transcript file for this handle, or `undefined` while there is none yet. */
  locate(handle: AgentHandle, ctx: HarnessContext): Promise<string | undefined>
  parse(text: string | undefined): TranscriptSummary
}

/** The signals `status` is decided from. */
export interface Signals {
  screen: string
  busy: boolean
  idle: boolean
  dialog: DialogRule['kind'] | null
  transcript: TranscriptSummary | undefined
  native: string | null
}

/** One model a CLI offers. */
export interface ModelInfo {
  id: string
  name?: string
  /** Effort levels this model accepts, when the CLI says. */
  efforts?: string[]
}

/** How a harness lists its models, when it can. */
export interface ModelSource {
  /** Where the list comes from, in words, for discovery output. */
  source: string
  list(bin: string, ctx: HarnessContext): Promise<ModelInfo[]>
}

/** One agent CLI. */
export interface Harness {
  /** The short name hosts use: `claude`, `codex`, `agy`. */
  name: string
  /** The product name. */
  title: string
  /** Binaries to look for, in order. */
  bins: readonly string[]
  /** Arguments that print the version. Defaults to `--version`. */
  versionArgs?: readonly string[]
  /** Accepted effort levels, or `null` when the CLI has no verified effort flag. */
  efforts: readonly string[] | null
  /** Which facts are verified, and on what version. */
  verified: string
  launch(request: SpawnRequest): LaunchPlan
  /** Handle fields of the harness's own, such as Claude's `session_id`. */
  handleFields?(request: SpawnRequest): Record<string, unknown>
  /** Refuse a handle this harness did not make; throw an `AdapterError`. */
  validate?(handle: AgentHandle): void
  /** Runs before launch when trust is `register`: record the directory in the CLI's trust store. */
  register?(request: SpawnRequest, ctx: HarnessContext): Promise<void>
  screen: ScreenRules
  transcript?: TranscriptSource
  /** Decide `running` or `waiting`; the default is {@link defaultClassify}. */
  classify?(signals: Signals): 'running' | 'waiting'
  /** Observation fields of the harness's own. */
  observationFields?(handle: AgentHandle, transcript: TranscriptSummary | undefined): Record<string, unknown>
  /** What `stop` presses before closing the pane. */
  interrupt: { keys: readonly Key[]; gapMs: number }
  /** The prompt can be typed once one of these shows (for {@link LaunchPlan.typePrompt}). */
  ready?: readonly RegExp[]
  submit?: {
    /** Between typing and Enter. Defaults to 300 ms. */
    settleMs?: number
    /** Leading characters that open a popup (`/`, Codex's `$`), and the longer settle they need. */
    commandPrefixes?: readonly string[]
    commandSettleMs?: number
    /** The popup eats the first Enter (Cursor), so a command gets a second one. */
    commandExtraEnter?: boolean
  }
  models?: ModelSource
}

/**
 * The default `status` rule: a dialog or any busy sign means `running`; an
 * idle transcript, or an empty input box with nothing busy, means `waiting`;
 * anything else is `running`, because a turn that cannot be seen to have ended
 * has not ended as far as a program can know.
 */
export function defaultClassify(signals: Signals): 'running' | 'waiting' {
  if (signals.dialog) return 'running'
  if (signals.transcript?.idle === false) return 'running'
  if (signals.busy) return 'running'
  if (signals.native === 'working') return 'running'
  if (signals.transcript?.idle === true) return 'waiting'
  return signals.idle ? 'waiting' : 'running'
}

/**
 * Environment variables other agent CLIs set to say "you are running inside
 * me". A child that inherits one can misidentify itself, so each harness
 * clears markers for the other CLIs.
 */
export const HARNESS_MARKERS = [
  'CLAUDECODE',
  'PI_CODING_AGENT',
  'GROK_AGENT',
  'GEMINI_CLI',
  'CURSOR_AGENT',
  'CURSOR_INVOKED_AS',
  'ATLASSIAN_AGENT_TYPE',
  'ROVODEV_CLI',
] as const

/** Every marker except the ones a harness sets itself. */
export function foreignMarkers(...own: string[]): string[] {
  return HARNESS_MARKERS.filter((name) => !own.includes(name))
}
