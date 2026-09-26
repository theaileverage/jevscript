# Jevscript Chief of Staff

A runnable Chief of Staff showcase. The policy and worker lifecycle are
composable Jevscript modules in [`jev/`](jev/); the Python package in
[`src/cos/`](src/cos/) supplies I/O, durable state, terminal backends and a
bounded watcher. This is a new harness, independent of
`examples/chief_of_staff.jev`.

## Start an agent session

The interactive agent is the conversational front door. It calls the same
`cos` commands as the CLI while a companion watcher runs Jevscript wakes.
Its instructions are scoped to this example in
[`agent-session/INSTRUCTIONS.md`](agent-session/INSTRUCTIONS.md); no project
`AGENTS.md` or `CLAUDE.md` is installed.

```sh
cd examples/chief-of-staff
uv sync
cargo build --manifest-path ../../Cargo.toml --bin jevscript
uv run cos --home ~/.cos init
uv run cos --home ~/.cos project add my-project /absolute/path/to/git/repo --mode local-only
uv run cos --home ~/.cos doctor
uv run cos --home ~/.cos session --agent claude  # or --agent codex
```

The default `crew` adapter is `python -m cos.terminal_agent`; it selects
Herdr when installed, then tmux, unless the current process is inside tmux or
cmux. Choose a backend in a dispatch profile (`cos profiles file.json`) or
set `COS_BACKEND=tmux` for the adapter process. A profile selects `harness`,
`model`, `effort`, `backend`, and an optional launch `command`. The built-in
adapter launches Claude Code or Codex by default; `command` may be an argv
list for another interactive agent CLI. It speaks the Jevscript JSONL adapter
protocol and implements spawn, observe, send, bounded wait and stop. Handles
contain the terminal endpoint and reattach by name after an adapter restart.
`COS_TMUX_SESSION` isolates its tmux windows; it defaults to `cos`.

The front door starts and stops only its own watcher. Use `--no-watch` when a
separate `cos watch` is already supervising the home. CLI access remains
available. The chat agent starts in the Chief of Staff home, outside any
registered project; it reaches project work only through the commands:

```sh
uv run cos --home ~/.cos say 'Fix the parser error in project X'
uv run cos --home ~/.cos say 'Also cover empty input' --project my-project --channel team --message-id msg-42 --reply-to msg-17
uv run cos --home ~/.cos bearings
uv run cos --home ~/.cos decisions
uv run cos --home ~/.cos answer '<decision-key>' yes
uv run cos --home ~/.cos steer '<task-id>' 'Add a regression test'
uv run cos --home ~/.cos playbooks list
```

`JEVSCRIPT_BIN` wins; otherwise the host uses this repository's
`target/release/jevscript` or `target/debug/jevscript`, then `PATH`. Live Jev runs fetch
the TypeSafe API key from the macOS keychain at run time, under service
`typesafe-api-key` by default (`jev.keychain_service` changes it). The key is
never written to a recording, ledger or adapter log. For an offline tour use
`uv run cos demo`; it runs a fake Jev server and JSONL agent process in a
scratch home, with no key.

`say` and `Host.submit` accept a message envelope: a stable `message_id`,
optional `reply_to` and `native_thread`, and project/channel scope. The CLI
uses the corresponding `--message-id`, `--reply-to`, `--native-thread`,
`--project` and `--channel` flags. `--after` remains a backlog dependency ID.
Omitting `message_id` gives a locally generated ID. Retrying a supplied ID
returns its recorded thread and task without creating another request.

[`threads.jev`](jev/threads.jev) first honors a valid native thread or reply
relation. A missing explicit relation is returned as unresolved. Otherwise it
uses a bounded same-scope candidate list and `pick among` with `none`; code
requires the configured `policy.thread_confidence` (default `0.65`) before
continuing. The host validates the scope, stores message-to-thread receipts
and updates the selected task or active worker inbox. A thread with no task
keeps its identity while the incoming message goes through normal intake. The
caller sees the thread ID and any task ID in the `say` result. This is local
message correlation; there is no provider connector or Discord API in this example.

## Task Skills at dispatch

Set `skill_catalog` in the CoS home config to an explicit list of approved
local Skills. Each entry has `id`, an absolute `path` to a real `SKILL.md`,
its lowercase SHA-256 `sha256`, and `dependencies` (other IDs in the same
catalog). A Skill with supporting files also declares `files`, a mapping of
every relative file path to its SHA-256 digest, including `SKILL.md`. The host
requires the manifest to cover the whole directory and rejects symlinks.
The frontmatter must contain a matching `name` and a `description`.
The host checks the file and digest when it builds a dispatch snapshot and
again before it installs anything. The catalog is limited to 64 Skills, and
each directory to 128 files and 8 MiB.
An empty catalog is valid and selects none. The historical cookbook roster is
not read by this dispatch path.

[`skills.jev`](jev/skills.jev) judges each approved Skill against the task
statement, original request and notes independently. It retains every fit
at or above `policy.skill_fit` (default `0.7`). A fit between
`policy.skill_uncertain` (default `0.35`) and that threshold stays uncertain;
the brief lists its ID and description and tells the worker to ask the owner
before work on that part of the task. Confident selections remain available.
The host deduplicates IDs, adds declared dependencies,
verifies pinned source bytes, and copies each complete Skill directory into the worker's
`.agents/skills/<id>/` directory. Claude workers also get a link under
`.claude/skills/<id>/`. The host writes a durable receipt and places the exact
selected IDs, worker paths, Skill digests, and tree digests in the brief before it asks the agent
adapter to spawn. The first durable selection is reused after an interrupted
dispatch. Receipt-owned files are checked and removed after landing, so a clean
worker branch stays landable. The brief tells the worker not to commit those
copies; if a worker stages them, the host blocks an open pull request from
merging and asks the worker to correct it. The gate checks the forge-reported
head commit's tree, so an unpushed local correction cannot clear it. A merged
pull request remains visible, and the owner is notified if that head contains
host Skill files or if cleanup keeps the isolated copy. A
per-item destination collision holds only that item with retry or dismiss;
a changed catalog source parks dispatch while other wakes continue. No model
answer supplies a file path, URL, or command.

## One wake at a time

Jevscript 0.1 requires `max` on loops and has a default 50-call run cap. A
recording cannot recover a process that crashed during an external effect,
and a post-restart replay is one-shot. The watcher therefore snapshots one
request or worker, runs one bounded Jevscript task, persists its `out result`
and recording, and acknowledges that wake. The next wake reads the resulting
state as its input. The [`episode`](src/cos/episode.py) runner parks owner
questions durably and resumes them from the recording after restart. The
[`effects`](src/cos/capabilities/effects.py) journal deduplicates completed
effects when a wake is retried; ambiguous crashes during an effect require
reconciliation with the terminal or worktree before another attempt.

The `on_wake` task dispatches to linked `threads`, `intake`, `routing`, `dispatch`,
`supervise`, `escalate`, `deliver` and `review` modules. `supervise.jev` owns
the per-worker lifecycle machine. `learn.jev` classifies repeated and wasteful
work; [`learning.py`](src/cos/learning.py) drafts a `.jev` playbook through
the writer capability, compiles it, replays relevant recordings and registers
it automatically only when the proof gate passes. The person can inspect,
disable or revert versions with `cos playbooks`.

`log` statements in `cos.jev` write progress to each run's recording. The
Python SDK's live log callback also indexes each line in `data/ledger.jsonl`
with its wake, subject and recording. Replayed lines are not indexed again.
The ledger additionally records episode usage and finished tasks for learning.

## Implemented capabilities

The status below means the behavior is implemented in this showcase and
covered by offline tests. Vendor CLI interactions require separate live checks.

| # | Capability | Chief of Staff implementation | Status |
| --- | --- | --- | --- |
| 1 | Session start and recovery | Home lock, `recover`, parked episode replay, agent-session watcher | Implemented |
| 2 | Project registry and delivery posture | `Registry`, local-only / direct-PR / no-mistakes, yolo policy | Implemented |
| 3 | Intake and routing | `threads.jev` continuation policy, `intake.jev` judgments, `routing.jev`, playbooks | Implemented |
| 4 | Backlog, dependencies, holds, done history | `Backlog`, Jevscript readiness rules and ledger | Implemented |
| 5 | Worker instructions | `dispatch.jev` contract plus writer-generated statement | Implemented |
| 6 | Isolated dispatch and runtime choice | `Fleet.worktree`, profiles, JSONL agent adapter, five terminal backends | Implemented; tmux agent path live-tested |
| 7 | Steering inbox with acknowledgement | `Inbox`, durable numbered messages and doorbells | Implemented |
| 8 | Wakes, stuck detection and recovery | `Watcher`, `supervise.jev` machine, bounded relaunch | Implemented |
| 9 | Validation, review, PR/merge watch and landing | `review.jev`, `deliver.jev`, `Delivery`, authority gate | Implemented |
| 10 | Human escalation and decisions | `escalate.jev`, `Decisions`, short outcomes | Implemented |
| 11 | Cleanup safety | Guarded `Fleet.cleanup`; unlanded work stays | Implemented |
| 12 | Away and quiet modes | Home mode, Jevscript escalation policy and return digest | Implemented |
| 13 | Preferences and learnings | `Memory`, task ledger, automatic versioned playbooks | Implemented |
| 14 | Scoped second mates | `Mates`, child home and scoped route | Implemented |
| 15 | Bearings fleet digest | `bearings.jev`, `Bearings` renderer | Implemented |

Public-mention relay, mail and visual boards are outside this showcase.
The TypeScript variant and TypeScript agent adapters are separate work.

## Layout and checks

`src/cos/state/` has independent registry, backlog, thread/message index, inbox, decisions, worker,
ledger and memory stores. `src/cos/backends/` has one module each for Herdr,
tmux, cmux, Orca and Zellij behind `Backend`. `src/cos/capabilities/` bridges
the runtime to external JSONL processes. `host.py` handles wake snapshots and
durable results; `commands.py` serves both front doors.

```sh
uv run --extra dev pytest -q
uv run cos smoke tmux
uv run cos smoke zellij
uv run cos smoke cmux
```

The live backend tests skip an unavailable tool. Herdr lifecycle checks need
`COS_SMOKE_HERDR=1`; Orca checks need an existing managed worktree path in
`COS_SMOKE_ORCA_WORKTREE`. Neither ran in the recorded
local evidence. The installed Zellij is 0.43, below the adapter's required 0.44
for detached pane targeting; cmux has no reachable control socket in this
session. The tmux JSONL test launches a small local Python agent,
reattaches to it, sends text, observes the result and closes its dedicated
test session. Live Jev classification still requires a keychain key and model
access; the offline suite uses recorded or scripted answers.
