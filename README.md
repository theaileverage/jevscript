# Jevscript

Python package `jevscript` 0.1.2 is published on PyPI. The JavaScript SDK and
CLI are prepared here as `@theaileverage/jevscript` 0.1.3; **the scoped npm
package is not published yet**. The pushed `v0.1.2` tag and PyPI release remain
unchanged. The public source is [theaileverage/jevscript](https://github.com/theaileverage/jevscript).

https://jevscript.sh

A small language for agent control loops, judged by [Jev](https://docs.typesafe.ai),
TypeSafe's System One model.

Jev reads one structured state and answers typed questions with probabilities,
in parallel. It does not generate text and it does not count. A Jevscript
program owns the loop, the thresholds, the budgets and the policy; Jev owns the
snap judgments; text models and terminal agents are capabilities the host binds
at run time.

```
program inbox_triage

in message: text

judgment triage(message):
  urgent  = message feels "needs a response within the hour"
  owner   = message pick:
    code      "asks for a code change or reports a bug"
    customer  "a customer asking for help or a reply"
    schedule  "asks to find or move a meeting time"
    other
  risky   = message feels "acting on it could send, pay, delete or commit something"
```

Three typed answers, one model call, no parsing.

## Install the SDK and CLI

For the published Python package (Python 3.10+):

```sh
python -m pip install 'jevscript==0.1.2'  # or: pipx install 'jevscript==0.1.2'
jevscript --version
```

After the scoped npm package is published (Node 22+):

```sh
npm install @theaileverage/jevscript@0.1.3
npm exec --package=@theaileverage/jevscript@0.1.3 -- jevscript --version
```

Both SDKs use their bundled, release-matched Rust CLI for the `jevscript serve`
stdio protocol; installation needs no Rust toolchain, install hook or binary
download. In JavaScript, import from `@theaileverage/jevscript`; in Python,
import from `jevscript`. The `jevscript` command also supports `check`, `run`,
`replay` and `lsp`. See the [npm SDK](sdk/js/README.md),
[Python SDK](sdk/python/README.md), and [release guide](docs/package-release.md)
for API examples, supported platforms and artifact verification.

Install the bundled coding-agent Skill explicitly in a project:

```sh
jevscript setup --agent codex --agent claude-code
```

The project Skill lives at `.agents/skills/jevscript`; Claude Code also gets a
link at `.claude/skills/jevscript`. Use `--project <dir>` for another existing
project, `--global` for a user-level installation, or `--copy` for independent
copies instead of links. Global setup links into `~/.codex/skills/jevscript`
and/or `~/.claude/skills/jevscript`. Setup runs offline, refuses unmanaged or
changed destinations, and never runs during package installation.

## The loop

Every Jevscript program is the same shape, whatever it drives:

1. **Observe.** A capability reports what happened — an agent's transcript, a
   diff, a test summary.
2. **Shape.** `shape` caps every field that will reach Jev, in tokens. The
   context window is the constraint, so staying inside it is the default path,
   not a repair step.
3. **Judge.** `feels`, `pick` and `rate` ask Jev questions whose answers were
   declared in advance. Questions that do not depend on each other batch into
   one request, and the runtime is charged per request, not per question — so
   asking every question a branch might need up front is the encouraged style.
4. **Decide.** Every branch, threshold, weight and count is written in
   Jevscript. A `gate` turns judgments into one of four verdicts using
   thresholds declared once per task.
5. **Act, and prove it.** An agent's claim of completion and Jev's estimate that
   the goal is met are both signals. `verify` over observed state is the only
   proof.

## The four units

| Unit | Is | Can |
| --- | --- | --- |
| `judgment` | A named, pure block of questions | One Jev request, except when an oversized `each` requires chunks. No side effects, no loops. Runnable on its own against fixture state, which is how questions get evaluated against labelled examples. |
| `task` | The unit a host runs | Observe, judge, gate, act, under declared budgets. Pauses the run at gates and human asks. |
| `machine` | A state machine Jev steps | States and described events. Every step asks one Choice over exactly the events whose code guards are true, plus `stay`. Jev only ever picks a legal transition. |
| `def` | A pure helper | Judgments, control flow, arithmetic. No capabilities, no gates, no pauses. |

A `task` is right when the loop is short and the branching is over content. A
`machine` is right when the run has distinct phases, when the legal next moves
depend on the phase, or when a host wants to draw and pin the control flow. A
machine's terminal state counts as `verified` only when the transition into it
carried a `when` guard — code's say-so, not Jev's.

Programs are values a host drives. A run is a state machine with seven typed
pause points — `confirm`, `escalate`, `waiting`, `budget`, `error`, `stopped`,
`done` — and the host steps it, answers it and can replay it. Every judgment,
generation, capability result and random draw is written to a JSONL recording,
and a recording replays with zero model calls and identical control flow.

`log info "routed request" { owner, confidence: c.confidence }` writes a line to
that recording, and `x = log debug classify(message)` logs a value and returns
it, so a log can wrap any expression. The levels are `debug`, `info`, `warn` and
`error`. A log never reaches Jev, never counts against a budget and never
changes control flow; it is allowed in every unit. Hosts receive each line
through the SDKs' log callback, a replay checks recorded lines without emitting
them again, and `--redact` stores their values as hashes.

Larger systems are composed from files: `use "./lib/agent_loop.jev" as harness with
claude, tree` imports another module, and the compiler links everything into one
flat program whose units are qualified (`harness.read_agent`). There is no registry
and no version syntax.

## Repo map

```
spec/        the language specification. The authority, maintained by the implementation lead.
crates/
  jevscript-syntax/     lexer, parser, AST
  jevscript-ir/         the versioned IR, with serde and a JSON Schema generator
  jevscript-compiler/   AST -> IR: linking, batching, checks
  jevscript-runtime/    executes IR, records and replays, talks to Jev
  jevscript-cli/        the `jevscript` binary
  jevscript-lsp/        the language server behind `jevscript lsp`
editors/
  vscode/      a VS Code extension (Cursor and Windsurf too) that runs the language server
  zed/         a Zed extension that runs the language server
sdk/
  js/          JavaScript host SDK
  python/      Python host SDK
adapters/
  claude-code/ an `agent` adapter for Claude Code in a tmux pane
examples/
  fix_issue.jev        a coding harness, importing the library below
  lib/agent_loop.jev   the harness loop as a reusable library
  inbox_triage.jev     a judgment-only program
  review_loop.jev      a review machine
  chief-of-staff/      a runnable host showcase with its own Python CLI
```

## Build and run

```sh
cargo build --workspace          # build the compiler, runtime and CLI
cargo test --workspace           # run the Rust tests
cargo run -p jevscript-cli -- --help
```

The binary lands at `target/debug/jevscript`:

| Command | Does |
| --- | --- |
| `jevscript compile <file.jev>` | Emit the IR as JSON. |
| `jevscript check <file.jev> [--tools <manifest.json>]` | Report warnings and errors without emitting IR, and check tool verbs against an adapter manifest. |
| `jevscript judge <file.jev> <judgment> --state <json>` | Run one judgment. Needs `TYPESAFE_API_KEY`. |
| `jevscript eval <file.jev> <judgment> --cases <jsonl>` | Score a judgment against labelled rows. |
| `jevscript run <file.jev> --input <json> --record <path>` | Run `main`, answering pauses on the terminal and printing `log` lines on stderr. |
| `jevscript replay <recording.jsonl>` | Replay with no model calls, reproducing the recorded `log` lines. |
| `jevscript serve` | The JSON-RPC stdio server the SDKs drive. |
| `jevscript lsp` | The language server for editors, over stdio. |
| `jevscript setup --agent codex` | Install the bundled coding-agent Skill in a project. |

Bind each capability explicitly when running a task:

```sh
jevscript run program.jev --input '{"issue":"Fix the parser"}' \
  --bind tree='./tree-adapter' --record run.jsonl
jevscript replay run.jsonl
```

Adapter commands stay open for the run and exchange JSONL over stdio; see the
[adapter protocol](crates/jevscript-cli/README.md). `--stub NAME` explicitly
selects a demonstration adapter. Recordings contain the linked program and
resolved profile, so replay needs neither source files nor an API key. Choose a
new output path for each recording; existing files are never overwritten or
appended to.

`--redact` hides state, observations and `log` values in the primary recording and writes the
full sensitive replay data to `<path>.replay.jsonl` with owner-only permissions
where supported. Keep both files to replay. Inputs, outputs, call arguments and
generated text remain visible in the primary file. Recording and replay options
cannot be combined in one run.

The SDKs spawn `jevscript serve` and speak JSON-RPC 2.0 to it over stdio:

```sh
cd sdk/js && pnpm install && pnpm test
python3 -m pytest sdk/python
```

Point them at a binary other than `jevscript` on the PATH with `JEVSCRIPT_BIN`.

Editors get diagnostics, highlighting, hover, navigation and completion from
`jevscript lsp`: see [editor support](docs/editors.md) for VS Code, Cursor,
Windsurf, Neovim, Helix and Zed.

For a larger application built on the same runtime, see the
[Chief of Staff example](examples/chief-of-staff/README.md). It has a separate
Python host, durable state and terminal-agent backends; its source checkout
setup is described there.

## Configuration

| Variable | Does |
| --- | --- |
| `TYPESAFE_API_KEY` | The bearer token for Jev. |
| `JEVSCRIPT_PROFILES` | A JSON file of model profiles, layered over the bundled ones. Token limits, request caps, tokenizer and prices all live there and nowhere else. |
| `JEVSCRIPT_PATH` | Colon-separated module search roots, for `use` paths that are not relative. |
| `JEVSCRIPT_BIN` | Which runtime binary the SDKs spawn. |

## Status

See [AGENTS.md](AGENTS.md) for the implemented surface and the
complete check commands. [Spec section 15](spec/jevscript-language-specification.md#15-conformance)
defines acceptance for compilation, request batching and state, exact replay,
the SDK boundary, machine menus, and model profiles. The
[conformance evidence](docs/conformance.md) maps each item to its tests and
distinguishes scripted examples from live provider checks.
