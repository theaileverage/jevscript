# CLI Chat verification

Chat now invokes the selected signed-in local CLI. Jev runtime bindings and
recordings remain separate from this response-only drafting path.

## Live CLI evidence

On 2026-09-29 at 18:53 UTC, the toolbox `/ws` boundary sent the full language spec
and a request for a greeting program to both installed CLIs. No Anthropic or OpenAI
API key reached either subprocess. Each answer supplied a complete program that
the real `jevscript compile` accepted with zero diagnostics on its first attempt.

| CLI | Version | Selected model | Reported response model | Result |
| --- | --- | --- | --- | --- |
| Claude Code | 2.1.284 | `default` | `claude-opus-5-5` | Checked greeting program, one attempt |
| Codex | 0.159.0 | `gpt-6.1-sol` | `gpt-6.1-sol` | Checked greeting program, one attempt |

Claude Code's `auth status --json` returns a JSON document, while its initialization
control response is JSONL. Discovery handles both formats. Its initialization
response advertised 12 model choices. Codex's `model/list` advertised eight visible
choices, with `gpt-6.1-sol` first. These catalogs do not guarantee that every model
will accept a later request. Claude Code aliases report their resolved model when
available in `modelUsage`; Codex exec reports the selected concrete catalog ID.

## Deterministic checks

The executable fixtures in `test/agent-fixture.ts` implement auth, model discovery,
stdin prompts, completion envelopes and deliberate failures. They reach no live
provider. The integration tests enter `/ws` and exercise the real compiler and
SQLite database. The browser tests operate the built page and process-level demo.

Coverage includes both CLI choices, nondefault model selection, conversation
context after changing agents, reply provenance, saved selection and messages
after reload and server restart, earlier SQLite schema upgrade, signed-out and
missing executables, stale models, safe failure messages, input/output limits,
timeout termination, pasted-program checking, and absence of fixture fallback.
Existing run, recording/replay, annotation, adapter and LSP checks remain in the
verification driver.

Run the project-local verification skill:

```sh
.claude/skills/verify-toolbox/scripts/doctor.sh
.claude/skills/verify-toolbox/scripts/drive.sh /absolute/path/to/evidence
```

## Local trial

Build the runtime, SDK and adapters as described in the toolbox README. Start the
demo from this checkout:

```sh
cd apps/toolbox
pnpm demo
```

Open `http://127.0.0.1:5188`, create an idea, choose Claude Code or Codex and a
model, then ask for a greeting program. The response names the CLI and model.
The program shows its compiler result. Jev runs and machine annotation remain
visibly labeled fixtures. Restart the demo on the same home to verify that the
conversation and selected model return. Use a distinct `JEVS_TOOLBOX_PORT` if
another demo is running. Do not reset an existing toolbox home for this check.

If a CLI is unavailable, install and sign in using `claude auth login` or `codex
login`, then click Refresh agents. API keys do not enable these CLI choices.
