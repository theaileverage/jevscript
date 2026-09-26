# Conformance evidence

[Spec section 15](../spec/jevscript-language-specification.md#15-conformance)
defines acceptance. The suites below exercise those rules; a green checker over
one recording alone is not a proof of the entire language implementation.

| Item | Evidence |
| --- | --- |
| 1. Examples and rejected programs | Compiler `tests/compile_examples.rs`, `tests/check_errors.rs`, and `tests/warnings.rs`; conformance `tests/section15_examples.rs` compiles all five root examples and the imported library from disk. |
| 2. Request batching | Compiler `tests/batching.rs`, runtime `tests/interp_statements.rs`, and direct request-count/question-id assertions over example recordings. The checker compares groups against linked IR and uses the recorded profile for `each` caps. |
| 3. Exact state | Runtime state and interpreter tests cover inline subject paths, full named-judgment parameters, comparisons and chunking. Example tests assert state keys directly as well as running the recording checker. |
| 4. Exact replay | Runtime capability, statement and machine tests cover recording, redaction companions, pauses, sampling, premature EOF and divergence. Failed external attempts, missing credentials, and host cancellation replay with complete pause equality. Example tests compare every surfaced pause and replay with Jev/adapters that panic if called, then replay from the recording alone. |
| 5. Host contract | CLI protocol and execution tests, `sdk/js/test/rpc.test.ts`, and `sdk/python/tests/test_rpc.py` drive the actual `jevscript serve` subprocess. |
| 6. Legal machine menus | Runtime `tests/interp_machines.rs` and `tests/interp_examples.rs`, plus direct expected menus and transitions for `review_loop.jev` in the conformance example suite. |
| 7. Profile-driven limits and prices | Runtime profile/state tests and the conformance suite's recursive runtime-source scan. The bundle records [provenance and policy limits](../crates/jevscript-runtime/profiles/README.md). |
| 9. Logs | Syntax `tests/parse_logs.rs`, compiler `tests/logs.rs`, runtime `tests/interp_logs.rs` (recording, budgets, judgment and machine logs, resume, replay without re-emission, divergence, redaction) and CLI `tests/run_replay_cli.rs`. The recording checker's `check_logs` rejects a log recorded inside an open request, and the `chief_of_staff.jev` example asserts its logs and a replay's with `logs_diverge_at`. Both SDK suites cover the log callback, `logs()` and standalone judgment logs. |

Run the complete commands in [AGENTS.md](../AGENTS.md) using the pinned Rust
toolchain. Build the CLI before SDK tests and build the JavaScript SDK before
the Claude Code adapter's type check.

The end-to-end example tests use scripted model answers and fake capabilities.
They establish language execution and replay behavior without relying on model
judgment quality or a live coding session. The Claude Code adapter also has a
real tmux integration test. Live provider smoke tests are separate evidence and
require an API key; the ordinary suite does not depend on them.

The recording checker can verify recorded menu membership and transition
structure, but arbitrary guard truth needs runtime execution evidence: guard
results are not separately recorded. Model decisions also remain probabilistic;
conformance does not claim that a model's choice proves a task is complete.
