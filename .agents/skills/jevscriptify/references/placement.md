# Placing a decision point

Placement decides, for each decision point, where the logic runs and which
record holds the decision. There are four places. Choose the first that fits.

## The test: does it earn a place in Jevscript?

A decision point belongs in Jevscript when it has at least one of these:

| Value | Meaning | Example |
| --- | --- | --- |
| Judgment | It asks Jev something only a reader of the text can answer. | "Does this status line ask the owner for a credential?" |
| Guard | Code decides from a Jev answer, or authorizes, orders or refuses an effect. | `if risky > policy.risk_hold:` before starting work; a `when` guard on `land`. |
| Recording | The decision must replay with the run it belongs to. | A route whose recording a later verification replays. |

Anything with none of the three stays in the host. Code that already runs
inside a recorded run for one of those reasons stays with that run, even when
it is plain arithmetic: moving it out would split one decision across two
records.

## The four places

### 1. Host code

The host is the trusted boundary. Keep these there:

- persistence, identity, locking, scheduling, polling and restarts;
- validating anything that enters from outside, including a program's `out`
  before the host acts on it, against the host's own registry or closed set;
- credentials, grants and the execution of effects, with idempotency keys and
  reconciliation, since a crash inside an effect has an ambiguous outcome;
- retrieval, filtering and ranking that code can do without judgment, such as
  the eight most recent threads in a scope or a text search over a catalog;
- rendering and formatting of facts the host already holds;
- turning a person's reply into one of the options they were offered. Validate
  it where it enters; do not ask Jev what the person meant by "yes".

Move logic out of the host when the host is secretly judging. A regex, keyword
list or prefix match over agent-written or person-written text that steers
control flow is a judgment without a declared answer space. Also move it when
the host repeats a program's vocabulary, such as a copy of a `pick` label list
or a set of machine event names it branches on. Have the program return the
fact the host needs instead.

### 2. A unit inside an existing `.jev` program

Use this when the decision runs inside a run that already exists and shares its
inputs, capabilities and recording. Pick the unit kind by what it must do:

| Unit | Choose it when | It cannot |
| --- | --- | --- |
| `judgment` | A fixed logical group of questions over declared parameters. It is the only unit `jevscript judge` and `jevscript eval` can run alone, and its state is every parameter in full. A long `each` may split into multiple network requests under the selected profile. | Branch, loop or call anything. |
| `def` | Code, optionally with inline judgments, that returns a value. Inline state is only the subject and `compare` paths. | Call a capability, gate or pause. |
| `task` | It calls capabilities, gates, asks a person, sets a budget or verifies. | Be run alone by `judge` or `eval`. |
| `machine` | The run has phases, the legal next moves depend on the phase, and every step happens inside this one run. | Start in a state chosen at run time (`initial` is static), or last beyond the run. |

Put questions that must be measured in a `judgment` and call it from the `def`
or `task` that uses the answers. Put a policy number in the program's policy
input, not in the question.

### 3. A separate `.jev` program

Give the logic its own program when at least one of these holds:

- the host starts it on its own trigger, not as a step of an existing run;
- it needs its own `in`/`out` contract that a host consumer reads;
- it needs its own recording, budget or replay lifecycle, such as a labelling
  pass or a generated playbook that is verified by replay before it goes live;
- it needs fewer capabilities than the program it would join, and least
  authority matters.

A file with no `in` or `out` is a library: other programs `use` it and hand it
capabilities explicitly with `with`. Prefer a library when the logic is shared
and has no entry point of its own.

### 4. A durable host state machine whose transitions a `.jev` program decides

Use this when the state must outlast one run: across wakes, restarts or days.
A Jevscript `machine` cannot hold it, because every run is bounded and starts
from a static `initial`.

The contract:

1. The host persists the current state and owns the closed set of states and
   the table of legal `(state, event, next)` triples, as data both sides read.
2. On each wake the host starts one bounded run with the persisted state and a
   fresh observation as inputs.
3. The program decides in code from guards and Jev answers, and its `out`
   names the event and the next state.
4. The host checks the exact triple against its table before persisting it,
   and refuses anything else.
5. Effects the program performs happen before that check, so authorize every
   effect with a guard inside the program. The host check protects the stored
   state; it cannot undo an effect. Key effects by wake so a retried wake does
   not repeat them.

Do not wrap a one-event "resume" step in a Jevscript `machine` to restore a
persisted state. Every machine step sends a Jev request, even when only one
event is enabled. Branch on the persisted state in code and reserve a
`machine` for the states where several legal moves genuinely compete.

## Unsure between two places

- Between the host and Jevscript, ask what a replay of the run would lose if
  the logic moved to the host. If nothing, it belongs in the host.
- Between a unit and a separate program, ask whether a host would ever start it
  alone. If never, it is a unit.
- Between a `machine` and a durable host state machine, ask whether the state
  must survive the run ending. If yes, the host keeps it.
