# Working on Jevscript

## The spec is the authority

`spec/jevscript-language-specification.md` decides the language, the runtime
model and the host contract. The implementation lead owns it; **workers do not
edit anything under `spec/`**. When implementation shows the spec is wrong,
ambiguous or missing something, report it to the lead, who changes the spec
first and adds a row to the decision log in section 16. If this file, a doc
comment or your instructions disagree with the spec, the spec wins and the
other thing is a bug worth fixing.

Read the spec before you name anything. IR node kinds, pause kinds, error codes,
JSON-RPC methods, adapter verbs, keywords and label names are all fixed there,
and using a near-miss name is worse than leaving the work undone. Every type in
this repo carries a doc comment naming the section it comes from; keep that up
when you add one.

## Crate map

Dependencies point downward; nothing points back up.

| Crate | Owns | Depends on |
| --- | --- | --- |
| `jevscript-syntax` | Tokens, the lexer, the AST, `Span`, `Diagnostic` and the closed list of compile error codes | — |
| `jevscript-ir` | The IR types, their serde derives, the JSON Schema generator, judgment shape hashes | `jevscript-syntax` (for `Span`) |
| `jevscript-compiler` | AST to IR: module linking, batching, defaults, whole-program checks | syntax, ir |
| `jevscript-runtime` | Values, the run state machine, pauses, the Jev client, capabilities, model profiles, recording and replay, the JSON-RPC types | syntax, ir |
| `jevscript-lsp` | The language server: recovering parse, name index, diagnostics, highlighting, hover, navigation, outline, completion | syntax, compiler |
| `jevscript-cli` | The `jevscript` binary, `serve` and `lsp` | all of the above |
| `jevscript-conformance` | Checks a recording against spec section 15 (batching, state, pauses, machine menus) and scans the runtime for hard-coded limits | ir, runtime |

Two shapes to know about:

- **`Span` and `Diagnostic` live in `jevscript-syntax`,** and `jevscript-ir`
  depends on it for them. There is one definition of a source position and one
  closed list of error codes in the workspace.
- **The interpreter is synchronous.** A run is stepped by the host and is
  single-threaded (spec section 10.5). `tokio` appears in exactly one place —
  inside `jev::HttpJevClient`, which owns a current-thread runtime and blocks on
  it — so that everything above it can be paused, recorded and replayed.

Outside the workspace: `sdk/js` and `sdk/python` are host SDKs that spawn
`jevscript serve` and speak JSON-RPC over stdio; `adapters/claude-code` is a
TypeScript `agent` adapter that runs Claude Code in a tmux pane;
`editors/vscode` and `editors/zed` are VS Code and Zed extensions that start
`jevscript lsp`;
`examples/` holds fixtures copied verbatim from spec section 14.

## Implemented surface

Real, and tested: the lexer; the parser, over every fixture in `examples/`, the
prelude of sections 7.3 and 8.1, and a negative suite for each check it owns
(`unbounded_loop`, `pick_no_other`, `pick_arity`, `rate_arity`,
`rate_bare_degree`, `subject_not_path`, `verify_twice`, `event_no_description`,
`log_level`);
the IR types and shape hashes, for judgments and machines alike, with the
generated schema committed as `crates/jevscript-ir/ir.schema.json` and a test
that fails when it drifts; the whole compiler — linking (`link.rs`: `use`
resolution against the importer and `JEVSCRIPT_PATH`, `use_cycle`,
`use_has_inputs`, `use_needs_unmapped`, capability mapping, transitive
qualification `a.b.unit`, and the prelude of 7.3 and 8.1 embedded from
`prelude.jev` as module `std`), the whole-program checks (`check.rs`: every
code its module doc lists, the section 12 warning codes, and a
definite-assignment walk for `unassigned_read`), lowering (`lower.rs`, including
the `, max N, tail` policy) and the
batching pass (`batch.rs`, spec 6.6), proven over every fixture in `examples/`
by `tests/compile_examples.rs`, one program per code in `tests/check_errors.rs`
and `tests/warnings.rs`; the run state machine's transition and resume
rules; values (truthiness, text form, `==`, the plain and `$jev`-tagged JSON
forms); the expression evaluator (`eval.rs`: every operator, builtin,
comprehension and interpolation, with effects delegated through the `Effects`
trait); state construction and question building (`state.rs`: exactly the
subjects, `each` splitting at the profile's question cap, `pick among`, the
6.10 size check); answers to values and sampling (`answer.rs`, `rng.rs`); the
JSONL recorder and replayer, including self-contained execution metadata,
lossless redaction markers and a private full replay companion; the HTTP Jev
client with its TypeSafe wire mapping, plus `ScriptedJevClient` for tests; the capability verb
tables and manifest comparison; model profiles, including the bundle and the
`JEVSCRIPT_PROFILES` overlay; the JSON-RPC server's full method set, capability
round trips, queued concurrent requests, event notifications, framing and
error objects; both SDK clients, including bound identity, retryability,
module roots and profile overlays; all five verbs of the Claude Code adapter, against a fake
tmux and a fixture transcript in unit tests and against a real tmux server in
an integration test.

The CLI's `compile` (IR as pretty JSON on stdout, diagnostics on stderr as
`file:line:col: code: message`, exit 1 on any error) and `check` (diagnostics
only, warnings included, exit 0 when there are no errors; `--tools m.json`
compares every referenced tool verb against a section 9.4 manifest and prints
`verb_missing: <cap>.<verb>`), `judge`, `eval`, `run` and `replay` are real too.
`run` accepts only explicit built-in stubs or persistent JSONL subprocess
adapters; `replay` boots from the recording without source, ambient profiles,
adapters or model calls.

The statement interpreter is real for tasks, defs and machines
(`crates/jevscript-runtime/src/interp/`): every statement of section 5,
judgment execution by request group (6.6) and named judgments (6.7), `shape`
with its three policies and `strict` (7.2), `focus` executed directly with the
prelude def's semantics (7.3), `verify` (7.4), runtime-written `trail` records
(7.5), gates with every verdict and default arm (7.6), nested budgets with the
pause naming the task whose limit was hit (7.1), defs with defaults and the
recursion limit (8), every capability verb of section 9 and the pause each
raises (7.7, 10.2), `Run::inject` during `waiting`, recording with redaction
(10.3) and replay with identity checks and `replay_diverged` (10.4), and
`judge::run_judgment` for section 11.3. Machines run on the same call stack:
observe shaping, code guards, exact Choice menus and state, `recent`, sampling,
all threshold verdicts, counted stays, action blocks, nested budgets, terminal
proof, pauses with machine state, runtime event history and deterministic
recording replay are implemented. It is proven by
`tests/interp_statements.rs`, `tests/interp_capabilities.rs` (each rule named
in its test), `tests/interp_machines.rs` and `tests/interp_examples.rs`, which
compiles `examples/chief_of_staff.jev`, `examples/fix_issue.jev` and
`examples/review_loop.jev` with the real compiler and runs them under fake
adapters, including zero-live-call recording replay.

How resume and replay work: a run keeps its recording as an in-memory event
log. Every `Run::next` walks the task from the start with the log served back
(Jev answers, capability results, observations, draws, `log` lines and pauses, each with an
identity check). A live run continues execution at the end of its current log;
an external replay treats premature EOF as `replay_diverged` and never silently
calls a live service. A new host answer explicitly leaves replay. Nothing in
the interpreter holds a continuation or a thread. The clock and randomness are
`draw` events even when read for the `minutes` budget or paused time. File
recordings embed the linked IR, resolved profile and effective sampling option,
so standalone replay does not need source files or ambient profile settings.
Recording destinations must be new; replay validates the single-run envelope,
supported IR version and matching terminal event. JSON float parsing preserves
the exact recorded values. Host cancellation is an explicit `abort` event;
in-flight cancellation records the callback result before stopping at that
boundary. Failed external attempts, including client initialization, record
`effect_error` with their exact identity and error before retry, pause or abort;
replay reproduces them without calling a service or reading current credentials.

Decisions the interpreter made, each with its spec section in a doc comment:
`RECURSION_LIMIT` is 64 def frames; a capability name in value position
(`claude.spawn in tree`, 14.1) is a handle to the capability; a verb on a
handle reaches the adapter with the handle as its first positional argument; a
zero-argument verb written as a property and the `idle` of `wait` are resolved
by the evaluator, since the compiler keeps them as a field and a name; a
declared `out` is a variable of the task's root scope so a block can assign it;
`shape strict` overflows are `state_too_large`; `verified` on `done` is the
declared `verify` condition read again in the task's final environment (7.4),
so what happens after the loop counts, and a `return` that never reached the
verify loop is unverified without the condition being read at all (the
compiler rejects a `verify` anywhere but directly in the task body as
`syntax`, so that final read is always in task scope); `minutes` is checked
after every blocking Jev, LLM, adapter or observation return, after the host
retries a failed one and before the effect runs again, and once more before
`done` (7.1, 9.6), and the clock is read only while some task on the stack has
a `minutes` limit; a result written with `sample true` samples when its judgment
is run alone too, from a clock-seeded source or the one
`judge::run_judgment_with_draw` is given; `PauseCommon.step` and
`usage.steps` are the run task's outermost-loop iterations while a step
record's `step` is its ordinal in the trail; a step record is written once the
next observation in the same task settles its `changed`, so a task that never
observes after a call writes no record for it; a task called from a task takes
its parameters positionally or by name, and `main` run as a task with
parameters takes them from the inputs by name.

No implementation stubs remain in the crates, SDKs or Claude Code adapter.
The explicit CLI `--stub` adapters are demonstration fixtures. Section 15's
acceptance suites and their evidence boundaries are mapped in
`docs/conformance.md`.

Two decisions the compiler made that the runtime and the SDKs depend on:

- **Unit references are qualified names.** A call to a unit lowers to a call
  whose callee is `Expr::Name` holding the unit's linked name — `read_agent`,
  `harness.watch`, `std.stuck`, `a.b.unit` — and every unit in the linked IR is
  keyed by that name. Unqualified prelude names (`stuck(...)`) are rewritten to
  `std.stuck` unless a user unit shadows them. Zero-argument verbs written as
  properties (`dev.stop`, `tree.diff.files`) stay `Expr::Field`.
- **Shape hashes cover a unit's own name, not its alias.** Section 3.9 says
  moving a judgment between files without changing it keeps its hash, and the
  alias is what moving changes, so `harness.read_agent` hashes as
  `read_agent`. Machines hash the same way.

Every `Diagnostic` names its file (section 12): a library's own for anything
raised inside a library, the root's path for the root when it was compiled
from one, and nothing when it came from a string. The CLI prints from that
field.

A stub is a promise about shape, not about behaviour. When you implement one,
delete its `TODO` and its `NotImplemented`; do not leave a stub that sometimes
works.

## Conventions

- Rust 2024, toolchain pinned in `rust-toolchain.toml`. `#![forbid(unsafe_code)]`
  and `missing_docs` warnings are on for every library crate.
- **Every dependency is pinned exactly** in `[workspace.dependencies]`
  (`"=1.0.229"`). The same goes for the SDKs' `package.json` and
  `pyproject.toml`. A floating version is a silent change to a language
  implementation.
- `thiserror` for library error enums; `anyhow` only in the CLI.
- `serde` for everything that crosses a boundary. IR and recording events derive
  both `Serialize` and `Deserialize`.
- Doc comments name their spec section. Prefer one sentence about *why* over
  three about what the code plainly does.
- Tests assert behaviour the spec names, and say which rule they are checking.

## Checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo build                              # the SDK tests need target/debug/jevscript
cd sdk/js && pnpm install && pnpm test && pnpm run typecheck
cd sdk/js && pnpm run build              # the adapter's types come from the SDK's dist
python3 -m pytest sdk/python
cd adapters/claude-code && pnpm install && pnpm test && pnpm run typecheck
cd editors/vscode && pnpm install && pnpm test && pnpm run typecheck && pnpm run package
cd editors/zed && cargo build --release --target wasm32-wasip2 --locked   # needs the wasm32-wasip2 target
```

All of them pass on a clean checkout. Keep it that way.

## The IR schema

The spec calls the IR's published schema `spec/ir.schema.json` (section 11.1).
The generated file is committed as `crates/jevscript-ir/ir.schema.json`, and
`crates/jevscript-ir/tests/schema.rs` fails when the types and the file
disagree. Regenerate it with:

```sh
cargo run -p jevscript-ir --example schema > crates/jevscript-ir/ir.schema.json
```

The lead copies it into `spec/` when it changes.

## Settled specification decisions and implementation notes

The spec and its section 16 decision log remain authoritative. Report new
conflicts to the lead rather than working around them silently.

- **`loop` as a module alias** (resolved 2026-09-21). Section 2.5 reserves `loop`; example 14.1a now uses `as harness`, and section 3.9 states that an alias cannot be a reserved word. `examples/fix_issue.jev` is updated. The lexer test pinning keyword behaviour stands.
- **Capability kinds.** Section 9 describes four kinds, and the section 13
  grammar now admits exactly those four (`kind = "agent" | "person" | "llm" |
  "tool"`), so `CapabilityKind` has no open variant. If adapter-defined kinds
  come back, both the AST and IR enums need one again.
- **Grammar gaps the parser fills by the examples** (resolved 2026-09-21: section 13, the section 6.8 table and the decision log now say exactly this). Section 13
  has no place for `using` without a comma (`llm.write "..." using x`, 9.3 and
  14.3), writes a label's detail block as `NAME ":" detail_block` with a second
  colon (6.3 shows one), and gives `pick` and `rate` no detail block although
  6.8 says every verb takes one and 6.11 puts `sample true` on them. The parser
  reads `using` as a named argument with or without a comma, takes one colon,
  and accepts `focus`, `compare [...]` and `sample true|false` lines inside a
  `pick:` or `rate:` block (the keys that cannot be confused with a label). It
  also requires the `until` condition that 13 brackets as optional, since the
  AST has no way to hold its absence, and reports a missing event text as
  `event_no_description` because the grammar leaves no later stage able to.
  See the doc comments in `crates/jevscript-syntax/src/parser/`.
- **Codes for the prose rules and the warnings** (resolved 2026-09-21).
  Section 12 now names `gate_args`, `duplicate_name`, `assign_immutable` and
  `loop_control_outside`, a table of warning codes (`bare_prob_condition`,
  `uncapped_field`, `prelude_shadowed`, `machine_unreachable_done`,
  `detail_ignored`), a `file` on every diagnostic, and the `, tail` policy
  syntax; the compiler raises exactly those. `duplicate_name` covers every
  repeated name or key section 12 lists — units, module aliases, states,
  events, judgment results, detail keys, gate arguments and gate arms — split
  between the parser (the three the grammar's repetitions admit) and the
  compiler (the rest). `syntax` is left for what the grammar rejects. A `with`
  entry naming a capability the library does not declare is still
  `use_needs_unmapped`, which section 12 does not spell out.

- **A judgment block's state** (resolved 2026-09-21). Section 6.9 now says
  the named-judgment rule includes every declared parameter in full, even one
  no subject reads, in every permitted `each` chunk; inline chunks carry only
  their paths. `state::build_judgment_request` does exactly that and the
  interpreter and `judge::run_judgment` use it; section 6.6 now also says the
  question cap never splits a group unless one `each` is over the cap by
  itself, and an oversized group is `state_too_large` before anything is sent.
- **`repeats` counts every capability call.** Section 7.5 makes a step record
  of every call the runtime makes, and the prelude's `repeats` (8.1) counts
  identical `[action, target, args]` triples. A loop that asks `tree.tests_pass`
  each iteration therefore has `repeats >= 2` from its second iteration, and
  `stuck` returns true before asking Jev anything. `examples/fix_issue.jev`
  nudges "Stop. In three lines..." on every iteration after the first under
  the fakes in `tests/interp_examples.rs`. This conservative prelude heuristic
  includes repeated queries; do not silently omit them from the trail.
- **`wait`'s mode word.** `dev.wait idle` lowers `idle` as a bare name (the
  checker exempts it); the evaluator hands an unbound name in a `wait` call to
  the adapter as text, matching the section 9.1 mode word.

## Things that are easy to get wrong

- **Batching is observable.** Conformance item 2 checks the grouping from the
  recording's `request` events, so `request_group` is not a hint. A call to a
  named unit — judgment, def, task or machine, wherever it sits in the
  statement — closes the open group, because batching never crosses a call
  boundary (3.9); only the builtins of 5.7 are pure.
- **State is exactly the subjects.** A variable in scope that no subject names
  does not reach Jev (spec section 6.9). Sending more is a correctness bug, not
  an optimisation question. The one exception is a named judgment, whose state
  is every declared parameter in full and nothing else; that is a different
  rule, not a loophole.
- **`tail` and `last_message` are agent-written.** Never let them steer control
  flow except through a judgment whose answers were declared in advance. The
  runtime writes `trail` records from its own calls for exactly this reason.
- **Replay must be exact.** Every non-deterministic source — the clock, the
  random source, every adapter call — is recorded and replayed. Reaching for
  `SystemTime::now()` or a fresh `Uuid` inside the interpreter breaks the
  determinism guarantee in spec section 10.4. Sampling (6.11) goes through the
  same recorded random source, which is the only reason a sampled run replays.
- **Limits live in profiles, never in code.** Token limits, the questions-per-
  request cap, the `pick` criteria cap and prices all come from
  `profile::Profile` (spec section 10.6). A literal `64000` anywhere in the
  runtime is a bug, and the bundle in `crates/jevscript-runtime/profiles/` is
  data with a date on it.
- **A machine's guards are the only thing that proves done.** Jev picks among
  legal transitions; a terminal state entered through an unguarded event is
  `done` but not `verified`, and hosts are told to tell the difference.
