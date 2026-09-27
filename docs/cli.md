# CLI reference

The `jevscript` command ships in the npm package `@theaileverage/jevscript`
and the PyPI package `jevscript`. A source build writes it to
`target/debug/jevscript`. `jevscript <command> --help` prints the options of
each command.

## Commands

| Command | What it does |
| --- | --- |
| `jevscript compile <file.jev>` | Prints the linked IR as JSON on stdout. |
| `jevscript check <file.jev> [--tools <manifest.json>]` | Prints warnings and errors without the IR. With `--tools`, compares every referenced tool verb against a tool adapter manifest and reports a missing verb as `verb_missing: <cap>.<verb>`. |
| `jevscript judge <file.jev> <judgment> --state <json>` | Runs one judgment against the given state and prints the answers. Calls Jev and reads `TYPESAFE_API_KEY`. |
| `jevscript eval <file.jev> <judgment> --cases <cases.jsonl>` | Runs a judgment over `{ state, expected }` rows and reports accuracy. Calls Jev and reads `TYPESAFE_API_KEY`. |
| `jevscript run <file.jev>` | Runs the task `main` and answers its pauses on the terminal. Prints each pause as JSON on stdout and each `log` line on stderr. |
| `jevscript replay <recording.jsonl>` | Replays a recording with no model or adapter calls and prints the recorded pauses and `log` lines. |
| `jevscript serve` | Runs the JSON-RPC 2.0 server on stdio that the SDKs drive. |
| `jevscript lsp` | Runs the language server on stdio. See [editor support](editors.md). |
| `jevscript setup --agent <agent>` | Installs the bundled coding-agent Skill. |

`judge` and `eval` also take `--model <id>`, which selects the Jev model and
its profile, and `--profiles <file>`.

## Diagnostics and exit codes

`compile`, `check`, and `run` print each diagnostic on stderr as
`file:line:col: code: message`, followed by the source line, a cause, a fix,
and a link to the code in the [error reference](error-reference.md). Any error
exits with code 1. Warnings alone exit with code 0.

## `run` options

| Option | What it does |
| --- | --- |
| `--input <json>` | The task inputs, as a JSON object keyed by `in` name. |
| `--bind <NAME=COMMAND>` | Binds capability `NAME` to a persistent subprocess that speaks the [adapter protocol](../crates/jevscript-cli/README.md). |
| `--stub <NAME>` | Binds capability `NAME` to a built-in demonstration adapter. The CLI prints a notice on stderr when it does this. |
| `--record <path>` | Writes the recording to a new file. The CLI refuses a path that exists. |
| `--redact` | With `--record`, stores hashes and token counts instead of payloads. See below. |
| `--sample` | Draws labels and levels from Jev's distribution instead of taking the most likely one. |
| `--profiles <file>` | A profiles file layered over the bundled profiles and `JEVSCRIPT_PROFILES`. |
| `--path <dir>` | A module search root for `use` paths that are not relative. Repeat it for more than one root. |

`run` never binds a capability on its own. Every `needs` declaration needs a
`--bind` or a `--stub`, or the run fails with `binding_missing`.

```sh
jevscript run program.jev --input '{"issue":"Fix the parser"}' \
  --bind tree='./tree-adapter' --record run.jsonl
jevscript replay run.jsonl
```

## Recordings

A recording is a JSONL file. It holds the linked program, the resolved model
profile, the sampling option, and every event of the run, so `replay` needs no
source files, profiles, adapters, or API key. The CLI and the SDKs never
overwrite or append to a recording, so each run needs a new path.

With `--redact`, the primary recording hides request state, observations, and
`log` values. The CLI writes the full data to `<path>.replay.jsonl`, with
owner-only permissions where the platform supports them, and prints its path.
Keep both files to replay. Inputs, outputs, call arguments, and generated text
stay visible in the primary file.

## `setup` options

`jevscript setup` copies the Skill that is embedded in the binary. It runs
offline and only when you run it. Package installation never runs it.

| Option | What it does |
| --- | --- |
| `--agent codex` | Installs for Codex, which reads `.agents/skills`. |
| `--agent claude-code` | Installs for Claude Code, which reads `.claude/skills`. |
| `--project <dir>` | Installs into an existing project directory. The default is the current directory. |
| `--global` | Installs into your home directory instead of a project. |
| `--copy` | Writes independent copies instead of links at the agent paths. |

Repeat `--agent` to install for both agents. A project install writes the
Skill to `<project>/.agents/skills/jevscript`, and Claude Code gets a link at
`<project>/.claude/skills/jevscript`. A global install writes it to
`~/.agents/skills/jevscript`, with links at `~/.codex/skills/jevscript` and
`~/.claude/skills/jevscript` for the agents you name. Running the same setup
again reports `already installed`. Setup refuses a destination that it did
not create or that was changed since.

## Environment variables

| Variable | What it does |
| --- | --- |
| `TYPESAFE_API_KEY` | The bearer token for Jev. Commands and runs that call Jev need it. |
| `JEVSCRIPT_PROFILES` | A JSON file of model profiles, layered over the bundled ones. Token limits, request caps, the tokenizer, and prices live in profiles. |
| `JEVSCRIPT_PATH` | Colon-separated module search roots for `use` paths that are not relative. |
| `JEVSCRIPT_BIN` | The runtime binary the SDKs start instead of the packaged one. |
