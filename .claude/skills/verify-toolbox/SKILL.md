---
name: verify-toolbox
description: Prove the jevs toolbox (apps/toolbox) works the way a developer uses it — its server's /ws and /lsp boundaries against the real jevscript CLI, serve and lsp, its SQLite state across restarts, `pnpm demo`, and all four screens (Chat, Playground, Machines, Adapters) in Chrome against fixture TypeSafe and Anthropic endpoints. Use after touching apps/toolbox, the SDK or adapter surfaces it links, the CLI output it parses, or the IR it reads.
---

# Verify the jevs toolbox

The toolbox is a local web app over the real runtime: `pnpm start` serves the
page and a WebSocket API, and behind it sit `jevscript compile`, `check
--tools`, `replay`, `jevscript serve` (through `sdk/js`) and `jevscript lsp`.
The only things replaced are the two paid services, TypeSafe and Anthropic,
which `apps/toolbox/demo/services.ts` stands in for locally. The toolbox's own
state is in `<home>/toolbox.sqlite`; recordings are JSONL files beside it.

The deterministic drive suite uses fake Claude Code and Codex executables as well
as the two fixture API endpoints. It calls no live model. Manual Launch enables
real Chat through signed-in local CLIs. No provider API key is needed for that path.

## Doctor

Read-only; run it first and whenever a step fails for an environmental reason:

```sh
.claude/skills/verify-toolbox/scripts/doctor.sh
```

It checks Node 26 or later (the server runs its TypeScript directly), pnpm,
`target/debug/jevscript`, the built SDK and adapters, the toolbox's
dependencies, Chrome (or `JEVS_BROWSER`) and tmux. Each missing item prints the
command that fixes it.

## Launch

For a manual pass, start the fixture demo (`pnpm demo`) with its state in the
evidence directory:

```sh
.claude/skills/verify-toolbox/scripts/launch.sh [evidence-dir] [port]
```

Ready when it prints `jevs toolbox demo: http://127.0.0.1:<port>` with the
database path and fixture endpoints. The demo reads no `.env` and replaces
provider API variables with the fixtures' values. Chat offers real Claude Code
and Codex responses using CLI sign-in. Jev and machine annotation remain visibly labeled
fixtures; source selection annotations use the selected local CLI. At a pin the fixture proposes the stuck-to-waiting edit when asked to
"ask me instead". Open the URL with
chrome-devtools-axi.

For live acceptance, create a New Idea with Claude Code and a discovered model;
ask for a greeting without capability calls and use the default TypeScript host.
Repeat with Codex and an explicit Python SDK request. Confirm truthful CLI/model
provenance and compilation. Inspect the complete saved file sets: entry `.jev`,
`host.ts`/`host.py`, visible `runtime.ts`/`toolbox_runtime.py`, and `runtime.json`.
All application source must be editable and highlighted. Edit both main hosts,
switch files and annotate selected source through the chosen CLI. Check and Run
both pairs, confirm outputs and recordings, reload, then restart the same demo
home and verify full source, active file, annotations, model and conversation.

Export every saved file unchanged to fresh folders outside the staged runner.
Use the actual installed SDKs and local `JEVSCRIPT_BIN` to run `node host.ts` and
`python3 host.py`, without the embedded marker or injected config. Check that
host load targets exist among those exact saved filenames, including after a
chat update changes a program declaration. Use new recording paths for repeats.
Do not count a staged alias or hidden helper as standalone portability.

For Codex runtime acceptance, select its built-in adapter choice and inspect the
installed adapter CLI's discovery. Exercise spawn, send, wait, observe and stop
through the supported adapter with a private tmux session, read-only sandbox and
approval policy never. Stop only that run-owned agent/session. Report signed-out,
missing CLI, unsupported OS sandbox or provider failures honestly; no fixture
fallback. Linux Python requires bubblewrap and libseccomp2 and must be reported separately from
a macOS-only local pass.

## Drive

```sh
.claude/skills/verify-toolbox/scripts/drive.sh [evidence-dir]
```

It runs, each into `<evidence-dir>/<step>.log`:

- **typecheck.** Both tsconfigs.
- **boundary.** `pnpm test`. Chat drafting with repairs, the annotator, check
  and local CLI discovery/selection, saved errors, signed-out behavior, input and
  output limits, timeout termination, CLI conversation restart and SQLite upgrade;
  idea-owned Jev/host source and selection annotations across restart, linked-file
  compilation, side-effect-free pair checks, explicit host task/inputs, permission
  refusal outside its workspace, safe errors, actual TypeScript/Python SDK hosts,
  complete exported file sets after declaration changes, page-owned pauses and recording replay;
  and `check --tools`, profiles, `/judge`, runs with a JSONL subprocess adapter
  (retry, a dead adapter, a manifest refusal), run lifecycle, resend parity
  with the runtime's own request bodies and error messages, `jevscript lsp`
  through `/lsp`, a real tmux pane tail, and `review_loop.jev` end to end with
  a zero-call replay; the database keeping ideas, pins, the run index and
  resend history across a restart, the one-time import of an earlier build's
  JSON files, and the refusal of a newer schema.
- **browser.** `pnpm test:browser`. It builds the page, starts
  `node server/main.ts` as `pnpm start` does, and drives Chat (draft, run,
  `/judge`, `/check`), the pause stack (retry, budget, confirm options, two
  runs stacked), Playground (LSP colours and diagnostics, dials writing to
  source, run, Requests, replay with no live call), Machines (graph, steps,
  pin, apply and revert) and Adapters (binding, the manifest check), then
  restarts the server and finds the ideas, pin, binding and steps again. It
  also checks the New Idea Harness model list and conversation chip, reply provenance, selection after reload,
  saved errors and unavailable states with fake executables; edits and annotations
  on Jev and host file tabs, checks without executing, explicit Run pair, error
  visibility and workspace/annotation restoration. Update details must preserve
  source, annotations, conversation and runs; canceled deletion keeps the idea,
  confirmed deletion stays removed after restart, recordings remain, other ideas
  keep their IDs, and late saves cannot recreate the deleted idea. It runs `pnpm demo`
  as a process with real-looking keys and a dead Anthropic URL in its environment,
  selects a fake CLI and runs its checked draft, stops it with
  Ctrl-C, restarts it on the same home and finds the state again.

It exits non-zero if any step failed.

## Evidence

The evidence directory defaults to `${TMPDIR:-/tmp}/jevs-toolbox-verify/<epoch>`.
It holds each step log, `summary.txt` (step and exit code), `commit.txt` and the
browser pass's screenshots. Label what it proves honestly:

- Everything here is **fixture integration** or **local E2E** against fixture
  endpoints. It proves the toolbox's side of each exchange, not the live
  services.
- Live CLI Chat is a separate manual check. The fixture suite never proves it.
  Live machine API annotation and Jev answers are not covered. Runtime agent
  panes and standalone SDKs require the separate checks above. Report only
  what was actually exercised.
- The tmux check is skipped without tmux, and a skipped step is not a pass.

## Cleanup

```sh
.claude/skills/verify-toolbox/scripts/cleanup.sh <evidence-dir>
```

Stops the demo that `launch.sh` started (it closes its fixtures), and keeps the
evidence. Test runs clean up after themselves: the toolbox home, recordings and
screenshots live in temporary folders, and the tmux check uses a private
`TMUX_TMPDIR` server that it kills.
