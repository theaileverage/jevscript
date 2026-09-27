# jevscript (Python SDK)

Drives the `jevscript` runtime over JSON-RPC on stdio. The surface is section
11.2 of `spec/jevscript-language-specification.md`, with a synchronous iterator
of pauses.

Install with `pipx install jevscript` for a user-level `jevscript` command, or
`python -m pip install jevscript` inside a Python 3.10+ virtual environment.
The platform wheel contains the release-matched Rust CLI and needs no Cargo or
install-time download. Run `jevscript --version` and `jevscript check` on your
program; the installed package also contains `examples/inbox_triage.jev`.
Activate a virtual environment before expecting its `jevscript` command on
PATH. Offline installation works from the matching wheel with
`python -m pip install --no-index ./jevscript-0.1.2-*.whl`. Unsupported
platforms have no wheel and installation fails rather than installing an SDK
without a command. See the [package release guide](../../docs/package-release.md)
for supported platforms and OS floors. Explicit `bin=` and `JEVSCRIPT_BIN`
override the packaged binary; the default is verified before use.

Run `jevscript setup --agent codex` or `--agent claude-code` from a project
to install the bundled coding-agent Skill offline. Repeat `--agent` for both,
add `--global` for user scope, or `--copy` for independent directories.
Package installation does not run setup.

```python
from jevscript import load

program = load("examples/fix_issue.jev")
run = program.task("main").start(
    inputs={"issue": issue},
    bind={"claude": claude, "tree": tree, "me": person},
)
for pause in run:
    if pause["kind"] == "confirm":
        run.resume({"answer": "yes"})
```

Adapter callbacks receive the program-scoped capability identity as their last
argument. An adapter exception may carry `retryable = True`; the runtime then
surfaces a retryable `adapter_error` pause. `load(..., paths=[...])` supplies
module roots, while `start(..., profiles="profiles.json")` and
`judgment.run(..., profiles="profiles.json")` layer model profiles.

A program's `log` lines (spec section 5.8) reach the host as `LogEvent`
dictionaries: `level`, `message`, `fields`, `task`, `source`, and for a run
`run_id` and `seq`. Pass `on_log` to route them as they are written (it runs on
the SDK's reader thread), or iterate `run.logs()`:

```python
import logging

levels = {"debug": logging.DEBUG, "info": logging.INFO,
          "warn": logging.WARNING, "error": logging.ERROR}
run = program.task("main").start(
    inputs={"issue": issue},
    bind={"claude": claude, "tree": tree, "me": person},
    on_log=lambda line: logging.log(levels[line["level"]], line["message"], extra={"jev": line}),
)
answers = program.judgment("triage").run(state, on_log=print)
```

A replay checks the recorded lines without emitting them again, so `on_log`
sees only lines written live. `log_event(event)` reads a line off any recording
event.

When `redact=True` and `record` is set, the primary JSONL hides request state,
observations, agent-wait results and `log` values. The private full companion at
`<recording>.replay.jsonl` is sensitive and must be retained with the primary
for replay. `record` and `replay` are mutually exclusive, and recording paths
must be new; the runtime never appends to or overwrites either artifact.

Run the tests with the runtime binary built:

```sh
cargo build
python3 -m pytest sdk/python
```

## Subprocess agent adapters

Use the same agent executable as `jevscript run --bind` with
`subprocess_agent` (spec sections 9.1 and 11.6):

```python
from jevscript import load, subprocess_agent

with subprocess_agent("jevscript-adapter-codex", ["--backend", "tmux"]) as agent:
    with load("program.jev") as program:
        run = program.task("main").start(bind={"dev": agent})
        for pause in run:
            pass  # handle pauses
```

The helper serializes JSONL requests, forwards call and observation results,
and preserves the adapter's `retryable` error flag. It launches an argv
sequence without a shell. Closing it reaps the child process and leaves
reattachable agent panes alone.
