# How Jevscript programs work

Jevscript splits an agent loop between three parties. [Jev](https://docs.typesafe.ai),
TypeSafe's System One model, reads one structured state and answers typed
questions with probabilities, in parallel. It does not generate text and it
does not count. The program owns the loop, the thresholds, the budgets, and
the policy. The host binds everything that acts on the world, such as terminal
agents, people, text models, and tools, as capabilities at run time.

The [language specification](../spec/jevscript-language-specification.md) is
the authority for everything on this page.

## Every program runs the same loop

1. **Observe.** A capability reports what happened: an agent's transcript, a
   diff, or a test summary.
2. **Shape.** `shape` caps, in tokens, every field that will reach Jev. The
   context window is the limit, so a program stays inside it by default
   instead of repairing an overflow later.
3. **Judge.** `feels`, `pick`, and `rate` ask Jev questions whose answers are
   declared in advance. Consecutive independent questions share a logical
   request group. An `each` over the model profile's question cap can split
   that group into several requests. Cost is per request, so ask questions
   a branch might need up front when they can share a group.
4. **Decide.** Every branch, threshold, weight, and count is Jevscript code. A
   `gate` turns judgments into one of four verdicts, using thresholds that a
   task declares once.
5. **Act, and check it.** An agent's claim that it finished and Jev's
   estimate that the goal is met are both signals. `verify` declares the
   condition the task reports. The `done` pause carries `verified: true` only
   when that condition is true at the end. The flag is only as strong as its
   evidence. `verify(tree.tests_pass)` reads a test result, and
   `verify(j.claims_done > 0.9)` reads a model estimate. The host decides
   whether to accept the result.

## Four units make up a program

| Unit | What it is | What it can do |
| --- | --- | --- |
| `judgment` | A named, pure block of questions | Send one Jev request, or several chunks when an `each` is over the question cap. It has no side effects and no loops. You can run it alone against fixture state, which is how you evaluate questions against labelled examples. |
| `task` | The unit a host runs | Observe, judge, gate, and act, under declared budgets. It pauses the run at gates and when it asks a person. |
| `machine` | A state machine that Jev steps | Declare states and described events. Each step asks one Choice over the events whose code guards are true, plus `stay`, so Jev can pick only a legal transition. |
| `def` | A pure helper | Call judgments, use control flow, and do arithmetic. It cannot call capabilities, gate, or pause. |

Use a `task` when the loop is short and the branches depend on content. Use a
`machine` when the run has distinct phases, when the legal next moves depend
on the phase, or when a host wants to draw and pin the control flow. A
machine's terminal state counts as `verified` only when the transition into it
had a `when` guard that passed. The flag reports that a code guard passed, not
that the goal is objectively met, because a guard can read model or adapter
values.

## A run is a series of pauses

A host starts a run and steps it. The run stops at one of seven pause kinds:
`confirm`, `escalate`, `waiting`, `budget`, `error`, `stopped`, and `done`.
The host answers a pause to continue the run. `done` carries the outputs,
`verified`, and the usage.

The runtime writes every judgment, generation, capability result, and random
draw to a JSONL recording. A recording replays with zero model calls and the
same control flow. It holds the linked program and the resolved model profile,
so a replay needs no source files and no API key.

## Log lines go to the recording, never to Jev

`log info "routed request" { owner, confidence: c.confidence }` writes a line
to the recording. `x = log debug classify(message)` logs a value and returns
it, so a `log` can wrap any expression. The levels are `debug`, `info`, `warn`,
and `error`.

A `log` never reaches Jev, never counts against a budget, and never changes
control flow. Every unit may use it. Hosts receive each line through the SDKs'
log callback. A replay checks the recorded lines and does not emit them a
second time. `--redact` stores their values as hashes.

## Modules link into one flat program

`use "./lib/agent_loop.jev" as harness with claude, tree` imports another
file and maps the importer's capabilities onto the library's. The compiler
links every module into one flat program whose units have qualified names,
such as `harness.read_agent`. There is no package registry and no version
syntax. `JEVSCRIPT_PATH` and `--path` add search roots for `use` paths that
are not relative.

## Where to go next

- [Get started](getting-started.md) runs a first program end to end.
- The [examples](../examples) show a coding harness (`fix_issue.jev` and
  `lib/agent_loop.jev`), a judgment-only program (`inbox_triage.jev`), and a
  review machine (`review_loop.jev`).
- The [CLI reference](cli.md) lists every command and flag.
