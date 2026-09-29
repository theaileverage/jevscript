# jevs toolbox

A local web app for trying Jevscript ideas: describe an idea and get a checked
program, run it, tune it, watch a machine step through its states, and bind its
capabilities. It drives the real runtime (`jevscript serve` through `sdk/js`)
and never shows a program that has not been compiled.

## Run it

The toolbox needs the runtime binary, the JavaScript SDK and the agent adapter
suite (for the Claude Code adapter) built first:

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

The demo never reads a real provider key. It loads no `.env` file, and it
replaces `TYPESAFE_API_KEY`, `JEVSCRIPT_PROFILES` and the `ANTHROPIC_*`
variables in its own environment with the stand-ins' values before anything
starts. The one thing on its screens that can reach outside is binding an agent
to Claude Code on the Adapters screen, which starts your real `claude` in tmux.

Its state is kept apart from a real toolbox home: `~/.jevs-toolbox-demo`
(`JEVS_TOOLBOX_HOME` and `JEVS_TOOLBOX_PORT` override the home and port). It
survives a restart. Ctrl-C stops the demo and its stand-ins; `pnpm demo --
--reset` starts it clean, and removing `~/.jevs-toolbox-demo` forgets it
entirely.

## Screens

- **Chat.** Describe an idea; Claude drafts a program with the spec as its
  system prompt. Every draft is compiled with `jevscript compile`; errors go
  back to Claude for up to two repairs, and a draft that still fails is shown
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
  the stub, a JSONL subprocess (spec section 11.6) or Claude Code in tmux. Check
  manifests runs the section 9.4 comparison. An agent bound to Claude Code shows
  a live tail of its pane, labelled as agent-written and never acted on.

## Where state lives

Everything lives under the toolbox home (`~/.jevs-toolbox`), never in the
repository:

- `toolbox.sqlite` holds the toolbox's own state: ideas with their chat,
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

## Environment

| Variable | Default | Used for |
| --- | --- | --- |
| `TYPESAFE_API_KEY` | from `.env` | Jev calls in runs, `/judge` and resend |
| `ANTHROPIC_API_KEY` | from `.env` | Chat drafting and the annotator; without it both say so, and pasted programs still work |
| `JEVS_TOOLBOX_MODEL` | `claude-opus-5-5` | The Claude model for drafting and annotating |
| `JEVSCRIPT_PROFILES` | none | A profiles overlay, shown in the profile switcher and used by runs |
| `JEVSCRIPT_BIN` | `<repo>/target/debug/jevscript` | The runtime binary |
| `JEVSCRIPT_PATH` | none | Module roots for `use` (spec section 3.9) |
| `JEVS_LSP_COMMAND` | `<JEVSCRIPT_BIN> lsp` | The language server, started per page connection |
| `JEVS_TOOLBOX_HOME` | `~/.jevs-toolbox` (`~/.jevs-toolbox-demo` for `pnpm demo`) | The database and recordings |
| `JEVS_TOOLBOX_PORT` | `5178` (`5188` for `pnpm demo`) | The server port |

Keys are read at start-up from the checkout's `.env` and, in a git worktree,
from the main checkout's `.env`. Variables already set win. Nothing is copied.

## Real and stubbed

Real: compile, check and `check --tools` (the CLI); runs, pauses, resume, abort
and recording (the SDK over `jevscript serve`); replay (`jevscript replay`, with
no key and no adapters); profiles (the bundle plus the overlay); the language
server; the JSONL subprocess and Claude Code adapters; resend.

Stubbed: the Stub binding is a demonstration adapter. It follows the CLI's
`--stub` except that typed tool verbs return a value of their declared type;
see `SPEC-GAPS.md` item 1.

Not verified end to end in this build: live Claude drafting and live annotator
edits, and live Jev answers. The tests run the real Anthropic SDK client and the
real runtime against local stand-ins for both services (`demo/services.ts`),
which prove the toolbox's side of each exchange but nothing about the live
services. A live Claude Code pane is not driven either; the adapter has its own
tmux tests, and the toolbox's pane tail is checked against a real tmux pane.

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
