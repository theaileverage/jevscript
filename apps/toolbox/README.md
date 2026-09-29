# jevs toolbox

A local web app for trying Jevscrypt ideas: describe an idea and get a checked
program, run it, tune it, watch a machine step through its states, and bind its
capabilities. It drives the real runtime (`jevscrypt serve` through `sdk/js`)
and never shows a program that has not been compiled.

## Run it

The toolbox needs the runtime binary, the JavaScript SDK and the Claude Code
adapter built first:

```sh
cargo build                                        # target/debug/jevscrypt
(cd sdk/js && pnpm install && pnpm run build)
(cd adapters/claude-code && pnpm install && pnpm run build)
cd apps/toolbox && pnpm install && pnpm dev        # http://127.0.0.1:5178
```

`pnpm dev` starts one Node process that serves the page through Vite and bridges
to it over WebSocket. `pnpm build && pnpm start` serves the built page instead.
Node 26 or later runs the server's TypeScript directly.

## Screens

- **Chat.** Describe an idea; Claude drafts a program with the spec as its
  system prompt. Every draft is compiled with `jevscrypt compile`; errors go
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

Ideas, their chat, pins, resend history and every recording live under
`~/.jevs-toolbox`, never in the repository. Each run records to a new file, and
Replay uses only that file.

## Environment

| Variable | Default | Used for |
| --- | --- | --- |
| `TYPESAFE_API_KEY` | from `.env` | Jev calls in runs, `/judge` and resend |
| `ANTHROPIC_API_KEY` | from `.env` | Chat drafting and the annotator; without it both say so, and pasted programs still work |
| `JEVS_TOOLBOX_MODEL` | `claude-opus-5-5` | The Claude model for drafting and annotating |
| `JEVSCRYPT_PROFILES` | none | A profiles overlay, shown in the profile switcher and used by runs |
| `JEVSCRYPT_BIN` | `<repo>/target/debug/jevscrypt` | The runtime binary |
| `JEVSCRYPT_PATH` | none | Module roots for `use` (spec section 3.9) |
| `JEVS_LSP_COMMAND` | `~/.treehouse/jevscript-4c52f4/1/jevscript/target/debug/jevscript lsp` | The language server, started per page connection |
| `JEVS_TOOLBOX_HOME` | `~/.jevs-toolbox` | Ideas and recordings |
| `JEVS_TOOLBOX_PORT` | `5178` | The server port |

Keys are read at start-up from the checkout's `.env` and, in a git worktree,
from the main checkout's `.env`. Variables already set win. Nothing is copied.

## Real and stubbed

Real: compile, check and `check --tools` (the CLI); runs, pauses, resume, abort
and recording (the SDK over `jevscrypt serve`); replay (`jevscrypt replay`, with
no key and no adapters); profiles (the bundle plus the overlay); the language
server; the JSONL subprocess and Claude Code adapters; resend.

Stubbed: the Stub binding is a demonstration adapter. It follows the CLI's
`--stub` except that typed tool verbs return a value of their declared type;
see `SPEC-GAPS.md` item 1.

Not verified end to end in this build: live Claude drafting and live annotator
edits (no Anthropic key was available; both are tested against a scripted
model), and a live Claude Code pane (the adapter has its own tmux tests).

## Tests

```sh
pnpm test        # vitest: server bridge, IR→graph, dials, pause stack, annotator, resend, LSP bridge, e2e
pnpm run typecheck
pnpm build
```

The end-to-end test runs `examples/review_loop.jev` under stub bindings through
the server against a local stand-in for the TypeSafe endpoint, checks the graph
and the recorded steps, then replays the recording and checks that no request
reached the endpoint. No test needs the `jevscript` repository or any key.

`SPEC-GAPS.md` lists where the toolbox had to work around the spec or runtime.
