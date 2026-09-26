/**
 * `@jevscript/adapter-core`: the shared core of the Jevscript `agent`
 * adapter suite (spec section 9.1).
 *
 * - {@link AgentAdapter} implements `spawn`, `observe`, `send`, `wait` and
 *   `stop` once, for any {@link Harness} on any terminal backend.
 * - `harnesses/` holds one {@link Harness} per agent CLI, with the verified
 *   facts it runs on; {@link HARNESSES} is the catalog.
 * - `backends/` drives Herdr (the default), tmux, cmux, Orca and Zellij.
 * - {@link discover} reports which CLIs and backends a machine has.
 * - {@link serveJsonl} serves the CLI's JSONL subprocess protocol (spec
 *   section 11.6) from any adapter, which is what every adapter executable does.
 */
export { AgentAdapter, type AgentAdapterOptions, type AgentObservation } from './agent.ts'
export * from './args.ts'
export * from './backends/index.ts'
export { optionsFrom, runAdapterCli, runAgentCli } from './cli.ts'
export {
  type AgentDiscovery,
  discover,
  discoverAll,
  discoverHarness,
  type DiscoverOptions,
} from './discover.ts'
export { AdapterError } from './errors.ts'
export * from './harness.ts'
export * from './harnesses/index.ts'
export { answer, type JsonlIo, serveJsonl } from './jsonl.ts'
export {
  type Clock,
  defaultClock,
  defaultExec,
  defaultFiles,
  type Exec,
  type ExecOptions,
  type ExecResult,
  type Files,
  shellQuote,
  which,
} from './process.ts'
export { boundTail, lastLines, matchesNear, safeText, stripAnsi, TAIL_LIMIT } from './screen.ts'
