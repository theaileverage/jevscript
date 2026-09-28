---
name: jevscriptify
description: Find the decisions in host code and `.jev` programs that belong in Jevscript, place each one, and turn the chosen ones into checked and evaluated Jevscript. Use when asked to jevscriptify code, look for Jev opportunities, decide whether logic belongs in the host, in a unit of a `.jev` program, in a separate `.jev` program or in a durable host state machine driven by Jevscript, audit a Jevscript host such as the Chief of Staff example, or evaluate a judgment against labelled cases.
---

# Jevscriptify

This Skill is the procedure. Three other sources own the details:

- `spec/jevscript-language-specification.md` decides the language. When this
  Skill disagrees with it, the spec wins.
- The `jev` Skill (`.agents/skills/jev/SKILL.md`) owns question design: one
  property per question, primitives, criteria, state and revision.
- The `jevscript` Skill (`skills/jevscript/SKILL.md`) owns the commands:
  `check`, `compile`, `run`, `replay`, `judge` and `eval`.

Work from a checkout with a built CLI (`cargo build`, then
`target/debug/jevscript`, or set `JEVSCRIPT_BIN`). Record the commit and
`jevscript --version` before the first step.

## Procedure

Copy these steps into the task list and mark each done or skipped with a reason.

1. Inventory the decision points.
2. Classify each one.
3. Place each one.
4. Decompose the judgments.
5. Write the Jevscript and check it.
6. Bind the host.
7. Build the cases and the baseline.
8. Evaluate and decide go or no-go.
9. Report, separating measured, inspected and hypothetical evidence.

Stop after step 3 when the request is an audit. Change code only for proposals
the user approved.

### 1. Inventory the decision points

A decision point is any place where what happens next depends on something
other than plain facts. List each with `file:line`:

- text written by an agent, a person or a model that steers control flow
  through a regex, keyword list, prefix match or length check;
- a threshold, ranking or ordering over a model estimate;
- a generation whose output is parsed and then branched on;
- every existing judgment, and every guard that authorizes an effect;
- a copy in the host of a program's vocabulary, such as its labels or its
  machine event names, that the host branches on;
- consecutive Jev requests on one path whose questions do not depend on each
  other.

In host code, start from a search and then read each hit:

```sh
grep -nE 're\.(match|search|fullmatch)|startswith\(|\.lower\(\)|in \(|[<>]=? ?0\.[0-9]' <host files>
```

For each `.jev` file, list the questions and what separates them:

```sh
python3 .agents/skills/jevscriptify/scripts/request_groups.py path/to/file.jev
```

The script prints every question with its compiled `request_group`, and the
unit calls, capability calls and `llm` generations between them. Questions with
the same group share one request and one state. Save its output as evidence.

### 2. Classify each one

Name what should make the decision:

| Class | Fits when |
| --- | --- |
| Jev | A knowledgeable person answers it in a second from the text: a yes or no, one of known options, a position on a described spectrum, or one of a program-built list. |
| Code | Facts decide it: comparisons, counts, dates, arithmetic, set membership, thresholds over Jev answers. |
| `llm` generation | The output is new text a person or agent reads. Never branch on its content without a judgment. |
| Agent | Multi-step work in the world. |
| Person | Authority, taste or information only the owner has. |

Split a decision that mixes classes. Jev estimates, code decides and a person
authorizes.

### 3. Place each one

Read `references/placement.md` and apply it. The outcome for each decision
point is one of:

- host code;
- a unit in an existing `.jev` program: `judgment`, `def`, `task` or `machine`;
- a separate `.jev` program with its own entry point, contract and recording;
- a durable state machine the host persists, whose transitions a `.jev`
  program decides and the host validates against a closed table.

A decision earns Jevscript through judgment, guard or recording value. Anything
else stays in the host, which also stays the trusted boundary for credentials,
effects and validation.

### 4. Decompose the judgments

Follow the `jev` Skill's workflow for each question, then apply the Jevscript
forms:

| Need | Jevscript form |
| --- | --- |
| Yes or no, thresholded in code | `feels`, with `yes` and `no` examples when the boundary is subtle |
| One of known options | `pick` with 2 to 8 labels and a bare `other` or `none` |
| A degree | `rate` with levels written as situations, never numbers or "low", "high" |
| One of a runtime list | `pick among ... by <field>`, with `none` when nothing may fit; its block takes no detail keys, so to put the text being matched into the state, ask it inside a `judgment` whose parameters include that text |
| The same question per item | `each <list> ...`, then count or filter in code |
| A long text | `shape` with `max` caps, or `focus ... on "<purpose>"` |
| Questions to measure alone | a named `judgment` block, called by the `def` or `task` |
| Several questions on one state | consecutive inline judgments, or one `judgment` block, so they share a request |

Ask every question a path might need in one request (speculative fan-out), and
leave a second request only for a question that depends on an earlier answer.
A call to a named unit, a capability call, a gate or a pause closes the open
request group, so reorder or merge before accepting an extra request. Keep
thresholds and weights in a policy input, never in question text.

### 5. Write the Jevscript and check it

1. Write the planned request groups down first: which questions share a
   request and how many requests the main path sends.
2. `jevscript check file.jev` must report no errors. Read every warning.
3. Run `request_groups.py` and compare the groups with the plan. A mismatch is
   a finding to fix, not a note.
4. When the program runs, record it and count the `request` events.

### 6. Bind the host

- Declare `in` with the fields the program reads and `out` for what the host
  consumes. Pass policy numbers as input.
- Declare each `tool` with a signature block, give the host adapter a
  manifest, and run `jevscript check file.jev --tools manifest.json`.
- Treat the program's `out` as data at the host boundary: check names, ids and
  next states against the host's own registry or table before acting on them.
- Pin the shape hashes of judgments the host reads, and treat a label or level
  rename as a breaking change.
- Keep agent-written text as data. It reaches control flow only through a
  judgment with a declared answer space.

### 7. Build the cases and the baseline

Read `references/evaluation.md`. Write a `--cases` file per judgment with
labels from the person who owns the decision, and name the baseline: what
decides today on the same rows.

### 8. Evaluate and decide go or no-go

Write the go/no-go criteria before running anything. Run
`jevscript eval file.jev <judgment> --cases cases.jsonl` when
`TYPESAFE_API_KEY` is set. Without it, state that live evaluation did not run;
a run against a stand-in for Jev checks mechanics only. Never print or copy the
key.

### 9. Report

Write the report with these parts, in this order:

1. **Scope.** Commit, files, CLI version, and what was not examined.
2. **Placement inventory.** One row per decision point: `file:line`, what it
   decides, current class and place, proposed class and place, and the
   evidence label.
3. **Ranked proposals.** Rank by the risk of the current behaviour first (agent
   text steering an effect, an unguarded effect), then measured cost on a hot
   path, then duplicated vocabulary or dead weight, then effort. Each proposal
   names its `file:line`, the change, the expected effect and how to check it.
4. **Evidence**, in three separate lists:
   - **Measured**, split into live, fixture and static;
   - **Inspected**, read in source with `file:line`;
   - **Hypothetical**, predictions that have not run.

Proposals stay proposals until the owner approves them.

## Additional resources

- `references/placement.md`: the placement rule, the four places, unit
  choice and the durable state machine contract.
- `references/evaluation.md`: what `judge` and `eval` run, the cases format,
  baselines, go/no-go and evidence labels.
- `scripts/request_groups.py`: questions and request groups from compiled IR.
