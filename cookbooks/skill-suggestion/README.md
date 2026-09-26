# Jevscript skill suggestion cookbook

This is a runnable Jevscript translation of TypeSafe AI's
[Skill suggestion cookbook](https://docs.typesafe.ai/cookbooks/skill_suggestion).
It uses a 157-skill, explicitly licensed subset of a dated Hermes source
snapshot and preserves the original two-stage policy: one wide ranking plus three independent gate
questions, deterministic top-three construction and thresholds, then one
detailed rerank plus three independent fit checks.

## Quick start

From the repository root:

```sh
cargo build
target/debug/jevscript check cookbooks/skill-suggestion/skill_suggestion.jev
python3 cookbooks/skill-suggestion/runner.py verify
python3 -m unittest discover cookbooks/skill-suggestion -p 'test_*.py' -v
```

`verify` is fixture/scripted evidence. It starts a localhost TypeSafe-shaped
HTTP endpoint, then runs the real Python SDK, JSON-RPC server, compiler, and
runtime. It makes no external request. On sandboxed hosts, binding localhost
may require the host's normal socket permission.

Run one fixture and optionally create a new recording:

```sh
python3 cookbooks/skill-suggestion/runner.py scripted \
  --case apple_notes \
  --record /tmp/apple-notes-suggestion.jsonl
python3 cookbooks/skill-suggestion/runner.py replay \
  /tmp/apple-notes-suggestion.jsonl \
  --request "Can you save this recipe as a new note in Notes.app?"
```

Recording destinations must not already exist. Replay uses the recording's
embedded IR and profile and performs zero Jev calls.

## Live TypeSafe run

```sh
python3 cookbooks/skill-suggestion/runner.py live \
  --model jev-latest \
  --request "Create a pitch deck from our firm template and model." \
  --record /tmp/skill-suggestion-live.jsonl
```

The runner reads `TYPESAFE_API_KEY` from the process environment, `.env.local`,
or `.env` at the repository root, in that precedence order. It never prints the
key.

The full path makes exactly two Jev requests. The early prose-only gate makes
one. `jev-latest` uses this repository's resolved model profile; the upstream
published benchmark used `jev-1.12` on 2026-07-31.

## Input and output

The `.jev` program declares:

- `request: text`: the latest user request.
- `roster: list`: 157 records with `name`, `category`, `description`,
  `description_full`, `body`, plus runner-derived `label: "iN"`. The label
  maps Choice probability keys back to source records; it does not rank them.

The terminal `done.outputs` record contains:

- `suggestion`: one skill name or `none`.
- `gate_score`: `(acts + procedure + (1 - prose_suffices)) / 3`.
- `shortlist`: the deterministic top three, or `[]` when the first gate exits.
- `fits`: the three second-stage Noul answers, or `[]` on early exit.

The four local fixture cases are Apple Notes (obvious match), PowerPoint
authoring versus an HTML deck (lookalikes), Mastodon (no exact skill), and a
prose-only monad explanation.

## Architecture

1. Jevscript derives `wide_roster`, containing only name, 60-character
   description, and index label. One request batches a 157-option `pick among`
   with the three independent Noul gates.
2. Jevscript computes the oriented mean, applies 0.30, maps `iN`
   probabilities back to the full roster, and runs `top(..., n 3)`.
3. Jevscript constructs three detailed criteria from full descriptions plus
   700-character body excerpts. One request batches the rerank Choice with
   `each detailed feels ...`, producing three independent fit Nouls.
4. Jevscript applies the 0.30 fit threshold and returns the reranked item or
   `none`.

Only subjects and explicit compare paths reach Jev. Tests pin the first state
to `{request, wide_roster}` and the second to `{request, detailed}`, so the
full 1,600-character bodies never leak into the wide request.

## Why the small `label` annotation exists

`pick among` returns probabilities keyed by `i0`, `i1`, and so on. Records are
ordered maps, so iterating probability values would sort `i10` before `i2`.
The host adds the deterministic source index as `label`, and Jevscript reads
`wide.probabilities[skill.label]` before calling `top`. Shortlist construction,
arithmetic, branching, and thresholds remain in Jevscript.

## Benchmark harness

Print upstream published/static results:

```sh
python3 cookbooks/skill-suggestion/benchmark.py published
```

Score captured agent turns:

```sh
python3 cookbooks/skill-suggestion/benchmark.py score turns.jsonl
```

Each JSONL row is:

```json
{"arm":"baseline","text":"...","gold":"apple-notes","loaded":["apple-notes"]}
```

The three measured arms are `baseline`, `jevscript_suggestion`, and `oracle`.
The metrics match upstream: wrong first load on covered rows and any load on
uncovered rows.

A live run is deliberately opt-in. The built-in measured-agent adapter uses
OpenRouter's `deepseek/deepseek-v4.1-flash` and the current licensed-subset prompt:

```sh
python3 cookbooks/skill-suggestion/benchmark.py live \
  --requests cookbooks/skill-suggestion/data/sample_requests.json \
  --output /tmp/skill-suggestion-turns.jsonl \
  --typesafe-model jev-latest \
  --agent-model deepseek/deepseek-v4.1-flash \
  --allow-custom-dataset \
  --confirm-paid-run
```

This loads `OPENROUTER_API_KEY` and `TYPESAFE_API_KEY` from the process/project
environment without overriding shell values. Python installations without a
configured CA file use the operating system's standard CA bundle; TLS
verification is never disabled. Pass `--agent-command` to measure a different
compatible agent. That command receives one JSON object on stdin with `request`,
`suggestion`, and `arm`, and must print `{ "loaded": ["skill-name", ...] }`.
An upstream-sized run can make up to 976 Jev requests and exactly 1,464 agent
turns.

### Measured local smoke result (2026-09-21)

The checked-in [live report](reports/live-smoke-2026-09-21.md) and
[raw JSONL](reports/live-smoke-2026-09-21.jsonl) come from 4 real Jevscript
runs (7 TypeSafe requests) and 12 real OpenRouter agent turns using the command
above with the former 182-skill roster. They are historical evidence and do
not measure this licensed subset. All three arms measured 0% wrong first load over 2 covered rows and 0%
needless load over 2 uncovered rows. This is a connectivity and end-to-end
behavior check, not a reproduction of the unavailable 488-row benchmark and
not evidence that the three arms are equally accurate at scale.

## Jevscript versus the original Python recipe

The executable policy is 39 lines of Jevscript. The official page's Python
blocks for stages 3–4 plus `suggest()`/`suggestion_block()` total 172 lines;
those blocks also include caches, timing, and demo printing, so this is a
responsibility comparison rather than a language productivity benchmark. The
rest remains explicit host code here: `runner.py` is 302 lines,
`benchmark.py` is 179 lines, the OpenRouter measured-agent adapter is 140, and
the focused test suite is 142.

| Responsibility | Jevscript | Python still does |
| --- | --- | --- |
| Request grouping | Compiler assigns two observable request groups | No manual question dictionaries or batching |
| Call budget | `budget calls 2` | Starts/drives the run |
| Typed judgments | `pick among` and `feels` | Supplies typed JSON inputs |
| State | Derived wide and detailed records; subject-only state | Loads/checksums the licensed subset |
| Policy | Mean, inversion, top three, both 0.30 thresholds, branches | None for a single suggestion |
| Profiles | Runtime-owned limits/model endpoint | Selects `jev-latest` or an override |
| Recording/replay | Runtime records exact requests, answers, and control flow | Chooses file paths and displays results |
| Host boundary | No capabilities are needed by this pure task | Dataset iteration and the measured external agent |

Jevscript replaces the suggestion policy, not the entire benchmark. Python is
still the appropriate host for iterating 488 rows, invoking the external
agent under measurement, caching artifacts, and computing aggregate metrics.

## Evidence and limitations

See [reports/verification.md](reports/verification.md) for the exact local
results and [PROVENANCE.md](PROVENANCE.md) for the source pin, checksums, and
licensing boundary.

- Scripted probabilities prove control flow, state shaping, request grouping,
  abstention, recording, and replay. They do not measure Jev accuracy.
- The current roster omits 4 proprietary and 21 license-undeclared skills.
  The prompt and selection results are not upstream parity evidence.
- The official 488-row dataset is described but not downloadable from the docs
  page, so upstream metrics are not locally reproduced.
- The official cookbook itself reports Mastodon as a surviving false positive.
  The local Mastodon fixture intentionally drives all second-stage fits below
  0.30 to prove the abstention branch; a live model may still pick `xurl`.
- No capability call or external side effect is authorized by a suggestion.
