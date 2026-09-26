# Jevscript error reference

This guide explains the stable codes defined by section 12 of the language specification. The specification is normative; this guide supplies repair advice. Diagnostic messages and marked source excerpts provide the source-specific details.

## Compile errors

| Code | What it means | Typical correction |
| --- | --- | --- |
| <a id="syntax"></a>`syntax` | The source does not match the grammar and no more specific code applies. | Start at the marked token, compare the statement with the grammar or nearest example, and correct missing punctuation, ordering, or indentation. |
| <a id="indent"></a>`indent` | Indentation contains a tab or is inconsistent within a block. | Replace tabs with spaces and align every statement in the same block to the same column. |
| <a id="unbounded_loop"></a>`unbounded_loop` | A `loop` or `until` has no `max`. | Add an explicit positive `max` appropriate to the operation. |
| <a id="pick_no_other"></a>`pick_no_other` | A `pick` has no escape label. | Add an `other` or `none` label describing the unmatched case. |
| <a id="pick_arity"></a>`pick_arity` | A `pick` has fewer than two or more than eight labels. | Add, combine, or remove labels until the answer space contains two through eight alternatives. |
| <a id="rate_arity"></a>`rate_arity` | A `rate` has fewer than two or more than ten levels. | Define two through ten ordered situations. |
| <a id="rate_bare_degree"></a>`rate_bare_degree` | A rate level is only a number or degree word. | Describe a concrete standalone situation instead of writing only `high`, `low`, or a number. |
| <a id="subject_not_path"></a>`subject_not_path` | A judgment subject is not a variable or field path. | Assign the expression to a variable first, then judge that variable or one of its fields. |
| <a id="judgment_side_effect"></a>`judgment_side_effect` | A judgment block contains an action, gate, or loop. | Move effects and control loops into a task; keep the judgment block to questions over its parameters. |
| <a id="def_side_effect"></a>`def_side_effect` | A def contains a capability call, gate, or pause. | Move the effect into a task or pass its already-obtained value into the def. |
| <a id="verb_unknown"></a>`verb_unknown` | The verb is not defined for the declared capability kind. | Use a verb from that kind's contract or declare a typed `tool` signature when the operation is host-defined. |
| <a id="verify_twice"></a>`verify_twice` | A task declares more than one verify loop. | Combine the conditions into the task's single top-level `verify`. |
| <a id="uppercase_identifier"></a>`uppercase_identifier` | An identifier contains an uppercase letter. | Rename it using lowercase letters, digits, and underscores. |
| <a id="unassigned_read"></a>`unassigned_read` | A variable can be read before any assignment reaches it. | Initialize it before the branch or assign it on every path that reaches the read. |
| <a id="use_not_found"></a>`use_not_found` | An imported file cannot be resolved, or the imported source cannot be loaded. | Correct the path and ensure the file is under the importer or a configured module search root. |
| <a id="use_cycle"></a>`use_cycle` | Imports form a cycle. | Move shared units into a third acyclic library and import that library from both sides. |
| <a id="use_has_inputs"></a>`use_has_inputs` | An imported file declares program inputs or outputs. | Extract reusable units into a library without `in` or `out`, and keep a thin runnable entry program. |
| <a id="use_needs_unmapped"></a>`use_needs_unmapped` | A library capability is missing from `with`, or its mapped kind differs. | Map every library capability to a same-kind capability declared by the importer. |
| <a id="use_private"></a>`use_private` | Code refers to another module's underscore-prefixed unit. | Use a public exported unit or remove the leading underscore if the unit is intentionally public. |
| <a id="main_has_params"></a>`main_has_params` | `task main` declares parameters. | Declare host-supplied values with top-level `in` and leave `main` parameterless. |
| <a id="verb_arity"></a>`verb_arity` | A typed tool call supplies the wrong number of positional arguments. | Match the declared tool signature, moving named values to their declared positions or removing extras. |
| <a id="machine_unknown_state"></a>`machine_unknown_state` | A machine event targets a state that is not declared. | Correct the target spelling or add the missing state. |
| <a id="machine_no_done"></a>`machine_no_done` | A machine has no terminal state. | Mark at least one state `done` and ensure intended paths can reach it. |
| <a id="event_no_description"></a>`event_no_description` | A machine event lacks its descriptive text. | Add a quoted description of the evidence for choosing that event. |
| <a id="machine_gate"></a>`machine_gate` | A machine action block contains `gate`. | Put risk on the event with `risky`, or perform gated work in a task called by the action. |
| <a id="pick_among_each"></a>`pick_among_each` | `pick among` is combined with `each`. | Choose one item from the list with `pick among`, or use a normal judgment over each item. |
| <a id="gate_args"></a>`gate_args` | A gate omits `risk` or `confidence`. | Supply both named arguments so the verdict is deterministic. |
| <a id="duplicate_name"></a>`duplicate_name` | A name or key is repeated in a scope that requires uniqueness. | Rename or remove the marked duplicate; also inspect the related first declaration when shown. |
| <a id="assign_immutable"></a>`assign_immutable` | Code assigns to an input or capability name. | Assign to a new local or declared output; inputs and bindings cannot be replaced. |
| <a id="loop_control_outside"></a>`loop_control_outside` | `break` or `continue` appears outside a loop. | Move it inside the intended loop or replace it with task/def control flow such as `return`. |
| <a id="log_level"></a>`log_level` | A `log` names a level other than `debug`, `info`, `warn` or `error`, or has no level at all (`log "x"`). | Write one of the four levels right after `log`, then the value: `log info "routed request"`. |
| <a id="log_in_question"></a>`log_in_question` | A `log` sits inside a judgment's question: its subject index, condition, labels, levels, detail or `pick among` question, which are built without effects. | Log the value on its own line before the judgment, or log the judgment's result after it; a `judgment` block may hold `log` lines between its results. |

## Compile warnings

| Code | What it means | Typical correction |
| --- | --- | --- |
| <a id="bare_prob_condition"></a>`bare_prob_condition` | A probability is used directly as a boolean condition. | Compare it with an explicit threshold so the policy is visible. |
| <a id="uncapped_field"></a>`uncapped_field` | A shaped or observed field may grow without a declared cap. | Add `max`, or shape the value into a bounded representation. |
| <a id="prelude_shadowed"></a>`prelude_shadowed` | A user unit hides an unqualified prelude def. | Rename the user unit or call the prelude explicitly through `std`. |
| <a id="machine_unreachable_done"></a>`machine_unreachable_done` | No declared path from part of a machine reaches a terminal state. | Add or retarget events so a terminal state is reachable, or remove the dead state. |
| <a id="detail_ignored"></a>`detail_ignored` | A question detail key has no effect in that position. | Remove it or move it to a judgment form that defines that detail. |
| <a id="log_shadowed"></a>`log_shadowed` | The file declares a unit named `log`, so `log <word> <value>` calls that unit and the `log` expression is unavailable in the file. | Rename the unit (and its calls) to write logs in this file; leave it to keep the file's existing meaning. |

## Runtime errors and machine escalation

| Code | What it means | Typical correction |
| --- | --- | --- |
| <a id="binding_missing"></a>`binding_missing` | A required capability has no host binding. | Bind every name declared by `needs` before starting the run. |
| <a id="state_too_large"></a>`state_too_large` | The assembled judgment state exceeds its resolved model profile. | Shape or focus large fields, reduce the request group, or choose an appropriate supported profile. |
| <a id="jev_unavailable"></a>`jev_unavailable` | The judgment service is temporarily unavailable or rate-limited. | Retry with backoff when permitted; inspect service availability and credentials if failures persist. |
| <a id="jev_rejected"></a>`jev_rejected` | The judgment service rejected the request as non-retryable. | Use the returned provider message to correct credentials, model selection, or request/profile incompatibility. |
| <a id="verb_missing"></a>`verb_missing` | A bound tool does not implement a called verb. | Bind an adapter whose manifest implements the verb, or correct the program's verb name. |
| <a id="adapter_error"></a>`adapter_error` | A host adapter failed. | Inspect the adapter's returned message and logs; correct its inputs or environment, or retry only when the pause says it is retryable. |
| <a id="type_error"></a>`type_error` | A runtime value has the wrong type for the operation. | Inspect the marked expression and convert or shape the value explicitly. |
| <a id="recursion_limit"></a>`recursion_limit` | Def recursion exceeded the runtime limit. | Add a terminating case, replace deep recursion with a bounded loop, or reduce the input depth. |
| <a id="replay_diverged"></a>`replay_diverged` | Replay no longer matches the recorded execution identity or event sequence. | Replay with the exact recorded program/profile metadata and unmodified complete recording; start a live run for changed inputs or code. |
| <a id="pick_too_many"></a>`pick_too_many` | A dynamic `pick among` list exceeds the resolved profile's criteria cap. | Filter, rank, or batch candidates before asking the judgment. |
| <a id="profile_missing"></a>`profile_missing` | The selected model profile is absent or unusable. | Install or select a valid profile with a supported tokenizer and all required fields. |
| <a id="no_enabled_events"></a>`no_enabled_events` | A machine state has no event whose guard is currently true. | Correct the guards or state design, or handle the resulting escalation in the host. |

For failures caused by external services or adapters, the reference gives the program-side repair path. Provider and adapter logs may contain additional operational details, but they do not replace the stable Jevscript code.
