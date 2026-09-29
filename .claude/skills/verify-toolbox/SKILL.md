---
name: verify-toolbox
description: Prove the jevs toolbox (apps/toolbox) works the way a developer uses it — its server's /ws and /lsp boundaries against the real jevscript CLI, serve and lsp, and all four screens (Chat, Playground, Machines, Adapters) in Chrome against fixture TypeSafe and Anthropic endpoints. Use after touching apps/toolbox, the SDK or adapter surfaces it links, the CLI output it parses, or the IR it reads.
---

# Verify the jevs toolbox

The toolbox is a local web app over the real runtime: `pnpm start` serves the
page and a WebSocket API, and behind it sit `jevscript compile`, `check
--tools`, `replay`, `jevscript serve` (through `sdk/js`) and `jevscript lsp`.
The only things replaced are the two paid services, TypeSafe and Anthropic,
which `apps/toolbox/test/fixtures.ts` stands in for locally.

Nothing here calls a live service. The drive script unsets both keys, and the
fixtures set their own, so a key in `.env` is never read by a test.

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

For a manual pass, start the fixtures and the production server:

```sh
.claude/skills/verify-toolbox/scripts/launch.sh [evidence-dir] [port]
```

Ready when it prints `jevs toolbox on http://127.0.0.1:<port>` with
`chat drafting: claude-opus-5-5` and `Jev: TYPESAFE_API_KEY set`. Both lines
refer to the fixtures. The fixture Claude drafts the inbox triage program for
any request, and at a pin it proposes the stuck-to-waiting edit when asked to
"ask me instead". Open the URL with chrome-devtools-axi.

## Drive

```sh
.claude/skills/verify-toolbox/scripts/drive.sh [evidence-dir]
```

It runs, each into `<evidence-dir>/<step>.log`:

- **typecheck.** Both tsconfigs.
- **boundary.** `pnpm test`. Chat drafting with repairs, the annotator, check
  and `check --tools`, profiles, `/judge`, runs with a JSONL subprocess adapter
  (retry, a dead adapter, a manifest refusal), run lifecycle, resend parity
  with the runtime's own request bodies and error messages, `jevscript lsp`
  through `/lsp`, a real tmux pane tail, and `review_loop.jev` end to end with
  a zero-call replay.
- **browser.** `pnpm test:browser`. It builds the page, starts
  `node server/main.ts` as `pnpm start` does, and drives Chat (draft, run,
  `/judge`, `/check`), the pause stack (retry, budget, confirm options, two
  runs stacked), Playground (LSP colours and diagnostics, dials writing to
  source, run, Requests, replay with no live call), Machines (graph, steps,
  pin, apply and revert) and Adapters (binding, the manifest check).

It exits non-zero if any step failed.

## Evidence

The evidence directory defaults to `${TMPDIR:-/tmp}/jevs-toolbox-verify/<epoch>`.
It holds each step log, `summary.txt` (step and exit code), `commit.txt` and the
browser pass's screenshots. Label what it proves honestly:

- Everything here is **fixture integration** or **local E2E** against fixture
  endpoints. It proves the toolbox's side of each exchange, not the live
  services.
- A live Claude draft, live Jev answers and a live Claude Code pane are not
  covered. Report them as not verified unless you ran them with real keys and
  said so.
- The tmux check is skipped without tmux, and a skipped step is not a pass.

## Cleanup

```sh
.claude/skills/verify-toolbox/scripts/cleanup.sh <evidence-dir>
```

Stops the server and fixtures that `launch.sh` started, and keeps the evidence.
Test runs clean up after themselves: the toolbox home, recordings and
screenshots live in temporary folders, and the tmux check uses a private
`TMUX_TMPDIR` server that it kills.
