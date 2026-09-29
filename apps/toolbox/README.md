# jevs toolbox

A local web app for trying Jevscript ideas: describe an idea and get a checked
program, run it, tune it, watch a machine step through its states, and bind its
capabilities. It drives the real runtime (`jevscript serve` through `sdk/js`)
and never shows a program that has not been compiled.

## Run it

The toolbox needs the runtime binary, the JavaScript SDK and the agent adapter
suite (Claude Code and Codex) built first:

```sh
cargo build                                        # target/debug/jevscript
(cd sdk/js && pnpm install && pnpm run build)
(cd adapters && pnpm install && pnpm run build)
cd apps/toolbox && pnpm install && pnpm dev        # http://127.0.0.1:5178
```

`pnpm dev` starts one Node process that serves the page through Vite and bridges
to it over WebSocket. `pnpm build && pnpm start` serves the built page instead.
Node 26 or later runs the server's TypeScript directly.

## Try it without keys

```sh
cd apps/toolbox && pnpm demo                       # http://127.0.0.1:5188
```

`pnpm demo` builds the page and serves it on 127.0.0.1 only, with the TypeSafe
and Anthropic endpoints replaced by local stand-ins (`demo/services.ts`). The
stand-in Claude drafts an inbox triage program for any request and, at a pin,
answers a question or proposes an edit (ask it to "ask me instead" on the
`stuck` edge); the stand-in Jev prefers `finished`, `approved` and `week`. Every
screen works: Chat, Playground, Machines and Adapters.

New Idea offers one **Harness** model list by the composer, with Claude Code and
Codex choices discovered from the installed CLIs. Select a model, then describe
your idea. These choices invoke your
signed-in local CLI and return a real response. The demo labels Jev and machine
annotation as fixtures on every screen. A selected local agent never falls back
to a fixture.

The demo loads no `.env` file, and it
replaces `TYPESAFE_API_KEY`, `JEVSCRIPT_PROFILES` and the `ANTHROPIC_*`
variables in its own environment with the stand-ins' values before anything
starts. Chat can reach Claude or OpenAI through CLI sign-in. Binding an agent
to Claude Code or Codex on the Adapters screen starts the corresponding real CLI in tmux.

Install and sign in to the CLIs you want to use. Run `claude auth login` or
`codex login`, then click **Refresh agents**. No Anthropic or OpenAI API key is
required for CLI Chat. The selector only offers the CLI's discovered model menu;
provider access or usage-limit failures appear as errors saved with the idea.
Claude Code aliases show their resolved model on a successful reply when the CLI
reports it. Codex uses the concrete model ID returned by `model/list`.

Its state is kept apart from a real toolbox home: `~/.jevs-toolbox-demo`
(`JEVS_TOOLBOX_HOME` and `JEVS_TOOLBOX_PORT` override the home and port). It
survives a restart. Ctrl-C stops the demo and its stand-ins; `pnpm demo --
--reset` starts it clean, and removing `~/.jevs-toolbox-demo` forgets it
entirely.

## Screens

- **Chat.** Choose Claude Code or Codex and a model, then describe an idea. The CLI drafts a program with the spec as its
  system prompt. Every draft is compiled with `jevscript compile`; errors go
  back to the selected CLI for up to two repairs, and a draft that still fails is shown
  with its diagnostics and is not adopted. A pasted program (fenced or bare) is
  checked without any model. Commands: `/run`, `/replay`, `/check`,
  `/judge <judgment> {"param": "value"}`. The right panel shows the inputs,
  Jev's latest answers (amber below the asking unit's `min_confidence`), what
  `person.notify` said, earlier results, and the pause stack.
- **Playground.** An editor driven by the `jevscript` language server: semantic
  colours, diagnostics with a hover card (code, message, cause from
  `docs/error-reference.md`, spec section), hover and completion. Console tabs:
  Output, Requests (inspect, edit and resend any recorded request), Trail and
  Diagnostics. The tuning panel switches profiles, moves the budgets and
  thresholds the program declares (rewriting the source), and toggles `sample`.
- **Machines.** The graph comes from the compiled IR: guarded edges evergreen,
  Jev's picks grey, risky edges amber dashed, terminal states double-bordered,
  the current state filled. The timeline scrubs the newest run or replay; the
  inspector shows Jev's menu, the gate and what Jev saw, all from recording
  events. Click a state or edge to pin a question or a change request; Claude
  answers, or returns an edit that the toolbox checks (errors, new warnings,
  reachability changes) before you apply it. Pins are saved with the idea.
- **Adapters.** Each `needs` with its binding, call counts and status; bind to
  the stub, a JSONL subprocess (spec section 11.6), Claude Code or Codex in tmux. Check
  manifests runs the section 9.4 comparison. An agent bound to either CLI shows
  a live tail of its pane, labelled as agent-written and never acted on.

## Idea files and host execution

Each idea owns a workspace of Jev programs and host files. Open a file from the
Chat panel’s **Idea files** list to edit it in Playground. Select a file tab or
add a relative file such as `lib/helper.jev`.
Selecting a Jev file makes it the entry program for checks and runs; its sibling
Jev files resolve `use` imports. Select text in either kind of source and use
**Annotate selection** to ask the current Harness model. The selected text and
reply remain with the file, including a notice when its source changes later.
Machine pins also remain attached to their original Jev file.

Every idea row has an actions menu. **Update details** changes its title,
description, Harness model and Jev profile while preserving files, annotations,
conversation and runs. **Delete idea…** asks for confirmation and atomically removes
only that idea’s database records, including resend history. Its recording files
stay on disk. End an active run before deleting its idea.

New agent drafts include a complete TypeScript SDK host by default; an explicit
Python request produces a Python SDK host. The editor highlights TypeScript,
JavaScript, Python and JSON. Edits, active file and selection annotations are
saved with the idea. Changing the program declaration in Chat preserves an
existing entry filename; the companion is checked against that exact saved name.

The complete file contract is:

| Language | Saved files |
| --- | --- |
| TypeScript | entry `.jev`, `host.ts`, `runtime.ts`, `runtime.json` |
| Python | entry `.jev`, `host.py`, `toolbox_runtime.py`, `runtime.json` |

All application source, including support files, is visible and editable. The
main host uses the real SDK's `load`, `task().start`, pause iteration, resume,
result/error handling and cleanup. The context file supplies bindings, saved
inputs, profile, sample and recording configuration. Outside the toolbox it reads
`runtime.json` and launches the normal SDK runtime and configured JSONL adapters;
inside it receives configuration and private stdio to the toolbox-owned runtime.
A fixture binding is never silently treated as a real standalone adapter. Set a
real `command` and `args` in `runtime.json` for any capability that needs one.

With the SDK and required adapters installed, copy the whole saved file set into
one folder and run `node host.ts` (Node 26+) or `python3 host.py`. Select a new
recording path in `runtime.json` before each standalone run; the runtime refuses
to overwrite recordings. `JEVSCRIPT_BIN` can select a local built CLI. Standalone
hosts run under the invoking user's ordinary permissions.

On a main host tab, **Check pair** compiles the selected Jev entry and syntax-checks
the host without executing it. **Run pair** explicitly executes that saved host
through its actual SDK. Bind capabilities on Adapters and set Inputs as for a
direct Jev run. Pauses appear in the existing page stack, and results and JSONL
recordings remain with the idea. Support files are editable but are not Run pair
entry points. Existing `runIdea()` hosts remain supported for earlier ideas.

Toolbox Run permits host filesystem access only in its disposable workspace and
blocks network and child-process execution. TypeScript uses Node's permission
mode; Python uses macOS Seatbelt or Linux bubblewrap with a seccomp policy (`bwrap` and `libseccomp2` must be installed).
Python discovery selects the actual installed interpreter, including framework
interpreters on macOS; it does not install another Python or launch unsandboxed.
External effects run through configured adapters in the toolbox process. Child
hosts receive no provider credentials. Stdout/stderr are bounded and discarded;
safe host errors appear in the page. Startup is bounded to five seconds, execution
to ten minutes, source/output to 1 MiB and JSONL control buffering to 2 MiB.
Ordinary Jev runtime recording and replay formats are preserved.

## Where state lives

Everything lives under the toolbox home (`~/.jevs-toolbox`), never in the
repository:

- `toolbox.sqlite` holds the toolbox's own state: ideas with their chat, files, annotations,
  bindings and inputs, pins, the index of runs per idea, and resend history.
  The server prints its path at start-up and reports it in `status`.
- `recordings/` holds one JSONL recording per run, as the runtime writes it
  (spec section 10.3). The database only points at them, and Replay uses only
  the file.

Builds before the database kept the same state as JSON files under
`ideas/`. The first start with a database imports them in one transaction,
records that it did, and leaves the files in place as a backup; it never
imports them again. A database written by a newer toolbox is refused rather
than read.

SQLite upgrades add the model choice and workspace without replacing earlier
source, chat, pins or run rows. An earlier single-file idea becomes its first Jev
file plus the default host; its pins remain attached to that Jev file.

## Environment

| Variable | Default | Used for |
| --- | --- | --- |
| `TYPESAFE_API_KEY` | from `.env` | Jev calls in runs, `/judge` and resend |
| `ANTHROPIC_API_KEY` | from `.env` | The annotator and legacy API Chat requests; local CLI Chat does not use it |
| `JEVS_TOOLBOX_MODEL` | `claude-opus-5-5` | The API model for the annotator and legacy API Chat requests |
| `JEVS_TOOLBOX_CLAUDE_BIN` | `claude` on PATH | Local Claude Code executable; a path, never a shell command |
| `JEVS_TOOLBOX_CODEX_BIN` | `codex` on PATH | Local Codex executable; a path, never a shell command |
| `JEVSCRIPT_PROFILES` | none | A profiles overlay, shown in the profile switcher and used by runs |
| `JEVSCRIPT_BIN` | `<repo>/target/debug/jevscript` | The runtime binary |
| `JEVSCRIPT_PATH` | none | Module roots for `use` (spec section 3.9) |
| `JEVS_LSP_COMMAND` | `<JEVSCRIPT_BIN> lsp` | The language server, started per page connection |
| `JEVS_TOOLBOX_HOME` | `~/.jevs-toolbox` (`~/.jevs-toolbox-demo` for `pnpm demo`) | The database and recordings |
| `JEVS_TOOLBOX_PORT` | `5178` (`5188` for `pnpm demo`) | The server port |
| `JEVS_TOOLBOX_ALLOWED_ORIGINS` | none | Comma-separated additional exact local page origins allowed to connect to `/ws` and `/lsp`, for example `http://localhost:5179`; the toolbox page's own origin is always allowed |

WebSocket requests must send an `Origin` matching the toolbox page's scheme,
host and port, or an explicitly listed origin. Only `http` or `https` origins
on `localhost`, `127.0.0.1` or `[::1]` with an explicit port may be listed.

Keys are read at start-up from the checkout's `.env` and, in a git worktree,
from the main checkout's `.env`. Variables already set win. Nothing is copied.

## Real and stubbed

Real: compile, check and `check --tools` (the CLI); runs, pauses, resume, abort
and recording (the SDK over `jevscript serve`); replay (`jevscript replay`, with
no key and no adapters); profiles (the bundle plus the overlay); the language
server; the JSONL subprocess, Claude Code and Codex adapters; resend.

Stubbed: the Stub binding is a demonstration adapter. It follows the CLI's
`--stub` except that typed tool verbs return a value of their declared type;
see `SPEC-GAPS.md` item 1.

CLI Chat starts in an empty temporary directory under `<home>/agent-work`, sends
the spec and conversation through stdin, and removes the directory afterward.
Claude Code uses print mode with tools, customizations, MCP and session persistence
disabled. Codex uses ephemeral exec with a read-only sandbox, no approvals, no user
config or exec rules, and shell execution, code execution, hooks, plugins, apps,
browser tools and delegation disabled. Each response has a 90-second deadline,
512 KiB input limit and 2 MiB combined output limit. Compiler repairs can make at
most two additional responses. Failed requests save a safe error and never expose
raw CLI stderr, credential values or full environment data.

Live CLI Chat was verified with Claude Code 2.1.284 using `claude-opus-5-5` and
Codex CLI 0.159.0 using `gpt-6.1-sol`. Both generated greeting programs through
`/ws` that compiled on the first attempt. See [CLI Chat verification](verification/harness-chat.md).

Live machine annotator edits and live Jev answers remain unverified. Source selection
annotations on both Jev and host files reached the selected Codex CLI in the local browser trial. The tests run the real Anthropic SDK client and the
real runtime against local stand-ins for both services (`demo/services.ts`),
which prove the toolbox's side of each exchange but nothing about the live
services. The CLI tests use fake executables and do not prove live service access.
The Codex tmux adapter was also driven live through all five agent verbs with
read-only sandboxing and approvals set to never. Claude Code runtime panes were
not driven live; existing adapter integration checks use fixtures and real tmux.

## Tests

```sh
pnpm test            # boundary suite: /ws, /lsp, the CLI, jevscript serve, the database and a real tmux pane
pnpm test:browser    # builds the page; drives all four screens in Chrome, restarts the server, and runs `pnpm demo`
pnpm run typecheck
pnpm build
```

Every test enters the toolbox the way the page does: through the server's `/ws`
and `/lsp` WebSockets, or through the page itself in a browser. Behind the
server everything is real (`jevscript compile`, `check --tools`, `replay`,
`jevscript serve` through the SDK, `jevscript lsp`), except the TypeSafe and
Anthropic endpoints, which are local fixtures. The browser pass uses Chrome
(`channel: 'chrome'`); set `JEVS_BROWSER` to another Chrome or Chromium binary.
Screenshots land in `JEVS_EVIDENCE` when it is set, else in the run's temporary folder.

`SPEC-GAPS.md` lists where the toolbox had to work around the spec or runtime,
and what was checked when it moved from the older Jevscrypt repository.
