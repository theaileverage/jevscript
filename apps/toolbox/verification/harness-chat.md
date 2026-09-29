# CLI Chat and workspace verification

The task branch includes the stable local migration head
`2b168f097c6e76f00a2893592389271a87044ce6`, including its configured-Origin
allowlist and accepted review fixes. The follow-up authorizes a ready review
through direct feature-branch delivery; the maintainer owns merging. No no-mistakes
pipeline is used for this delivery.

## Live browser evidence

On 2026-09-29, Chrome at 1440×900 drove the combined demo on
`http://127.0.0.1:5394`. These responses reached signed-in CLIs, used no
Anthropic or OpenAI API key, and supplied complete greeting programs that the
real compiler accepted with zero diagnostics on the first attempt.

| CLI | Version | Selected model | Reported response model | Saved reply UTC |
| --- | --- | --- | --- | --- |
| Claude Code | 2.1.284 | `default` | `claude-opus-5-5` | 19:59:59 |
| Codex | 0.159.0 | `gpt-6.1-sol` | `gpt-6.1-sol` | 19:52:20 |

The demo keeps these as **Live Claude greeting** and **Live Codex greeting**.
Updating their titles preserved the exact first prompt, saved response, files and
run history. The earlier garbled Claude trial was caused by the verification
operator's browser input sequence; it was replaced by a clean live invocation and
removed through the confirmation dialog. Browser automation also asserts that
its first prompt arrives as one unchanged message.

The Codex idea's visible `host.ts` passed Check pair before an explicit Run pair
produced `greeting = Hello, Captain!`, zero model calls and a saved JSONL recording.
Codex also answered selected text from both its Jev and host files. These are live
CLI annotations, saved with their exact ranges and original text. The host bridge
uses the real SDK/runtime; this greeting uses no Jev provider. Jev judgments and
machine annotation in this demo still use clearly labelled fixture services.

Earlier `/ws` live probes at 18:53 UTC independently reached both CLIs. Discovery
advertised 12 Claude choices and eight visible Codex choices. Claude's default
alias and fixed Opus choice are distinct choices, so the UI labels the default
explicitly rather than showing two identical model rows. Discovery does not prove
that every advertised model will accept a later request.

## Complete SDK hosts and editor acceptance

On 2026-09-29 during the follow-up, Chrome at 1440×900 drove fresh New Idea
requests on the retained 5394 demo. **SDK Portable TypeScript acceptance** used
Claude Code's discovered `default` choice, reported `claude-opus-5-5`, and
produced `portable_ts.jev` plus a complete TypeScript SDK host. **SDK Portable
Python acceptance** explicitly requested Python, selected Codex `gpt-6.1-sol`,
and produced `portable_py.jev` plus a complete Python SDK host. Both drafts
compiled without repairs, inputs, capability calls or Jev provider calls.

The saved file contract has no hidden application source dependency:

| Language | Idea-owned editable files | Actual SDK/runtime output |
| --- | --- | --- |
| TypeScript | `portable_ts.jev`, `host.ts`, `runtime.ts`, `runtime.json` | `Portable TypeScript works` |
| Python | `portable_py.jev`, `host.py`, `toolbox_runtime.py`, `runtime.json` | `Portable Python works` |

CodeMirror highlighted both main hosts and their support files. Browser edits
prepended a comment in each language; selection annotations reached the chosen
real CLI and retained exact selected text and truthful model provenance. Check
pair and Run pair executed each complete host through its actual SDK and the
Rust runtime. File switching, reload and demo restart preserved both full edited
sources, active files, annotations, conversation, models and recordings. A `/ws`
snapshot compared digests for all ten retained ideas before/after restart with
no difference; original user ideas were left intact.

Both complete saved file sets were exported unchanged to fresh folders and run
outside the staged runner, without embedded transport or injected configuration.
TypeScript used the built SDK installed into the clean local package tree;
Python used the actual checkout SDK installed into an isolated local virtual
environment. Each exited zero with the expected output, empty stderr and a new
recording. This is local SDK execution, not a published registry-package smoke.
Earlier Python export failed because a verification-operator change had renamed
its Jev entry; the saved pair was restored and both exports rerun successfully.
Chat updates now preserve existing workspace filenames and validate host load
references against the saved entry. Separate executable integration checks change
the declaration in both languages, repair the host reference, restart SQLite,
export the exact saved files and run/replay them with the actual SDK/CLI.

Codex is also a built-in runtime adapter choice. A clean install linked the
built Codex and Claude Code packages; public adapter discovery reported the
installed Codex CLI and tmux backend. The actual Codex adapter used a private tmux
session, discovered `gpt-6.1-sol`, read-only sandbox and approvals never. Live
spawn, wait, send, observe and stop returned `CODEX_RUNTIME_OK` followed by
`CODEX_RUNTIME_SECOND`. Claude Code runtime panes were not exercised live.

Python's macOS confinement was verified with the discovered framework interpreter
executed directly, rather than its secondary launcher. Linux bubblewrap is a
separate CI/platform check; it was not exercised on this Mac. Safe host errors
remain visible; generated host stdout/stderr do not reach UI or normal logs.

Evidence is retained under `apps/toolbox/.local/harness-chat/followup/` (ignored):
`typescript-host-final.png`, `python-host-final.png`, `typescript-support-final.png`,
`export-portable.log`, `export-live.log`, `restart-persistence.json`,
`codex-live.json`, `codex-live.log`, `clean-codex-discovery.json`, and `final/`.

## Static and fixture evidence

The project-local verification skill passed Doctor, both TypeScript checks, the
production build, 57 integration checks and 11 browser checks. The linked SDK
checks passed 19 JavaScript and 17 Python cases; SDK and adapter type checks
also passed. Every added
behavior check enters `/ws` or the browser; no isolated unit cases were added.
The browser pass uses a 1440×900 viewport.

The deterministic suite uses fake CLI executables and local TypeSafe/Anthropic
endpoints. It proves selection, nondefault models, CLI provenance, no fixture
fallback, safe errors, signed-out/missing/stale models, invocation limits,
conversation restart, earlier SQLite migration and pasted-program checking.
Workspace coverage exercises linked Jev files, edits and annotations on both file
kinds, checks without execution, selected host/task/inputs, permission refusal,
errors, pauses, recording/replay and restart. Browser CRUD checks prove preserved
files/history on update, cancel-delete, selected-only confirmed deletion, restart,
retained recordings and refusal of late saves. Integration also proves that a
late resend cannot restore deleted history. Existing LSP, adapters, Machines,
request inspection/resend, tuning, runtime pause and replay flows passed.

These fixture passes are supporting evidence, separate from the live browser
trials above. Live Jev answers, machine API edits and an actual Claude tmux agent
session were not exercised.

## Paper comparison

Reference: [jevs toolbox, Page 1](https://app.paper.design/file/01M3PXK0ZJA69W5EASN6JD68J0/p-1-0).
The independent Opus review identified M1–M7. The final browser pass corrected all
seven; live provenance remains beside the Jev reply label. Local screenshots and
logs are in `apps/toolbox/.local/harness-chat/evidence/` (ignored by Git).

| Checklist | Result and visible evidence |
| --- | --- |
| 1 | Pass: Chat columns 272 / 808 / 360; pale right panel. |
| 2 | Pass: exactly Chat, Playground, Machines, Adapters; needs badge appears in the person-pause case. |
| 3 | Pass: 32px title, uppercase eyebrow, right meta; long titles truncate and header stays one line. |
| 4 | Pass: New idea · no files yet, Untitled idea, selected top rail row. |
| 5 | Pass: top textarea and model list, 16px gap, 48px below header; no thread or bottom composer. |
| 6 | Pass: green border/focus ring, 14px radius, 16px textarea text, inside footer and 36px send button. Card stretches to the real catalog's height. |
| 7 | Pass: 250px list beside textarea. |
| 8 | Pass: Harness headers say live, selected row has green check; Default is distinguished from fixed Opus. |
| 9 | Pass: every choice comes from CLI discovery. |
| 10 | Pass: note separates Harness drafting from the Jev profile. |
| 11 | Pass: empty Idea files panel and dashed Jev/Host slots. |
| 12 | Pass: user bubble, Jev reply, subtle truthful CLI attribution, checked program and result card. |
| 13 | Pass: compact one-row composer; exact shorter placeholder fits (266px text inside a 294px Claude textarea), no clipped second line. |
| 14 | Pass: muted harness, bold model and small chevron in the pill; no signed-in caption below it. Native select remains underneath for keyboard accessibility. |
| 15 | Pass with fixture browser evidence: run status, inputs, Jev answer bars and confirm pause buttons. No claim of live Jev inference. |
| 16 | Pass: visible 24px actions slot and truncated rail titles. |
| 17 | Pass: Update details and red Delete idea… menu with trash icon. Pencil uses a text glyph. |
| 18 | Pass: centered Update dialog, title/description, side-by-side Harness and 160px Jev selects, preservation note, Cancel/Save. |
| 19 | Pass: named confirmation, owned-data explanation, recordings stay, irreversible notice, red Delete button and outlined target row. Body uses paragraphs. |

Screenshots: `review-fixed-new-idea.png`, `review-fixed-claude-chat.png`,
`review-fixed-codex-chat.png`, `review-fixed-update-dialog.png`,
`review-fixed-delete-dialog.png`, `live-jev-annotation-visible.png`,
`live-host-annotation.png`, and the `accepted/screenshots/` browser sequence.

The Idea Files interaction is functional but does not reproduce Paper's floating
annotation popover: source selection questions and saved notes appear below the
editor in its scroll area. Host source now uses a highlighted CodeMirror editor; Jev source
retains its LSP editor. This is a remaining visual difference, not a live-service claim.

## Local trial and restart

The running task demo uses its own retained SQLite home:

```text
http://127.0.0.1:5394
apps/toolbox/.local/harness-chat/home/toolbox.sqlite
```

Create an idea, select a discovered Harness model and send a request. Open a file
from the right Idea files panel to edit it in Playground. On `host.ts`, Check pair
checks syntax and Jev compilation; Run pair explicitly executes its visible
source through the actual SDK. Use the right Inputs JSON and configured bindings,
or edit the complete visible host. For standalone use, copy all four files and
set a new recording path in `runtime.json`. Select source,
scroll to its annotation controls and ask the selected model a question. The rail
menu updates details or confirms deletion.

Restart only this task demo, preserving its database:

```sh
.claude/skills/verify-toolbox/scripts/cleanup.sh apps/toolbox/.local/harness-chat
.claude/skills/verify-toolbox/scripts/launch.sh apps/toolbox/.local/harness-chat 5394
```

Verification commands:

```sh
.claude/skills/verify-toolbox/scripts/doctor.sh
.claude/skills/verify-toolbox/scripts/drive.sh apps/toolbox/.local/harness-chat/evidence
```

Unavailable CLI choices explain sign-in or executable failures; refresh after
`claude auth login` or `codex login`. Provider failures remain saved errors and
never produce a fixture fallback. Jev and machine annotation remain fixture
services; no live-provider inference or deployment is claimed.
