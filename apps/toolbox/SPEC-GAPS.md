# Spec gaps found while building jevs toolbox

The spec (`spec/jevscrypt-language-specification.md`) is the authority, and the
toolbox does not edit it. Each entry below is a place where the toolbox needed
something the spec or runtime does not give it, what it does instead, and where
that workaround is visible. They are for the implementation lead to decide on.

## 1. The CLI's `--stub` fails every typed tool verb

Section 11.6 says built-in stubs are "visibly identified as stubs" but not what
they return. The CLI stub (`crates/jevscrypt-cli/src/adapters.rs`) returns an
empty record for every `tool` verb. The runtime checks declared return types
(section 9.4), so `examples/review_loop.jev` under `--stub tree` stops at once
with `type_error: tree.test_summary is declared to return text but the adapter
returned record`.

**Workaround.** The toolbox's stub (`server/adapters.ts`, `stubAdapter`)
mirrors the CLI stub verb for verb, except that a declared tool verb returns a
value of its declared type: `text` → `"stub <verb>"`, `bool` → `true`,
`number` → `0`, `list` → `[]`, `record` → `{}`, `none` → `null`. The Adapters
screen says so on the Stub card. **Decision needed:** whether the CLI stub
should do the same, and whether section 11.6 should say what stubs return.

## 2. JSON-RPC has no built-in stubs

`task.start` binds only host adapters (section 11.5); `--stub` exists only in
the CLI. The toolbox drives runs through `sdk/js`, so its stubs are host-side
adapters and appear in the recording as ordinary `call` events, not as CLI
stubs. Not a defect, but "built-in `--stub`" in the toolbox means "the
toolbox's stub adapter".

## 3. No raw-request method, so resend ports the wire mapping

Section 11.5 has `judgment.run` but nothing that sends an arbitrary request.
`judgment.run` cannot resend a recorded request exactly: it rebuilds the
questions from the idea's current source and resolves the current profiles,
and a matching shape hash does not prove those equal the recorded ones,
because the hash covers the answer shape (section 11.4), not descriptions,
conditions or detail blocks. So the Requests tab resends **every** request (named
judgments, inline judgments and machine steps) by posting the shown state and
questions to the recorded profile's endpoint with `TYPESAFE_API_KEY`, using a
TypeScript port of `wire::request_body` and `wire::parse_response`
(`shared/wire.ts`). `test/resend.test.ts` proves the port byte-identical to
the body the runtime posts, for both a named judgment and machine steps, and
checks that a judgment reworded under the same shape hash still resends the
recorded wording. `test/wire.test.ts` checks that malformed responses fail
with the runtime's messages.

No RPC method was added. Resending never writes the recording (the tests
compare its hash before and after). **Decision needed:** whether a
`request.send` method belongs in a later version of section 11.5, so hosts do
not have to track `jev.rs`.

## 4. `machine_step` does not record the gate

Section 7.8's `machine_step` carries `state, enabled, chosen, probabilities,
confidence, to`, but not the thresholds, the risk value or the verdict. The
step inspector shows:

- **thresholds** from the IR embedded in the recording's `start` event;
- **risk** as 1 when the chosen event is `risky` in that IR, else 0 (the rule
  section 7.8 states);
- **verdict** from the `confirm` or `escalate` pause recorded between the
  step's `answers` and its `machine_step` (the spec records the step after the
  gate resolves); `stay` when Jev chose `stay`; `stop` when the destination did
  not change and no pause explains it; `proceed` otherwise.

All of it comes from recorded events and the recorded IR, but the verdict is a
reading of the event order rather than a recorded field.
**Decision needed:** whether `machine_step` should carry `verdict` (and the
host's answer for a `confirm`).

## 5. A `request` event does not say where it came from

The Requests tab labels each request with its machine state, judgment name or
"inline". The `request` event has no origin field, so the toolbox infers it
(`shared/requests.ts`): a machine step is one Choice with id `event` over
`{state, goal, obs, recent}` (section 7.8), and a named judgment is a request
whose state keys are exactly the judgment's parameters and whose question ids
are its result names (section 6.9). An inline judgment that happened to match
both rules would be labelled as the named one.

## 6. SDK behaviour the toolbox works around

- `Run.events()` in `sdk/js` ends only after an event arrives once the run has
  ended; after the terminal pause no event arrives, so the iterator never
  returns. The toolbox reads the recording file for anything after the end.
- When a `jevscrypt serve` process dies while the SDK writes to it, the write's
  `EPIPE` surfaces as an unhandled `error` on the child's stdin and takes the
  host process down. Seen once when serve processes were killed by hand.

## 7. Replay uses the CLI, not `task.start { replay }`

`task.start` with `replay` needs a loaded program, and `program.load` needs
source. "Replay uses the recording only" is met by running
`jevscrypt replay <recording>` (section 11.6) with `TYPESAFE_API_KEY` and
`JEVSCRYPT_PROFILES` removed from its environment. The CLI prints only pauses,
so the toolbox reads the replayed run's events from the recording file.

## 8. Ideas live outside the repository, so relative `use` paths do not resolve

Ideas are stored under `~/.jevs-toolbox`, and each check writes the source to a
temporary file there. A program with a relative `use` (section 3.9) will not
find its library; `JEVSCRYPT_PATH` roots still apply.

## For the migration to Jevscript

This toolbox was built against the older Jevscrypt repository. None of the
items below was fixed here; each needs checking against current Jevscript
before the toolbox moves, and the toolbox's workaround dropped if Jevscript
already behaves.

| Suspected issue in this copy | Where seen | Toolbox dependency to revisit |
| --- | --- | --- |
| CLI `--stub` returns `{}` for typed tool verbs, so `review_loop.jev` fails with `type_error` (item 1) | `crates/jevscrypt-cli/src/adapters.rs` | `server/adapters.ts` `stubAdapter` typed returns |
| `sdk/js` `Run.events()` never returns after the terminal pause (item 6) | `sdk/js/src/index.ts` | `server/runs.ts` reads the recording file after the end |
| `sdk/js` crashes the host with an unhandled `EPIPE` when `serve` dies mid-write (item 6) | `sdk/js/src/rpc.ts` | none yet; the toolbox server would still go down |
| No raw-request RPC method; resend ports `wire::request_body`/`parse_response` (item 3) | spec 11.5, `jev.rs` | `shared/wire.ts` and its byte-parity test |
| `machine_step` has no verdict field (item 4) | spec 7.8, `record.rs` | `shared/recording.ts` `machineSteps` |
| `request` events have no origin (item 5) | spec 10.3 | `shared/requests.ts` `originOf` |
| Language server paths and CLI flags | `JEVS_LSP_COMMAND` default points at the separate `jevscript` checkout | `server/lsp.ts`, `server/env.ts`, `server/jevscrypt.ts` (binary name `jevscrypt`) |

Also for the migration: the language server requires `tokenTypes` and
`tokenModifiers` in the semantic-tokens client capability and exits without
them (LSP 3.17 marks them required, so this is correct; `src/lsp.ts` sends
them).

## Review status

Codex (`gpt-6-sol`) reviewed commit 75a2612 and reported seven findings. All
seven were addressed in 808aef0 and 790f11f, each with a test except the Chat
"checking…" display (no DOM test tooling). A Codex re-review of those fixes was
started but ended with "Selected model is at capacity" before it wrote a
report, so the fixes are verified by tests and a browser pass, not re-reviewed.
