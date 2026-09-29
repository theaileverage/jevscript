# Spec gaps found while building jevs toolbox

The spec (`spec/jevscript-language-specification.md`) is the authority, and the
toolbox does not edit it. Each entry below is a place where the toolbox needed
something the spec or runtime does not give it, what it does instead, and where
that workaround is visible. They are for the implementation lead to decide on.

## 1. The CLI's `--stub` fails every typed tool verb

Section 11.6 says built-in stubs are "visibly identified as stubs" but not what
they return. The CLI stub (`crates/jevscript-cli/src/adapters.rs`) returns an
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
the body `jevscript serve` posted during the same run, for a named judgment and
both machine steps; checks that a judgment reworded under the same shape hash
still resends the recorded wording; and checks that each malformed response
fails a resend with the message the runtime gave for the same response.

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
- When a `jevscript serve` process dies while the SDK writes to it, the write's
  `EPIPE` surfaces as an unhandled `error` on the child's stdin and takes the
  host process down. Seen once when serve processes were killed by hand.

## 7. Replay uses the CLI, not `task.start { replay }`

`task.start` with `replay` needs a loaded program, and `program.load` needs
source. "Replay uses the recording only" is met by running
`jevscript replay <recording>` (section 11.6) with `TYPESAFE_API_KEY` and
`JEVSCRIPT_PROFILES` removed from its environment. The CLI prints only pauses,
so the toolbox reads the replayed run's events from the recording file.

## 8. Ideas live outside the repository, so relative `use` paths do not resolve

Ideas are stored under `~/.jevs-toolbox`, and each check writes the source to a
temporary file there. A program with a relative `use` (section 3.9) will not
find its library; `JEVSCRIPT_PATH` roots still apply.

## Moving from Jevscrypt to Jevscript

The toolbox was built against the older Jevscrypt repository (branch
`jevs-toolbox` at `f566526`) and moved here unchanged apart from what is listed
below. Each suspected issue in that copy was checked against Jevscript before
its workaround was kept or dropped. Outside `apps/toolbox`, the move adds a
`toolbox` job to `.github/workflows/ci.yml` and the `verify-toolbox` skill under
`.claude/skills/`; no runtime, SDK, adapter or spec file changed.

| Suspected issue | Checked against Jevscript | Outcome |
| --- | --- | --- |
| CLI `--stub` fails typed tool verbs (item 1) | Reproduced: `jevscript run examples/review_loop.jev --stub claude --stub tree --stub me` stops with `type_error: tree.test_summary is declared to return text but the adapter returned record` | Still present; the toolbox's typed stub stays |
| `Run.events()` never returns after the terminal pause (item 6) | `sdk/js/src/index.ts`: `#finish` sets the ended flag but does not wake an iterator waiting for the next event (code reading) | Still present; the toolbox still reads the recording file after the end |
| `EPIPE` from a dead `serve` crashes the host (item 6) | `sdk/js/src/rpc.ts` has no `error` listener on the child's stdin (code reading) | Still present in the SDK; the toolbox's own JSONL adapter had the same defect, reproduced in `test/bench.test.ts`, and is fixed |
| No raw-request RPC method (item 3) | Section 11.5 and `METHODS` in `sdk/js/src/rpc.ts` unchanged | Still present; the wire port stays, proven against the runtime's own bodies |
| `machine_step` has no verdict (item 4) | `Event::MachineStep` in `crates/jevscript-runtime/src/record.rs` unchanged | Still present; the verdict is still read from event order |
| `request` events have no origin (item 5) | `Event::Request` unchanged | Still present; the origin is still inferred |
| Replay needs source over JSON-RPC (item 7) | `task.start` still takes `program_id` (section 11.5) | Still present; replay still uses `jevscript replay` |
| Language server path and CLI names | The old default pointed at a separate `jevscript` checkout | Resolved: the default is `<JEVSCRIPT_BIN> lsp` from this checkout, and every name is `jevscript`/`JEVSCRIPT_*` |

Found while moving, and fixed in the toolbox only:

- **`check --tools` reports through diagnostics.** Jevscript prints
  `file:0:0: verb_missing: tree.flaky` with the usual cause, help and docs
  lines, where the old CLI printed a bare `verb_missing: tree.flaky`. The old
  parser matched only the bare form, so Check manifests silently reported
  nothing missing. `server/jevscript.ts` now reads `verb_missing` diagnostics.
- **The IR leaves out empty lists.** `crates/jevscript-ir/ir.schema.json` does
  not require `machines` (nor a machine's `params`), and `jevscript compile`
  omits them when empty. Every screen that read `ir.machines` threw on a
  program without a machine, which blanked the page. `shared/ir.ts` `readIr`
  fills them in where IR JSON arrives: `jevscript compile` output and a
  recording's `start` event.
- **An applied pin could not be reverted.** Clicking an edge or state reopened
  only `open` pins, so after Apply the pin's Revert button was out of reach.
  Applied pins reopen now.

Not taken up: recordings now carry `log` events (section 5.8), which the
toolbox does not display.

For the lead, outside the toolbox: section 9.2 writes `ask <text>, options
[<text>...]`, but `me.ask "Which lane?", options ["today", "week"]` is a
`syntax` error (`expected ]`, found `,`) at the first comma in the list. Bound
to a variable first (`options lanes`), it compiles and runs.

## Review status

The source lead reported 55 passing toolbox tests and a clean typecheck and
build in Jevscrypt. Codex reviewed commit `75a2612` there and reported seven
findings, all addressed in `808aef0` and `790f11f`; a re-review of those fixes
ended at model capacity without a verdict, so it is not a pass. None of that
is evidence about this copy. Here the suite was rebuilt at the toolbox's
boundaries (see the README), and the browser pass runs in CI.
