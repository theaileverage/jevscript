/**
 * The terminal backend layer: where an agent's pane lives.
 *
 * An `agent` adapter (spec section 9.1) runs an interactive CLI in a terminal
 * pane and reads it back. That pane can live in Herdr (the default), tmux,
 * cmux, Orca or Zellij; each backend below turns the same seven operations
 * into that multiplexer's own commands. The adapter never shells out to a
 * multiplexer itself, so a harness written once works on every backend.
 *
 * A backend returns a {@link PaneRef}: plain JSON that goes into the handle,
 * so a host that restarts can hand the handle back and reach the same pane.
 */

/** The backends this package drives. */
export type BackendName = 'herdr' | 'tmux' | 'cmux' | 'orca' | 'zellij'

export const BACKEND_NAMES: readonly BackendName[] = ['herdr', 'tmux', 'cmux', 'orca', 'zellij']

/** The keys an adapter ever presses on its own. */
export type Key = 'Enter' | 'Escape' | 'C-c' | 'C-u'

/** What to start in a new pane. */
export interface LaunchSpec {
  /** The working directory the agent starts in. */
  cwd: string
  /** The agent's argv: binary first, then its arguments, no shell involved. */
  argv: string[]
  /** Environment variables to set for the agent only. */
  env: Record<string, string>
  /** Environment variables to remove for the agent only (inherited markers). */
  unset: string[]
  /** A label for the tab or window, where the backend shows one. */
  title: string
  /** A backend session or namespace override (tmux session, Zellij session, Herdr session). */
  session?: string
}

/**
 * Where a pane lives, as JSON fields merged into the handle. Every backend but
 * tmux writes `backend`; a handle without one is a tmux handle, which is the
 * shape the original Claude Code adapter minted and still reattaches.
 */
export type PaneRef = Record<string, unknown>

/** Whether a pane is there and whether the agent in it has ended. */
export interface PaneState {
  /** The pane still exists. */
  exists: boolean
  /** The agent's process has ended, whether or not its pane remains. */
  exited: boolean
  /** The agent's exit status, when the backend could learn it. */
  exitCode: number | null
}

/** Herdr's own view of an agent in a pane (other backends have none). */
export type NativeStatus = 'working' | 'idle' | 'done' | 'blocked' | 'unknown'

/** What {@link TerminalBackend.probe} reports for discovery. */
export interface BackendProbe {
  name: BackendName
  /** The binary resolved on `PATH`. */
  installed: boolean
  path: string | null
  version: string | null
  /** The backend can create panes right now (server running, socket reachable). */
  ready: boolean
  /** Why it is not ready, in words. */
  detail: string | null
}

/** One terminal multiplexer, seen through the operations an adapter needs. */
export interface TerminalBackend {
  readonly name: BackendName
  /** Read-only availability check: never starts or stops anything. */
  probe(): Promise<BackendProbe>
  /** Start `spec` in a new pane without taking the user's focus. */
  create(spec: LaunchSpec): Promise<PaneRef>
  state(ref: PaneRef): Promise<PaneState>
  /** The visible screen as plain text; empty when the pane is gone. */
  capture(ref: PaneRef): Promise<string>
  /** Type one line literally, without submitting it. */
  type(ref: PaneRef, text: string): Promise<void>
  /** Deliver several lines as one bracketed paste, without submitting them. */
  paste(ref: PaneRef, text: string): Promise<void>
  key(ref: PaneRef, key: Key): Promise<void>
  /** Remove the pane. Safe to call again. */
  close(ref: PaneRef): Promise<void>
  /** Herdr only: the multiplexer's own agent status for the pane. */
  nativeStatus?(ref: PaneRef): Promise<NativeStatus | null>
}

/** The backend a handle names; a handle without `backend` is tmux (see {@link PaneRef}). */
export function backendOf(ref: PaneRef): BackendName {
  const name = ref['backend']
  return typeof name === 'string' && (BACKEND_NAMES as readonly string[]).includes(name)
    ? (name as BackendName)
    : 'tmux'
}
