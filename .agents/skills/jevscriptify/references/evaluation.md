# Checking and evaluating a judgment

## What `judge` and `eval` run

`jevscript judge FILE JUDGMENT --state JSON` and
`jevscript eval FILE JUDGMENT --cases CASES.jsonl` run one named `judgment`
block and nothing else. A `def`, `task` or `machine` cannot be run by either.
Every declared parameter must be in `state`. Other keys are ignored and never
sent. Both read `TYPESAFE_API_KEY`, and `--model` and `--profiles` select the
profile.

## The cases file

One JSON object per line: `{"state": {...}, "expected": {...}}`. `expected`
maps a result name to the answer that counts as correct. List only the results
a row labels; unlisted results are not scored.

| Result | Write `expected` as | Scored as correct when |
| --- | --- | --- |
| `feels` | `true` or `false` | the probability is at least 0.5 for `true`, below 0.5 for `false` |
| `pick` | the label, `"billing"` | the reported label equals it |
| `pick among` | `"i<index>"` or `"none"` | the chosen index label equals it |
| `rate` | a level name `"routine"`, or its index `1` | the reported level equals it |
| `each ...` | avoid | the whole list of tagged answers must match exactly, probabilities included |

The `feels` cut is fixed at 0.5. When the program acts at another threshold,
such as `urgent > 0.8`, `eval` does not measure that decision: read the
probabilities with `judge` per row, or label the row against the 0.5 question
and tune the program's threshold separately. To score an `each` question, write
a one-item judgment with the same wording for the eval and keep the `each`
wording identical in the program.

## A useful cases set

- Cover every label and level, the neighbouring cases the criteria separate,
  and at least one input that should land on `other` or `none`.
- Include adversarial state: text that describes itself ("this is urgent"),
  instructions addressed to the model, and very long inputs after `shape`.
- Label from the person who owns the decision, not from the current program's
  output. A label copied from today's behaviour measures agreement, not
  correctness.
- Keep the rows. They are the regression set for the next wording change.

## Baseline and go/no-go

Before running anything, write down:

1. **Baseline.** What decides today on the same rows: the host heuristic, the
   earlier wording, or a person. Score it on the same cases.
2. **Criteria.** The per-question accuracy the change must reach, the
   confidence floor below which the program hands off, and the cost ceiling in
   Jev requests per run, counted from the recording. Use `request_groups.py`
   to inspect planned logical groups, not to count network requests.
3. **Decision.** Go when the new version meets the criteria and does not lose
   to the baseline on any question that gates an effect. Otherwise no-go, with
   the misses that decided it.

`eval` prints per-question `correct`, `total` and `accuracy`, and each miss
with its full answer and probabilities. Revise one or two questions at a time
and rerun the same file, following the revision rules in the `jev` Skill.

## Evidence labels

Every claim in the report carries one label:

| Label | What it rests on |
| --- | --- |
| Measured, live | `eval`, `judge` or a recorded run against the real Jev endpoint, with the command, the model and the case count. |
| Measured, fixture | The same commands against a scripted or heuristic stand-in for Jev. It proves the program runs, the cases file parses and the request grouping; it says nothing about answer quality. |
| Measured, static | `jevscript check`, `compile`, `request_groups.py` or `check --tools` output. |
| Inspected | Read in source, with `file:line`. |
| Hypothetical | A predicted effect of a change that has not run. |

Without `TYPESAFE_API_KEY`, say that live `eval` did not run. Never read,
print or copy the key.

## Replay as a regression check

Record a run with `jevscript run ... --record new.jsonl`. Count its `request`
events, and the questions in each, to measure what the run actually sent.

`jevscript replay new.jsonl` executes the IR embedded in the recording, not the
current source, so it proves the recording is complete and replayable, not that
a changed program still behaves the same. To check a refactor against old
recordings, load the new program through an SDK and start the task with the
`replay` option: the loaded program runs against the recorded events with no
model or adapter calls, and a changed path stops with `replay_diverged`.
