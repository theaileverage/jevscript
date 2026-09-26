# Agent adapters

An agent adapter is a host binding for Jevscript's `agent` capability (spec
section 9.1). The runtime asks the host for `spawn`, `observe`, `send`, `wait`
and `stop`; the host selects an adapter independently of the agent's language.
Handles are JSON records with a terminal pane reference, so a later process
can reattach.

Each package exports a TypeScript class and a persistent JSONL executable. The
executable speaks the protocol in [`crates/jevscript-cli/README.md`](../crates/jevscript-cli/README.md).

| CLI | Package | Executable |
| --- | --- | --- |
| Claude Code | `@jevscript/adapter-claude-code` | `jevscript-adapter-claude-code` |
| Codex | `@jevscript/adapter-codex` | `jevscript-adapter-codex` |
| OpenCode | `@jevscript/adapter-opencode` | `jevscript-adapter-opencode` |
| Pi | `@jevscript/adapter-pi` | `jevscript-adapter-pi` |
| omp | `@jevscript/adapter-omp` | `jevscript-adapter-omp` |
| Antigravity | `@jevscript/adapter-agy` | `jevscript-adapter-agy` |
| Cursor | `@jevscript/adapter-cursor` | `jevscript-adapter-cursor` |
| Gemini CLI | `@jevscript/adapter-gemini` | `jevscript-adapter-gemini` |
| Grok | `@jevscript/adapter-grok` | `jevscript-adapter-grok` |
| Kimi | `@jevscript/adapter-kimi` | `jevscript-adapter-kimi` |
| Devin | `@jevscript/adapter-devin` | `jevscript-adapter-devin` |
| Rovo | `@jevscript/adapter-rovo` | `jevscript-adapter-rovo` |
| Muse | `@jevscript/adapter-muse` | `jevscript-adapter-muse` |

Executables default to Herdr. Pass `--backend tmux`, `cmux`, `orca` or `zellij`
for another terminal. `jevscript-agent discover --models` lists installed
CLIs, versions, supported effort levels, available model listings and backend
readiness. An individual executable accepts `--discover --models`. A null
model list means no verified listing surface; model availability varies by
account and installed CLI version.

## Bind an executable

After `pnpm install && pnpm run build` in `adapters/`:

```sh
jevscript run examples/review_loop.jev --bind claude='jevscript-adapter-codex --backend tmux'
```

The name before `=` is the program's capability name. Any `agent` executable
can serve it. JavaScript hosts can use
`subprocessAgent('jevscript-adapter-codex', ['--backend', 'tmux'])`; Python hosts
can use `subprocess_agent('jevscript-adapter-codex', ['--backend', 'tmux'])`.
Each helper owns one persistent process. Close it when done. Closing leaves
agent panes in place; call the `stop` verb to stop an agent.

## Write a custom adapter

Implement a `Harness` and reuse the core. The core implements the five spec
verbs, JSON handles, safe text submission, busy and turn-end detection, trust
dialog hooks, the JSONL server and the five terminal backends.

```ts
import { AgentAdapter, runAdapterCli, type Harness } from '@jevscript/adapter-core'

const myCli: Harness = {
  name: 'my-cli', title: 'My CLI', bins: ['my-cli'], efforts: null,
  verified: 'your tested CLI version',
  launch(request) {
    return { argv: [request.bin, '--prompt', request.prompt], unset: [] }
  },
  screen: { busy: [/Working/], idle: [/Ready/] },
  interrupt: { keys: ['Escape'], gapMs: 300 },
}

export class MyCliAdapter extends AgentAdapter {
  constructor() { super(myCli) }
}

// In the executable: stdout must contain only JSONL replies.
process.exitCode = await runAdapterCli(myCli)
```

`spawn` passes adapter-specific named arguments in `request.named`. A harness
may add transcript parsing, model discovery, trust handling and flags. Put CLI
notices on stderr, never stdout. Test against `FakeBackend` from
`@jevscript/adapter-core/testing` and test the built executable over JSONL.

The catalog records launch flags, busy markers and trust dialogs for the CLI
versions named by each harness. Backend implementations record the terminal
commands they use. Recheck these behaviors for new CLI versions.

## Checks

`pnpm test`, `pnpm run typecheck` and `pnpm run build` run without live agent
CLIs. `pnpm run smoke` is opt-in: it starts real agent CLIs and terminal panes.
Herdr live smoke also needs `JEVSCRIPT_HERDR_LAB=1`, so the ordinary check
does not run it.

### Local live evidence (2026-09-26)

On tmux, the opt-in smoke completed a trivial `JEVSCRIPT_SMOKE_OK` turn for
Claude Code, Codex, Pi, omp and agy. The installed OpenCode 2.0.6 failed a
bounded direct control, `opencode run`, with `Error: User not found.`; its
interactive turn remains unverified. Cursor Agent 2026.05.09 displayed the
workspace trust choice, accepted Enter, then reported `Error: Named models
unavailable`; its account cannot run a model turn. The trust interaction is
also covered by a fake-backend regression test. Gemini, Grok, Kimi, Devin,
Rovo and Muse were not installed, so their live tests skipped. No Herdr live
smoke was run.
