# Jevscript Language Specification

Version 0.1, draft. 2026-09-21.

Jevscript is a small language for control loops that run agents and language models under the judgment of Jev, TypeSafe's System One model. Jev reads one structured state and answers typed questions with probabilities. It does not generate text and it does not count. Jevscript programs own the loop, the thresholds, the budgets and the policy. Jev owns the snap judgments. Text models and terminal agents are capabilities the host binds at run time.

This document is the authority for the 0.1 language, runtime model, and host contract. It is written so that a compiler, a runtime, and two host SDKs can be built from it independently. The [document-boundary evaluation](../docs/specification-boundaries.md) identifies which material belongs in separate normative contracts in a future documentation split; until that migration is made atomically, linked guides and reference documents are explanatory and this file remains the sole normative authority.

The reference implementation is written in Rust: the compiler, the IR, the runtime and the command-line tool are one Cargo workspace. Host SDKs for JavaScript and Python drive the Rust runtime over the boundary in section 11.5. The language does not depend on this choice; any implementation that meets section 15 conforms.

Source files use the extension `.jev`.

---

## 1. Design principles

1. **Jev judges, code decides.** Every branch, threshold, weight and count is written in Jevscript. Here *code* means expressions and statements the runtime evaluates: comparisons, guards, arithmetic, loops, gates and thresholds. Agent output and generated text are data; a model estimate is a judgment result. Code decides how those values affect control flow. For example, `risk = summary feels "requires approval"` obtains an estimate, while `if risk > 0.7:` states the program's decision rule. Every judgment has a declared answer space before its request: `feels` returns a probability; `pick` and `rate` use their declared labels or levels; `pick among` and machines construct their permitted options from program data and guards immediately before asking. The model cannot add an option or branch.
2. **The context window is the constraint.** Jev's state plus all questions share a fixed token budget. The language makes state shaping the default path, not a repair step. Programs declare what Jev sees and how much of it.
3. **Programs are values the host drives.** A run is a state machine with typed pause points. The host steps it, answers it, and can replay it. Nothing inside a program reaches the world except through a declared capability.
4. **Code owns the completion predicate.** An agent's claim and Jev's estimate are signals. The runtime evaluates the program's declared `verify` predicate at the task boundary and reports whether it held. This flag describes that predicate's result; it does not by itself establish that the external goal is objectively complete or that the predicate's inputs were independent of a model. Section 7.4 defines the exact task rule; section 7.8 defines the separate machine rule.
5. **Everything is recorded.** Every judgment, generation, capability result and random draw is written to a recording. A recording replays with zero model calls and identical control flow.
6. **Few keywords, no punctuation tax.** Blocks are indented. Calls read as commands. Strings interpolate. A program should read top to bottom by someone who has never seen the language.

---

## 2. Lexical structure

### 2.1 Encoding and lines

Source is UTF-8. Lines end with LF. A statement ends at the end of its line unless the line opens a block with `:` or the line ends inside an open bracket `(`, `[`, `{`, in which case the statement continues on the next line.

### 2.2 Indentation

Blocks are delimited by indentation, as in Python. A line that ends with `:` opens a block. The block is every following line indented deeper than the opening line, until the first line at the opening line's indentation or shallower. Indentation uses spaces. Tabs are a compile error. A block's indentation must be consistent within itself; the compiler does not require a fixed width.

### 2.3 Comments

`#` starts a comment that runs to the end of the line.

### 2.4 Identifiers

An identifier matches `[a-z_][a-z0-9_]*`. Identifiers are lowercase by convention and by rule: an uppercase letter in an identifier is a compile error. This keeps labels that reach Jev, state paths and host-visible names in one stable form.

### 2.5 Keywords

The reserved words are:

```
program  use  in  out  needs
judgment task  def  machine  return
feels  pick  rate  each  other  among  on
shape  focus  trail  max
state  when  stay  initial
until  loop  for  if  elif  else  and  or  not  is
continue  break  stop  escalate
gate  proceed  confirm  budget
true  false  none
```

`as`, `with`, `using`, `above`, `below`, `by`, `prompt`, `done`, `risky`, `sample`, `goal`, `observe`, `strict`, `tail`, `log` and the four log levels `debug`, `info`, `warn` and `error` are contextual words. A contextual word has special grammar only in the positions listed after the grammar in section 13; elsewhere it is an ordinary identifier. For example, `using = "notes"` is a valid assignment, while `writer.write "summarize" using notes` reads `using` as part of that call. Unlike a reserved word, a contextual word may therefore name a variable, parameter, field or label when it is outside its special position. Accepting a reserved word specifically as a field or named argument (for example, `dev.stop` or `spawn in tree`) is a different grammar exception.

### 2.6 Literals

| Literal | Examples | Type |
| --- | --- | --- |
| number | `3`, `0.75`, `2k` | number. `k` multiplies by 1000. |
| percent | `80%` | number, equal to `0.8`. Sugar for thresholds. |
| text | `"stop"`, `"line\n"`, `"""multi\nline"""` | text |
| boolean | `true`, `false` | bool |
| none | `none` | none |
| list | `[1, 2, 3]`, `[]` | list |
| record | `{ title: "x", body: t }`, `{ title, body }` | record. `{ title }` is shorthand for `{ title: title }`. |

A triple-quoted text literal may span physical lines. Its newline characters are part of the value, and escapes and interpolation work as they do in a one-line text literal:

```
message = """Review {file}.
Return one finding per line."""
```

The literal preserves every space on continuation lines, even inside an indented block; no common indentation is removed. For example, the text below starts with `First`, contains a newline and two spaces, then `Second`:

```
task main:
  message = """First
  Second"""
```

Those two spaces are text, not a new Jevscript block.

### 2.7 Text interpolation

Inside a text literal, `{expr}` evaluates `expr` and inserts its text form. A literal brace is written `{{` or `}}`. Interpolation is evaluated by the runtime before any value is sent anywhere; Jev, LLMs and adapters only ever see the finished text. Interpolation does not parse text back into the original type: `"{count}"` is text even when `count` is a number. Use `number(text_value)` for an explicit conversion; it accepts numeric text such as `"12.5"` and raises `type_error` when the text is not a number.

Conversion does not make agent-written text trustworthy. If a program calls `number(agent_text)` and uses the result as a threshold or count, that is an explicit program decision to let the agent-supplied value influence control flow; section 9.1's caution still applies.

---

## 3. Program structure

A source file holds exactly one program.

```
program <name>

use "<path>" as <name> with <mapping>   # zero or more
in  <name>: <shape>                     # zero or more
out <name>                              # zero or more
needs <name>: <kind>                    # zero or more

<judgment | task | def | machine>*      # one or more; exactly one task named main if the program is runnable
```

At the top level, `in` declares data and `needs` declares authority:

| Declaration | Host supplies | Program receives |
| --- | --- | --- |
| `in issue: record` | An input value when a run starts. | Immutable data; a record describing a tool cannot call that tool. |
| `needs tree: tool` | A binding to a host adapter. | The ability to call the adapter's permitted verbs (section 9). |

The word `in` inside `for item in items` is the loop separator; `spawn in tree` is the `spawn` verb's named argument. Neither is an input declaration.

### 3.1 `program`

Names the program. The name is what hosts load by. A program with no `task main` is a library of judgments and defs and can still be loaded; its judgments can be run individually.

### 3.2 `in`

Declares an input the host must supply when it starts a task. The shape is one of:

- a bare type: `text`, `number`, `bool`, `list`, `record`
- a record shape listing field names: `{ title, body, branch }`, optionally typed: `{ title: text, count: number }`

Inputs are readable anywhere in the program. They are immutable.

### 3.3 `out`

Declares an output. A task assigns it. When the task finishes, the run's `done` pause carries every declared output. An output that was never assigned is `none`.

### 3.4 `needs`

Declares a capability the host binds. The kind decides which verbs the compiler accepts on it (section 9). A program whose capability is not bound at start fails to start with a `binding_missing` error. Capability names are program-scoped values and cannot be reassigned.

A capability is a declared name and kind paired with a host-provided adapter, rather than a function discovered from input text. In this example the program can call `tree.tests_pass` only because it declares `tree`; the host binds that name to an adapter that implements the verb:

```
program test_check
needs tree: tool:
  tests_pass() -> bool

task main:
  passing = tree.tests_pass
```

The exact adapter contract is in section 9. A supplied `in tree: record` would provide data but no authority to run tests.

### 3.5 `judgment`

A named block of judgments over declared state fields. Its body cannot call capabilities or loop, but executing the block does issue a model request (or the limited `each` chunks of section 6.6). Section 6.

### 3.6 `task`

A named block that can observe, judge, gate and act. A task runs until it returns, hits a terminal pause, or exhausts a budget. Section 7.

### 3.7 `def`

A named helper without capability calls or gates. It may contain judgments and recorded reads such as `now()` or `random()`, so calls to it are not necessarily referentially transparent. Each `def` invocation that contains judgments issues one request per batch (section 6.6). Section 8.

### 3.8 `use`

Imports another `.jev` file as a module. Section 3.9.

### 3.8a `machine`

A named state machine whose transitions Jev chooses among, one step at a time. Section 7.8.

### 3.9 Modules

A large system is built from many programs. Jevscript composes them with files, not with a package manager: one `.jev` file is one module, and `use` brings another file's units into scope under a name.

```
use "./lib/agent_loop.jev" as harness with claude, tree
use "../shared/triage.jev" as triage
```

**Paths.** The path is a text literal, relative to the importing file. The host may add search roots (SDK `load` option `paths`, or the `JEVSCRIPT_PATH` environment variable, colon-separated); a path that does not start with `./` or `../` is resolved against those roots in order. Search roots are supplied when the program is loaded and linked, before its metadata is returned. Starting a task uses that linked program without recompiling it. There is no registry and no version syntax in 0.1.

A module alias is an ordinary identifier and cannot be a reserved word; `as loop` is a compile error because `loop` is a keyword.

**What a module exports.** Every `judgment`, `task`, `def` and `machine` whose name does not start with `_` is exported. A name starting with `_` is private to its file, and referring to it from another file is a compile error (`use_private`). Exported units are referred to as `<alias>.<name>`:

```
j = harness.read_agent obs
if harness.stuck(obs.recent): ...
harness.watch(dev, tree)
```

**Library files.** A file that is imported may declare `needs` but not `in` or `out`. A file with `in` or `out` is a program, and importing it is a compile error (`use_has_inputs`). Libraries therefore pass everything through parameters, which is why tasks take parameters (section 7).

**Capabilities across modules.** A library's `needs` are satisfied by the importer, explicitly, in the `with` clause. `with claude, tree` binds the library's `claude` and `tree` to the importer's capabilities of the same names. `with dev: claude` binds the library's `dev` to the importer's `claude`. Every `needs` in the library must be mapped, or the import is a compile error (`use_needs_unmapped`). Kinds must match. The mapping is static, so the compiler checks verbs against kinds across the module boundary exactly as it does within a file. Nothing is bound implicitly: a library cannot reach a capability it was not handed.

**Budgets and thresholds.** A library task runs under the calling task's remaining budget. It uses its own declared thresholds for its own gates. A library task that pauses (`confirm`, `escalate`, `waiting`) pauses the whole run; the pause's `task` field carries the qualified name (`harness.watch`) so the host can tell where it came from.

**Batching.** A call to a library `judgment` is one request, as always. Judgments inside a library `def` batch by the rules in section 6.6 within that def. Batching never crosses a call boundary in 0.1.

**Cycles** are a compile error (`use_cycle`). A file that cannot be found or does not parse reports `use_not_found` or the callee's own diagnostics with their file names.

**Linking.** The compiler produces one IR document for the root program with every transitively used module inlined and every unit name qualified (`harness.read_agent`). The runtime and the host SDKs see one flat program. `Program.judgments` lists qualified names, `Program.judgment("harness.read_agent").run(state)` works, and each judgment's shape hash (section 11.4) is computed on the unit's own name and answer space, never on its alias, so moving a judgment between files without changing it keeps its hash.

**The prelude** is the implicit module `std`. Its exported names (`stuck`, `repeats`, `focus_impl`) are in scope unqualified and also as `std.stuck`. A user definition with the same name shadows the unqualified form and the compiler warns.

---

## 4. Types and values

Jevscript is dynamically typed with a small fixed set of types. The compiler checks what it can from declarations; the runtime checks the rest and raises `type_error` on mismatch.

| Type | Description |
| --- | --- |
| `text` | Immutable Unicode string. |
| `number` | IEEE 754 double-precision floating-point number. There is no integer/float distinction. |
| `bool` | `true` or `false`. |
| `none` | The absent value. |
| `list` | Ordered sequence of values. |
| `record` | Ordered map from identifier to value. |
| `prob` | A `number` constrained to the inclusive range `[0, 1]`, produced by `feels`. It uses the same floating-point representation and behaves as a number in arithmetic and comparison; its lossless host JSON form retains the `prob` tag. |
| `choice` | The result of `pick`. Fields: `label: text`, `confidence: prob`, `probabilities: record`; `pick among` also supplies `index` and `item`. |
| `level` | The result of `rate`. Fields: `level: number`, `score: number`, `normalized: number`, `confidence: prob`, `probabilities: record`. |
| `handle` | An opaque reference to something an adapter owns, such as a spawned agent or a worktree. |
| `pause_result` | What a `confirm` pause returned from the host. Fields: `answer`, `text`. |

The numeric range of a `prob` describes a well-formed answer, not empirical calibration: `0.8` does not certify that eight of ten similar real-world claims are true. A judgment adapter and runtime must preserve this range at their boundary. Likewise, a `choice` or `level` confidence describes the answer distribution's concentration under the selected model's 0.1 interpretation; it is not the probability that the selected answer is correct.

Values have two JSON representations. In plain state JSON, a `prob` is a number and a `choice` or `level` looks like its fields below; this is the representation the judgment model sees. Across the host boundary, a structured value carries a `$jev` discriminator so it survives a round trip as its original type. For example, a `prob` is `{"$jev":"prob","value":0.8}` and a `choice` begins `{"$jev":"choice","label":"billing",...}`. A plain JSON object without `$jev` is a Jevscript `record`, even when it happens to have fields named `label` and `confidence`. The host form of a `level` also carries its declared `names` when present. Nested structured values retain their tags in host JSON.

### 4.1 Truthiness

`false`, `none`, `0`, `""`, `[]` and `{}` are false. Everything else is true. A `prob` is a number, so `if p:` tests `p != 0`, which is almost never what a program means. The compiler warns on a bare `prob` in a condition; write `p > 0.7`.

### 4.2 Text form

Every value has a text form used by interpolation and by capability calls that take text. Numbers print without trailing zeros. Lists and records print as JSON. `choice` prints its label. `level` prints its level index. `none` prints as empty text.

### 4.3 Equality and `is`

`==` compares by value. `is` compares a `choice` against a label name without quotes: `j.next is stuck` is `j.next.label == "stuck"`. `is` on a `level` compares the selected index with a declared level name if the levels were named. It is label matching, not object identity; a misspelled label does not match.

For example:

```
route = message pick:
  billing "about an invoice, charge or refund"
  account "about sign-in or account access"
  other

if route is billing:                 # same test as route.label == "billing"
  queue = "finance"

progress = summary rate:
  unchanged "the work has not moved forward"
  advancing "the work is moving toward completion"
if progress is advancing:            # compares the named level, not its score
  status = "moving"
```

---

## 5. Expressions and statements

### 5.1 Operators

Precedence, lowest to highest:

```
or
and
not
== != < <= > >= is
+ -
* / %
unary -
call, field access, index
```

`+` on two texts concatenates. `+` on two lists concatenates. Mixed text and number in `+` is a type error; interpolate instead.

Examples of the highest-precedence expression forms are `-cost` (unary `-`), `review(issue)` (call), `issue.title` (field access), and `files[0]` (index). They may compose: `-report.scores[0]` negates the first element of the `scores` field.

### 5.2 Calls

A call is written in command form or function form. The two are equivalent.

```
tree.create issue.branch                     # command form
tree.create(issue.branch)                    # function form
dev = claude.spawn in tree, prompt issue.body
dev = claude.spawn(in: tree, prompt: issue.body)
```

In command form, arguments are separated by commas. A named argument is written `name value`. In function form, a named argument is `name: value`. Positional arguments come before named ones. When a command-form call has no arguments and reads as a property (`dev.observe`, `tree.tests_pass`), it is still a call; the compiler treats a trailing identifier on a capability or handle as a zero-argument call.

Command form is permitted only as a whole statement or as the right-hand side of an assignment. Inside a larger expression, use function form.

### 5.3 Assignment

```
x = expr
x.field = expr        # only on records held in variables, not on inputs or capability results
```

There is no index assignment (`xs[i] = ...`). Lists and records are built whole, with literals, comprehensions, and `+`. Variables are block-scoped. Assignment to an outer variable from an inner block updates the outer variable. Reading a variable before assignment is a compile error.

The current collection operations are value building, not mutating methods: use `[item for item in items if condition]` to filter, `left + right` to concatenate lists, `{ title: title, body: body }` to construct a record, and the builtins in section 5.7 for access, ranking and aggregation. There is no general `record.merge(...)` or `list.append(...)` method.

### 5.4 Conditionals

```
if cond:
  ...
elif cond:
  ...
else:
  ...
```

`cond` is an ordinary expression. A previously assigned judgment result may drive it with an explicit label test or numeric threshold:

```
risk = summary feels "requires approval"
if risk > 0.7:
  escalate "Approval is required"
```

A named unit call may appear in a condition in function form, for example `if classify(message).risk > 0.7:`. Inline judgment syntax is an assignment form, so `if summary feels "done":` is not a condition. There is no conditional or ternary expression; use an `if` block to assign the selected value.

### 5.5 Loops

```
for item in items:            # iterate a list
  ...

loop max 5:                   # fixed upper bound, no condition
  ...

until cond, max 5:            # run body until cond is true, at most 5 times
  ...
```

Every loop has a compile-time upper bound. `for` is bounded by its list. `loop` and `until` require `max`. There is no unbounded loop in the language. `continue` and `break` behave as in Python.

`until cond, max N` evaluates `cond` before each iteration and once more after the last. If `cond` is false after `N` iterations, the loop ends and execution continues; a task that needs the condition to be true must check it afterwards or use `verify` (section 7.4).

### 5.6 Terminating statements

```
return expr?          # leave the current def or task
stop "reason"         # end the run with a stopped pause
escalate "reason"     # end the run with an escalate pause
```

| Form and location | Consumer |
| --- | --- |
| `return expr` in a `def` | The Jevscript caller receives the expression value. |
| `return expr` in a task called by another unit | The Jevscript caller receives the expression value. |
| `return expr` in a host-started task, including `main` | The task ends. Its expression value is not in the `done` pause; declared `out` values are the host-visible outputs. |
| `stop "reason"` | The host receives `stopped.reason`; the run is terminal. |
| `escalate "reason"` | The host receives `escalate.reason` and may explicitly resume the run. |

Falling off the end of a def or called task returns `none`. A bare `return` does the same. A host-started task emits `done` whether it returns or falls off the end, subject to its final verification and budget checks.

### 5.7 Builtin functions

These are always available. Most compute from their arguments, but `now()` reads the clock and `random()` draws from a pseudorandom source. Those two reads are recorded. Neither their results nor a def that invokes them may be freely reordered or memoized across calls.

| Function | Meaning |
| --- | --- |
| `len(x)` | Length of a list or text. |
| `count(probs, above p)` | Number of entries in a list of probs strictly above `p`. Also `below p`. |
| `max(xs)`, `min(xs)` | Over a list of numbers. Empty list gives `none`. |
| `sum(xs)` | Sum of a list of numbers. |
| `top(items, by probs, n k)` | The `k` items whose paired prob is highest, in descending order. |
| `join(texts, sep)` | Concatenate texts with a separator. Default separator is a newline. |
| `split(text, sep)` | Split text on a separator into a list. |
| `lines(text)` | Split on newlines. |
| `head(text, n)`, `tail(text, n)` | First or last `n` characters. |
| `tokens(x)` | The runtime's token estimate for a value's text form. |
| `chunk(text, tokens n)` | Split text into pieces of at most `n` tokens on line boundaries. |
| `hash(x)` | A stable short hash of a value's text form. |
| `now()` | Current time as ISO text. Recorded and replayed. |
| `random()` | A uniformly distributed number greater than or equal to `0` and strictly less than `1` (the interval `[0, 1)`). The draw is recorded, and replay returns that exact value without drawing again. |
| `zip(xs, ys)` | A list of two-element lists pairing `xs` and `ys`, as long as the shorter. |
| `keys(r)`, `values(r)`, `items(r)` | Record access as lists. |
| `text(x)`, `number(x)`, `bool(x)` | Conversions. |

An explicit sampling seed reproduces the pseudorandom draws. It does not reproduce live model answers, adapter effects or host replies; replay uses the complete event recording in section 10.4. `now()` uses the same recorded `draw` event family as `random()` and sampling, with its own kind.

### 5.8 `log`

```
log <level> <expr>                                  # level: debug | info | warn | error
log <level> <expr> { <field>, <field>: <expr>, ... }
```

`log` records one line in the run's recording and evaluates to the value of its expression. It is an expression, so it can wrap any value inline, and it is also a statement when written on its own line:

```
log info "routed request" { owner, confidence: c.confidence }
x = log debug classify(message)     # logs the record's text form and assigns the record
if log warn risk > 0.7:             # logs the comparison, then tests it
  escalate "risky"
```

**Form.** The level is one of `debug`, `info`, `warn` and `error`, lowest first; any other word in that position is `log_level`. The expression after the level is the logged value, and its text form (section 4.2) is the line's message. There is no separate message slot: `log info "routed request"` logs that text, and `log debug classify(message)` logs the text form of the record it returns. An optional record literal (section 2.6) directly after the value holds structured fields. Field names are unique in one log (`duplicate_name`), `{ owner }` is shorthand for `{ owner: owner }`, and field values may be any value. The value is evaluated first, then the fields in written order.

**Extent.** The logged expression extends as far to the right as an expression can, as the text of `focus` does: `log debug a + b` logs `a + b`, and `1 + log debug a * 2` adds `1` to the logged `a * 2`. It ends where an expression cannot continue: at `,`, `)`, `]`, `:`, the end of the line, or a record literal, which is the log's fields. Parenthesise to log part of an expression: `(log debug a) + b`. Command form (section 5.2) is not an expression, so a logged call is written in function form: `log debug tree.create(branch)`.

**A contextual word.** `log` is special only where an expression or statement begins and is followed by a word and then the start of an expression. Everywhere else it is an ordinary identifier: `log = 3`, `log.count`, `f(log)` and `f(log: 1)` keep their meaning. A file that declares a judgment, task, def or machine named `log` has no `log` expression at all: there, including in its text interpolations, `log info x` stays a command-form call to that unit with `info` bound to `x`, as it was before this form existed, and the compiler warns `log_shadowed` to suggest a rename. A unit named `log` in another module is reached only as `<alias>.log`, which never starts a log, so it does not affect the importer.

**Where.** A `log` is allowed in every unit, because it has no effect. In a `judgment` block (section 6.7) a `log` line may sit among the result lines. The block is still one request over its parameters; its log lines run once the answers have arrived, in written order, and each reads the parameters, the inputs and the results written above it. A `log` cannot appear inside a judgment expression: its subject's index, condition, labels, levels, detail or `pick among` question. A question is built without effects, so this is `log_in_question`. A machine's `goal`, event descriptions, guards, `observe` fields and actions are ordinary expressions and statements and may log.

**No effect.** A `log` never changes control flow except through the value it returns, which is its expression's value unchanged. It never reaches Jev: the log line is recorded, not sent, and is never part of a request's state or questions. The logged value flows on as ordinary data, subject to section 6.9 like any other. A `log` is not a model request, a capability call, a step record (section 7.5) or a pause. It does not count against `calls`, `usd` or `steps`, and it is not a batching boundary (section 6.6) unless its own expression calls a unit or capability. It cannot fail on its own: text forms and host JSON are total, so only evaluating the value or a field can raise an error.

**Recording.** Each evaluated `log` writes one `log` event (section 10.3) with its `level`, `message`, `fields` and `source`, and the `task` a pause raised at that point would carry. That is the task name, `machine:<name>` inside a machine, or the calling task for a def or named judgment. The event is streamed to the host with every other event (`Run.events()`, section 11.2), and each SDK offers a log callback and a `logs()` stream built from it. A judgment run alone (section 11.3) has no recording; it returns its log lines with its answers, named after the judgment. Its log lines may evaluate everything a log line in a run may: calls to defs and judgments, which ask through the same Jev client, and `now()` and `random()`, which are read live because nothing is recorded.

**Replay.** A log line's identity is its level, message and fields, recorded with its task and source. Replay and catch-up after a resume check each `log` against the next recorded event, and a difference is `replay_diverged` (section 10.4). A replayed log is not emitted to the host a second time: the event stream and the SDK log callback see only lines written live, and a run that leaves replay after a changed host answer emits the lines it writes from there. `jevscript replay` reproduces the recorded lines from the recording file (section 11.6).

**Redaction.** Log values can carry state or observations, and the runtime does not track where a value came from. When `redact` is set, the primary recording stores a log's message and every field value as redaction markers (section 10.3); its level, field names, task and source stay readable, and the private replay companion keeps the full line. Like request state and observations, the host's live event stream and log callback carry the line in full: `redact` governs the stored recording.

---

## 6. Judgments

A judgment expression asks Jev a question about a value and returns a typed answer. There are three verbs, one prefix, and one batching form. Each request has a closed answer space before it is sent. The source declares the `feels` result kind and the labels or levels of a static `pick` or `rate`; `pick among` derives its indexed alternatives from the program-built list, and a machine derives its menu from guards, before its step request. The model supplies estimates within that space, never a new branch or free-form action.

### 6.1 Subjects and state paths

The value on the left of a judgment verb is its **subject**. A subject must be a path: a variable name, an input, a judgment parameter, or field and list-index accesses starting from one of those, such as `local.body` or `files[3]`. An index is evaluated to select a position in the existing list; the whole subject cannot be an arbitrary computed expression. The subject becomes a state path that Jev is told to inspect by name.

```
in message: text
in files: list

judgment review(summary):
  from_parameter = summary feels "states a completed result"

task main:
  local = { body: message }
  from_input    = message feels "asks for a refund"
  from_variable = local feels "contains a body"
  from_field    = local.body feels "contains a concrete request"
  from_index    = files[3] feels "mentions a failing test"
```

Compute first when the desired subject is an arbitrary expression:

```
tail = tail(transcript, 4k)
looping = tail feels "repeats an earlier attempt"
```

### 6.2 `feels`

```
p = <subject> feels "<condition>"
```

Asks a yes/no question. Returns a `prob`: the probability that the condition holds. A `prob` near `0.5` means Jev is unsure; it does not mean "somewhat". Maps to a Jev Noul.

The condition must describe one crisp property. The compiler cannot check this; the guidance in section 6.8 is normative for programs that want reliable answers.

### 6.3 `pick`

```
c = <subject> pick:
  <label>  "<description>"
  <label>  "<description>"
  other
```

Asks Jev to choose one of the labelled descriptions. Returns a `choice`. Maps to a Jev Choice. Rules:

- Two to eight labels, including `other`.
- This two-to-eight rule is a language rule for hand-written labels, not a claim about a provider API maximum. The resolved model profile separately constrains request size and criteria.
- Exactly one label must be `other` or `none`, written bare, with no description. This is the escape option. Jev chooses it when no other description fits. Omitting it is a compile error. Jev needs a criterion for every option, so the runtime describes the escape as `none of the other options fits`.
- Labels are identifiers. They are an API: host code reads `c.label`, and programs use `c is <label>`. Renaming a label is a breaking change.
- A description may be a record instead of a text, to give Jev contrastive detail:

```
billing:
  what     "charges, invoices, refunds or subscriptions"
  not_for  "order tracking or account access"
  examples ["I was charged twice", "Where is my refund?"]
```

`c.confidence` is how peaked the distribution is. In 0.1 the TypeSafe adapter uses a reported confidence when present and its documented fallback when absent; a portable cross-provider confidence formula is not defined here. `c.probabilities` is a record from label to prob. A result may therefore look like `{ label: "billing", confidence: 0.82, probabilities: { billing: 0.91, account: 0.06, other: 0.03 } }`; the numbers are illustrative, and its Jevscript type is `choice`, not a bare text label. Section 4 explains the separate tagged host JSON form.

### 6.4 `rate`

```
l = <subject> rate:
  "<situation at the low end>"
  "<situation>"
  "<situation at the high end>"
```

Asks Jev to place the subject on a spectrum described as situations, low to high. Returns a `level`. Maps to a Jev Score. Rules:

- Two to ten levels.
- This two-to-ten rule is a language rule for hand-written levels, not a claim about a provider API maximum.
- Each level describes a situation that stands alone. This is a Jevscript authoring rule, not a universal provider requirement. Levels must not refer to each other or to numbers. The compiler rejects a level whose text is only a number or a degree word (`"low"`, `"medium"`, `"3"`); it cannot judge every description's quality.
- Levels may be named: `unchanged "each step leaves the state unchanged"`. Named levels allow `l is unchanged`.

`l.level` is the nearest whole level index, starting at 0; on an exact half step the reference runtime rounds toward the higher index. `l.score` is the probability-weighted mean position in this ordered answer space, not a measured physical quantity. `l.normalized` is `score / (levels - 1)`. `l.confidence` measures how peaked the distribution is under the same 0.1 provider/fallback interpretation as `choice`, and `l.probabilities` maps each level name (or index as text) to a `prob`. A three-level result may look like `{ level: 1, score: 1.2, normalized: 0.6, confidence: 0.71, probabilities: { unchanged: 0.1, drifting: 0.6, advancing: 0.3 } }`; the confidence is illustrative, and its Jevscript type is `level`. Threshold or rank on these fields; do not interpolate quantities from them.

### 6.4a `pick among`

```
c = <subject-list> pick among "<question>":
  by <field>          # optional: which field of each item is the description Jev sees
  none                # optional: allow choosing nothing
```

Chooses one element of a runtime list. This is the form for "which of these candidates", such as the clickable elements in a computer-use loop or the open pull requests in a queue. Each element becomes one option; its description is the element's text form, or the named `by` field when the elements are records. Returns a `choice` with two extra fields:

| Field | Value |
| --- | --- |
| `index` | The chosen element's position in the list, or `none`. |
| `item` | The chosen element itself, or `none`. |

`label` is `"i<index>"` or `"none"`, and `probabilities` is keyed the same way. If `none` is not declared, Jev must choose an element, and the program should threshold on `confidence` before acting.

Rules:

- The list must have at least one element at run time, and at most the model profile's `max_criteria_per_question` (section 10.6). Over that limit the run pauses with `error` of kind `pick_too_many`; filter and rank in code first. Section 7.3's pattern, `each ... feels` then `top`, is the way to get a long list down to a short one.
- `each` cannot be combined with `pick among`.
- The shape hash (section 11.4) covers the question and the `by` field, not the runtime items, so the host contract is stable across inputs.
- Descriptions are runtime text and may be agent-written. The same caution as section 9.1 applies: the option Jev picks is the only thing the program acts on, and it is always an index into a list the program built.

### 6.5 `each`

```
ps = each <subject-list> feels "<condition>"
cs = each <subject-list> pick: ...
ls = each <subject-list> rate: ...
```

Asks the same question once per element of a list, all in the same request, and returns a list of answers in the same order. This is how Jevscript counts, filters and ranks: Jev answers per item, code aggregates.

```
off = each files feels "is unrelated to the issue: {issue.title}"
n   = count(off, above 0.6)
```

Each element of the list becomes its own state path (`files[3]`), so the elements should be small. `each` over a list longer than the runtime's per-request question limit is split across requests automatically and recorded as one logical judgment.

### 6.6 Batching

Every judgment expression compiles to a Jev question. Questions are grouped into requests by these rules:

1. Inside a `judgment` block, all questions form one request. Always.
2. Inside a `task` or `def` body, consecutive judgment expressions that do not depend on each other's answers and are not separated by a capability call, a gate, or a pause form one request. The compiler computes this; the programmer does not annotate it.
3. A judgment whose subject depends on an earlier judgment's answer starts a new request.

All questions in a request see the same state. Questions never see each other's answers. This is Jev's own model and Jevscript does not hide it.

The per-request question cap does not permit arbitrary splitting of a compiler request group. If its expanded questions exceed the profile cap and no individual `each` exceeds that cap, execution fails before sending with `state_too_large`, naming the group. If an individual `each` exceeds the cap, section 6.5 permits consecutive chunks of that logical group, each within the cap and in question order. This is the sole exception to the one-request rule for a named judgment.

Speculative fan-out is encouraged: ask every question a branch might need up front, and ignore the answers the branch does not take. The runtime charges one call per request, not per question.

### 6.7 `judgment` blocks

```
judgment <name>(<param>, <param>, ...):
  <result> = <judgment expression>
  <result> = <judgment expression>
  ...
```

A named judgment. Its parameters are the state fields. Its body is only judgment assignments and `log` lines (section 5.8); no other statements are allowed. Calling it returns a record of the results:

```
j = read_agent obs          # obs is a record with fields matching the parameters
j = read_agent(summary: s, files: f, tests: t, recent: r)
j.claims_done               # a prob
j.next.label                # a text
```

Because a `judgment` has no capability calls and declares its state parameters, the host can run it directly with fixture state (section 11.3). Running it still sends a model request. This is the unit for testing questions against labelled examples.

### 6.8 Question detail

Any judgment verb accepts a detail block that refines the question. The detail keys map directly to Jev's instruction object.

```
claims_done = summary feels "states the work is complete":
  focus "Look for an explicit statement, not a plan or intention."
  note  "The agent writes in first person."
  yes   ["Done.", "All tests pass and the PR is ready."]
  no    ["I will now run the tests.", "Next I need to fix the import."]
```

| Key | Applies to | Meaning |
| --- | --- | --- |
| `focus` | all | What to look at within the subject. In a `pick:` or `rate:` block, `focus`, `compare` and `sample` lines sit among the labels or levels. |
| `note` | `feels` | Supporting fact the question needs. In a `pick:` or `rate:` block it would read as a label, so fold it into `focus` there. |
| `compare` | all | A list of other state paths to read alongside the subject. |
| `yes`, `no` | `feels` | Short concrete examples of each side. |
| `examples` | a `pick` label | Short concrete instances of that label. |

Guidance that the compiler cannot enforce but the runtime's answer quality depends on:

- One property per question. A question that weighs two things gets a low-confidence answer.
- Write the exact condition. Jev reads scoping words and negations literally.
- Keep policy out of the question. "A shared address cannot override a name conflict" is an `if`, not a `focus`.
- Convert numbers to words before judging them. Send `"three days late"`, not `72`.
- Do not ask Jev to count, add, or compare dates. Use `each` plus builtins.

### 6.9 State construction

The state sent with a request is built from the subjects of every question in the request, each placed at its path. For a `judgment` block the state is exactly the parameters. For inline judgments in a task the state is exactly the variables the subjects name, with only the fields the paths reach.

The named-judgment rule includes every declared parameter in full, even one no subject reads, and excludes all other in-scope values. Every permitted `each` chunk of a named judgment carries this same parameter state. Inline request chunks contain only the subject and `compare` paths they actually ask about.

Nothing else is sent. A variable that is in scope but not named by a subject does not reach Jev. This is deliberate: Jev's accuracy falls as unrelated state grows.

A subject that indexes into a list, such as `files[3]`, is sent as the list with `null` at every position the request does not judge, so that the path still addresses the element. A `compare` path (section 6.8) is placed in the state the same way as a subject.

There is no host-only extra-state merge. To give a named judgment more context, declare a parameter and pass it explicitly: all named parameters enter its request state in full. For an inline judgment, put relevant context in the subject record or name a path in `compare`. Host inputs remain subject to the same rules. Section 6.12 shows the resulting abstract requests.

### 6.10 Size limits

The runtime estimates tokens before sending. If the state plus all questions exceed the model's total limit, or the state plus the largest question exceed the per-question limit, the run pauses with `error` of kind `state_too_large`, naming the largest subject. These are properties of the selected judgment model, not universal Jevscript constants: every provider/model profile supplies its own total limit, per-question limit, question cap, criteria cap and tokenizer. No program or runtime branch hard-codes one provider's limits. The `shape` statement (section 7.2) is how programs stay under the selected profile's limits.

### 6.11 Sampling

By default `pick` chooses the highest-probability label and `rate` reports the nearest level. Sampling draws the label or level from Jev's distribution instead, so a 20% option is taken one run in five. It is useful for exploring a program's branches, for stress-testing gates, and for deliberately varied behaviour.

Sampling is switched on in two places:

- Per judgment, with the detail key `sample true`. That judgment always samples.
- Per run, with the SDK option `sample: true` or `sample: { seed: <number> }`. Every `pick` and `rate` in the run samples.

`feels` is never sampled; it returns the probability and the program thresholds it. Sampling changes which `label` or `level` is reported and nothing else: a `rate` result keeps its `score` and `normalized` position, and `probabilities` and `confidence` remain unchanged. Every draw uses the run's recorded random source (section 10.3, event `draw`), so a sampled run replays exactly. Recordings note `sampled: true` on the affected `answers` events.

### 6.12 Abstract request examples

The examples below describe the state and questions a judgment request represents. `binary`, `categorical`, and `ordinal` explain the three answer spaces; these sketches are not the TypeSafe HTTP payload, the IR JSON schema, or a new host wire format. The actual request records its state and questions (section 10.3), with the 0.1 adapter mapping the forms to Noul, Choice, and Score.

| Program expression or step | State in the request | Question in the request |
| --- | --- | --- |
| `urgent = e.body feels "needs a reply today"` | `{e: {body: "..."}}` | Binary; id `urgent`, subject `e.body`, condition `needs a reply today`. |
| `owner = e.body pick: ...` with `code`, `customer`, `other` labels | `{e: {body: "..."}}` | Categorical; id `owner`, subject `e.body`, exactly the three declared labels and their descriptions. |
| `progress = summary rate: ...` with named levels `unchanged`, `drifting`, `advancing` | `{summary: "..."}` | Ordinal; id `progress`, subject `summary`, the three declared situations in order. |
| `flags = each files feels "is unrelated"` for `files = ["a", "b"]` | `{files: ["a", "b"]}` | Two binary questions: ids `flags[0]`, `flags[1]` and subjects `files[0]`, `files[1]`. The answer is a two-element list in this order. |
| `selected = candidates pick among "which fits?"` with `by title` and `none` in its block | `{candidates: [{title: "a"}, {title: "b"}]}` | One categorical question; id `selected`, subject `candidates`, labels `i0` and `i1` described by the two titles, followed by `none`. The returned `index` and `item` come from this program-built list. |
| A named `assess(summary, metadata)` judgment with `risk = summary feels "risky"` in its body | `{summary: "...", metadata: {...}}` | Binary; id `risk`, subject `summary`. Every named parameter is included in full, even though this question does not read `metadata`. |
| A machine step in `working` with enabled events `finished`, `stuck` | `{state: "working", goal: "...", obs: {...}, recent: [...]}` | Categorical; id `event`, subject `obs`, comparisons `state`, `goal`, `recent`, labels `finished`, `stuck`, then `stay`. A false `when` guard would remove its event before this request. |

For an inline group, multiple consecutive questions share one assembled state and one request (section 6.6). Thus the three questions over `e.body` in section 14.3 send only `{e: {body: "..."}}` even if `e.subject` and `e.source` are also in scope. A `compare` path adds only its named value. If a request contains only `files[1]`, its state uses `{files: [null, "b"]}` to preserve the index. If `each` must split at the profile question cap, each chunk contains only its own inline paths; a named judgment chunk still contains every declared parameter in full.

`focus` (section 7.3) composes these same operations. Its prelude implementation chunks text, then asks `each chunks feels "contains information relevant to: {purpose}"`; the request has one binary question per chunk, with ids like `keep[0]` and subjects like `chunks[0]`. The runtime chooses chunks with ordinary code and repeats until the result fits. The model does not choose arbitrary text to retain outside that declared process.

---

## 7. Tasks

```
task <name>(<param>, ...) budget calls <n>, minutes <n>, usd <n>, steps <n>:
  <statements>
```

A task is the unit the host runs. `main` is the entry point and takes no parameters; it reads the program's `in` declarations. Any other task takes parameters and may be called from a task like a function; each call runs under the caller's remaining budget and returns the value of its `return`. Tasks are the unit of reuse across modules (section 3.9): a library exposes a task, the importer hands it handles and values.

### 7.1 Budgets

| Key | Counts |
| --- | --- |
| `calls` | Model requests: Jev requests plus `llm` generations. |
| `minutes` | Wall-clock minutes since the task started, excluding time spent paused waiting on the host. |
| `usd` | Estimated spend from recorded usage and the runtime's price table. |
| `steps` | Iterations of the outermost loop in the task. |

Any budget missing from a task is unlimited for that key, except `calls`, which defaults to 50. Exceeding a budget pauses the run with `budget`. The host may extend and resume, or abort.

This 50-call default is a 0.1 language execution rule, filled into compiled IR and also applied to an IR task or machine whose `calls` field is absent. It is distinct from a selected model's request or token caps. Removing it would change the same source program's execution and requires a versioned decision; section 10.6 governs model profiles, not this task resource default.

**Nesting.** When a task or machine calls another task or machine, the callee's effective budget for each key is the smaller of its own declared limit and the caller's remaining amount. Usage inside the callee counts against every task on the call stack. A callee therefore can tighten its budget and can never widen it. The `budget` pause names the task whose limit was hit; extending it extends only that task's limit, and the run continues if every task on the stack still has room.

**Agents' own spend.** A spawned agent runs its own model on the host's account. The runtime does not see those calls, so they do not count against `calls` or `usd`. An `agent` adapter may report `usage: { usd, tokens }` in its observation record; the runtime sums whatever it reports into `usage.adapter_usd` and `usage.adapter_tokens` on the `done` pause, for information. A program that wants to gate on it reads the observation field and writes the `if` itself.

### 7.2 `shape`

```
obs = shape:
  <field>  <expr>, max <tokens>
  <field>  <expr>
```

Builds a record whose fields are token-capped. `shape` is the declared answer to the context window: every field that will reach Jev states how large it may be.

A field takes an ordinary expression or one command-form call (section 13). It may compute literals, operators, records, lists, comprehensions, field and index access, function-form calls, `focus`, or `trail`, subject to the enclosing unit's restrictions. An inline judgment is a separate assignment form and cannot be nested in a shape field; assign its result first if the program needs that value. Fields evaluate in written order. A cap applies to the constructed field value; it does not prevent an expression's permitted calls or account for their cost.

- `max` is a hard cap in tokens. `2k` is 2000.
- When a field's value exceeds its cap, the runtime applies the field's policy. The default policy is `head`: keep the first tokens and record a `truncated` warning. Alternatives: `tail`, written `<field> <expr>, max <tokens>, tail`, or `focus on "<purpose>"` (below), written by making the field's value a `focus` expression.
- A field without `max` is uncapped, and the compiler warns unless the field is a number, bool, or a list of fewer than 64 short items.
- `shape strict:` makes an overflow an `error` pause instead of a truncation.

### 7.3 `focus`

```
short = focus <text> on "<purpose>", max <tokens>
```

Reduces a text to the parts relevant to a purpose using Jev, recursively, until it fits. `focus` is a language form, not a def, because `on` and `max` are keywords in its call syntax. Its semantics are exactly those of this prelude def, which the runtime ships and may execute directly:

```
def focus_impl(text, purpose, limit):
  if tokens(text) <= limit:
    return text
  chunks = chunk(text, tokens 1500)
  keep   = each chunks feels "contains information relevant to: {purpose}"
  kept   = [c for c, p in zip(chunks, keep) if p > 0.6]
  if len(kept) == 0:
    kept = top(chunks, by keep, n 3)
  return focus_impl(join(kept), purpose, limit)
```

Code decides the chunk boundaries and when the result fits. Jev decides relevance. One request per level of recursion. The `[... for ... if ...]` comprehension form is defined in section 8.2.

The prelude is the first home for behavior expressible with existing statements and functions. `focus` has a dedicated form because its `on` and `max` syntax is part of the language; its reduction semantics remain those of `focus_impl`. `stuck` and `repeats` remain ordinary defs. `shape`, `trail`, `gate`, and `verify` have runtime state or pause effects and cannot be replaced by a value-only helper without changing their guarantees.

### 7.4 `verify`

```
until verify(<expr>), max 6:
```

`verify` marks the programmer-declared condition whose satisfaction the task reports. The runtime owns its evaluation and records the result; the host reads the `done.verified` field and may apply its own acceptance policy without changing that field. It has three effects:

1. The predicate expression is evaluated by the Jevscript runtime. It is not automatically sent to Jev as a new verification question. Its values may nonetheless come from earlier judgments or permitted calls, including a def that judges.
2. When it becomes true, the enclosing loop ends and the recording notes `verified: true`.
3. A `done` pause emitted from a task that declared a `verify` carries `verified: true` only if that condition was true at the end. A host can therefore distinguish a satisfied declared predicate from a task that merely ran out of loop.

A task may declare `verify` once, on an `until` directly in the task body. It cannot be nested inside another statement block, a def, or a machine action; these placements are `syntax` errors. This keeps its condition in task scope for the final evaluation. If execution returns before reaching the verify loop, completion is unverified and the skipped condition is not evaluated, including any capability reads it would perform. Once execution reaches the verify test, it is evaluated again in the final task environment to determine `done.verified`.

`verified: true` means that this declared predicate evaluated true at the final boundary. Its strength depends on the predicate and its evidence. `verify(tree.tests_pass)` can read a test result; `verify(j.claims_done > 0.9)` can depend on a model estimate. Neither the flag nor a successful comparison independently proves the external goal. A program seeking a stronger check must choose an appropriate observation and write it into the predicate; the runtime does not infer trustworthiness from a field name.

A `gate` that sees a judgment saying the goal is done (section 7.6) does not end a task that declares a `verify`; it only ends a task that has none, even if execution has not reached the declared verify loop yet.

### 7.5 `trail`

```
recent = trail <n>
```

The last `n` step records of the enclosing task, oldest first. A step record is built by the runtime, not by any model:

```
{
  step:    3,
  action:  "send",             # the capability verb
  target:  "dev",              # the capability or handle
  args:    "Tests fail: ...",  # text form, capped at 200 tokens
  changed: true,               # whether the next observation differed from the previous one
  outcome: "observed"          # observed | error | no_change
}
```

`changed` is computed by hashing the text form of the observation that followed the action. Because the runtime writes these records from its own calls, they cannot be steered by anything an agent prints.

### 7.6 `gate`

```
gate risk <prob>, confidence <number>, done <prob>?:
  proceed  -> <statements>
  confirm  -> <statements>
  escalate -> <statements>
  stop     -> <statements>
```

A gate turns judgments into a verdict using thresholds declared once per task, and then runs the arm for that verdict. It is the only place where policy thresholds live.

`risk` and `confidence` are required; `done` is optional. Thresholds are declared on the task:

```
task main thresholds risk_confirm 0.2, min_confidence 0.5, stop_confidence 0.3, done 0.9:
```

The gate's verdict is computed in this order:

1. `stop` if `confidence < stop_confidence`.
2. `confirm` if `risk >= risk_confirm`.
3. `escalate` if `confidence < min_confidence`.
4. `proceed` otherwise.

If `done` is supplied and `done >= thresholds.done`, the verdict is `proceed` and the gate sets the task-local flag `gate.done`, which a task without `verify` treats as its end. A task with `verify` ignores it.

An arm that is not written has a default:

- `proceed`: continue.
- `confirm`: pause the run with `confirm`, message `"Gate asked for confirmation"`, and continue if the host answers `yes`; otherwise end with `stopped`.
- `escalate`: pause the run with `escalate` and end.
- `stop`: end the run with `stopped`.

Arms are ordinary statement blocks. `->` followed by a single statement on the same line is allowed; `->` at the end of the line opens an indented block.

### 7.7 Pauses from tasks

A task pauses the run in these ways:

| Statement | Pause kind |
| --- | --- |
| `person.ask "..."` | `confirm`. Resumes with the host's answer. |
| a gate `confirm` default | `confirm` |
| `escalate "reason"` or a gate `escalate` | `escalate`. Terminal unless the host resumes explicitly. |
| `stop "reason"` or a gate `stop` | `stopped`. Terminal. |
| `agent.wait ...` | `waiting`. Informational; auto-resumes. |
| budget exceeded | `budget`. Resumable if the host extends. |
| runtime or adapter error | `error`. Resumable only for retryable kinds. |
| `return` from `main`, or falling off its end | `done`. Terminal. |

Section 10 defines the pause payloads.

### 7.8 `machine`

A machine is the fourth unit. It declares states and the events enabled in each state. At every step Jev is asked one Choice over exactly the enabled events, plus `stay`. Code evaluates guards, runs the actions, and moves the state. The machine is the deterministic skeleton; Jev only ever picks a legal transition.

```
machine <name>(<param>, ...) budget ... thresholds ...:
  goal "<text>"                                  # sent to Jev on every step
  initial <state>                                # optional; defaults to the first state

  observe:                                       # runs before every step, same form as shape
    <field>  <expr>, max <tokens>
    ...

  state <name>:
    on <event> "<description>" -> <state> [when <expr>] [risky]:
      <statements>                               # optional actions, run when the event fires
    on <event> "<description>" -> <state>
    ...

  state <name> done                              # terminal; no events
```

**Step.** Calling a machine runs it until it reaches a `done` state or the step limit:

```
result = review(dev, tree, max 20)
```

`max` is required. Each step:

1. If the current state is terminal, return.
2. Evaluate `observe` into a record, exactly as `shape` would. It is bound to the name `obs` in every guard and action block of that step.
3. Enabled events are the current state's events whose `when` guard is true. Guards are evaluated in code and a false guard removes the event from what Jev sees.
4. If no event is enabled, pause with `escalate`, reason `no_enabled_events`.
5. Send one request whose state is `{ state: <name>, goal, obs, recent }`, where `recent` is the last six machine events as runtime-written records `{ step, from, event, to }`. The single question is a Choice with id `event`, subject path `obs`, and `compare` paths `["state", "goal", "recent"]`, asking "Which enabled event should happen next to advance the goal?" Its labels are the enabled event names in declaration order, described by their texts, plus `stay` last, described as "nothing in the observation calls for a transition yet".
6. Apply the machine's thresholds as a gate with `confidence` set to the choice's confidence and `risk` set to `1` if the chosen event is marked `risky`, else `0`. `stop`, `confirm` and `escalate` verdicts behave as in section 7.6; a `confirm` that the host declines is treated as `stay`. If the host explicitly resumes an `escalate`, that step is also treated as `stay`: its action block does not run and the next step observes again. The step is counted in either case. When no event was enabled in step 4, explicitly resuming likewise counts the step and observes again, without a Jev request.
7. If the choice is `stay`, count the step and repeat. If an event fires, run its action block, then set the state to the event's target.

**Result.** The call returns a record:

```
{ state: <final state name>, steps: <number>, done: <bool>, verified: <bool>, events: [<event records>] }
```

`done` is true if the final state is terminal. `verified` is true only if the transition into the terminal state carried a `when` guard that passed; this is the machine's declared completion rule. A terminal state reached through an unguarded event is done but not verified. A guard can depend on model-derived or adapter-supplied values, so this flag reports a satisfied code guard rather than objective truth.

**Rules.**

- Every state name in a `->` target must be declared (`machine_unknown_state`). At least one state must be `done` (`machine_no_done`). Every event needs a description (`event_no_description`). A state with no path to any terminal state is a warning (`machine_unreachable_done`).
- Event names are identifiers and are unique within a state. The same event name may appear in several states with different targets.
- Action blocks may call capabilities and invoke judgments, defs, tasks or other machines. Task and machine calls use the same nested-budget accounting as calls from a task. The 0.1 recursion limit applies to def frames; it is not a universal task/machine call-depth guarantee. Actions may not `gate`, and they may not change the state directly; the transition does that. `return` belongs to defs and tasks and is a `syntax` error anywhere in a machine action, including its nested statement blocks. A called task may return a value to the action, but it cannot return from the enclosing machine. `break` and `continue` remain local to explicit loops within the action.
- Parameters are readable in guards, actions and `observe`. A machine may declare `needs` only through its module, like a task.
- The machine's shape hash covers state names, event names and their targets. Descriptions are excluded, so rewording is not a breaking change.
- Pauses raised inside a machine carry `task: "machine:<qualified name>"` and `state: <name>`.
- Recording writes one `machine_step` event per step: `state`, `enabled`, `chosen`, `probabilities`, `confidence`, `to`.

**Recording a decision.** For a Jev decision, `chosen`, `probabilities` and `confidence` preserve the model decision (including a sampled choice); `to` is the destination permitted by the gate, and remains the current state when a confirm is declined, an escalation is resumed, or the gate stops. Record the event after the gate verdict is resolved and before running an action or starting the next step. A completed `stay`, including a declined confirm or resumed escalation, is included in the result's `events` and subsequent `recent` records with `event: "stay"` and identical `from` and `to`.

When no event is enabled, write the step event before the `no_enabled_events` pause with `enabled: ["stay"]`, `chosen: "stay"`, `probabilities: {}`, `confidence: 0`, and `to` equal to the current state. The empty distribution and zero confidence are explicit no-decision sentinels, not a model answer; no `request` or `answers` event is emitted and no gate is applied. Only an explicit resume completes and counts that step, adding its `stay` to `events` and `recent`. A terminal state checked in step 1 does not produce a step event.

**When to use a machine and when a task.** A task is right when the loop is short and the branching is over content. A machine is right when the run has distinct phases, when the set of legal next moves depends on the phase, or when a host wants to draw and pin the control flow. Machines compose through modules like everything else: a library exports a machine, a program calls it.

---

## 8. Defs

```
def <name>(<param>, <param> = <default>, ...):
  <statements>
  return <expr>
```

A helper without capability calls or gates. It may contain judgments, conditionals, loops, and builtins, including recorded time or random reads; it may call other defs. It cannot directly perform a capability call or declare a pause. Recursion is allowed; the runtime enforces a def-frame depth limit and pauses with `error` of kind `recursion_limit` if exceeded.

### 8.1 The standard prelude

A small set of defs is available in every program without import. They are written in Jevscript and their source ships with the runtime. `focus` (section 7.3) is one. `stuck` is another:

```
def stuck(steps):
  if repeats(steps) >= 2:
    return true
  same = steps feels "the recent attempts use the same approach with only cosmetic differences"
  progress = steps rate:
    unchanged "each step leaves the state unchanged"
    drifting  "the state changes but not toward the goal"
    advancing "the state is moving toward the goal"
  return same > 0.7 or (progress is unchanged and progress.confidence > 0.6)

def repeats(steps):
  keys = [hash([s.action, s.target, s.args]) for s in steps]
  most = 0
  for k in keys:
    n = len([x for x in keys if x == k])
    most = max([most, n])
  return most
```

The prelude exists to show that the primitives compose, and so that programs share one definition of "stuck" that can be improved centrally.

### 8.2 Comprehensions

```
[expr for x in xs]
[expr for x in xs if cond]
[expr for x, y in zip(xs, ys)]
```

List comprehensions are the only comprehension form. `zip` is a builtin that pairs lists. Comprehensions are expressions and may appear anywhere an expression may.

---

## 9. Capabilities

A capability is declared with `needs` and bound by the host. Its kind fixes its verbs.

### 9.1 `agent`

Something that runs a task in the world over time and can be observed: Claude Code, Codex, any terminal agent, or a browser agent.

| Verb | Signature | Returns |
| --- | --- | --- |
| `spawn` | `spawn in <handle>?, prompt <text>, ...` | `handle` |
| `observe` | `<handle>.observe` | observation record |
| `send` | `<handle>.send <text>` | none |
| `wait` | `<handle>.wait idle, minutes <n>` | observation record; pauses with `waiting` |
| `stop` | `<handle>.stop` | none |

The observation record has at least:

```
{
  status:        "running" | "waiting" | "exited",
  last_message:  text,          # the agent's most recent complete message to the user
  tail:          text,          # the last screen or ~4k tokens of transcript
  exit_code:     number | none
}
```

An adapter may add fields. Programs should `shape` before judging any of them; `tail` in particular is agent-written text and can contain anything.

`spawn` accepts adapter-specific named arguments after `prompt`. The compiler passes unknown named arguments through to the adapter without checking them.

### 9.2 `person`

A human in the loop.

| Verb | Signature | Effect |
| --- | --- | --- |
| `ask` | `ask <text>, options [<text>...]?` | Pauses with `confirm`. Returns a `pause_result`. |
| `notify` | `notify <text>` | Sends without pausing. |
| `take_over` | `take_over` | Pauses with `escalate`. |

### 9.3 `llm`

A text generation model.

| Verb | Signature | Returns |
| --- | --- | --- |
| `write` | `write "<instruction>" using <value>?` | text |

The runtime evaluates `using`, if present, and forwards that value as the named `using` argument to the host's `write` adapter, without implicitly adding other program variables or first converting the value to text. It records the instruction, the exact evaluated `using` value, the output and usage; replay checks that value as part of the generation's identity. The adapter owns its mapping from a Jevscript value to a model prompt or other context. A universal projection for records, lists, opaque handles, adapter history and system framing is not defined in 0.1, so portable programs should supply explicit text or a documented adapter-specific shape. Each `write` counts one call against the task budget. The runtime estimates generation tokens from the instruction, the value's text form and the output, but the selected Jev profile does not price the host's LLM generation.

### 9.4 `tool`

Anything else. A `tool` capability forwards verbs to the host adapter, which returns a value the program uses like any other. `tree` in the examples is a `tool`. Verbs are checked at three points, each optional and each stricter than the last:

**1. Declared signatures.** A `needs` of kind `tool` may open a block that lists the verbs the program uses:

```
needs tree: tool:
  create(branch) -> handle
  diff() -> record
  test_summary() -> text
  tests_pass() -> bool
  open_pr() -> text
```

With a signature block, the compiler rejects an unlisted verb (`verb_unknown`) and a call with the wrong number of positional arguments (`verb_arity`). Return types are `text`, `number`, `bool`, `list`, `record`, `handle` or `none`, and the runtime checks the adapter's actual return against them (`type_error`). Without a block, the tool is open and nothing is checked before run time.

**2. Adapter manifests.** When the host binds a `tool`, it may pass a manifest, a JSON record `{ verbs: { <name>: { params: [<name>...], returns: <type> } } }`. At `task.start` the runtime compares every verb the IR references on that capability against the manifest and refuses to start with `verb_missing` if one is absent, so a mismatch surfaces before any model call. `jevscript check --tools <manifest.json>` runs the same comparison without starting a run.

**3. Run time.** A verb the adapter does not implement fails at the call with `error` of kind `verb_missing`. This is the only check that applies to an open tool with no manifest.

Adapters for `agent`, `person` and `llm` have fixed verbs and need no manifest.

### 9.5 Adapter contract

From the runtime's side an adapter is an object that answers `call(verb, args) -> value` and, for `agent` kinds, `observe(handle)`. Every call and its result is recorded. Adapters run on the host side of the runtime boundary (section 11.5), so they are written in the host language, not in Jevscript.

Adapters return values of the verb's declared kind: text, bool, none, records, lists, or handles as appropriate. Observations should expose structured fields that a program can shape and judge. An opaque handle remains an adapter-owned reference, not a request to copy all of its external contents into a judgment state.

### 9.6 How a capability call reaches the host

A capability verb in a program is a request to the host, not a call into the runtime. When a task or machine evaluates `screen.click target.index`, the runtime sends the host `capability.call { capability: "screen", verb: "click", args: [56] }` over the boundary in section 11.5 and blocks until the host replies with a result value. The SDK receives that request, looks up the adapter the client bound under `screen` at `task.start`, invokes the matching method, and returns whatever it returns. The client therefore writes ordinary functions and never sees the protocol.

Two consequences follow:

- **A capability call is not a pause.** The client's pause loop does not see individual calls. It sees pauses only. A client that wants to approve a specific call does so either in the program, by marking the event `risky` or feeding a judgment into `gate risk` so a `confirm` pause is raised, or in the adapter, by asking before executing. Both are supported; the first puts the policy in the program, the second in the host.
- **Every call is observable anyway.** Each call and its result is written to the recording as a `call` event (section 10.3) and streamed to the client through `Run.events()`, so a client can display tool activity live without turning calls into pauses.

The host may implement an adapter wherever the tool actually lives. A thin client can forward each call to another process, a device, or a remote service and return the answer. The runtime blocks for as long as the host takes; the `minutes` budget still runs, and `abort` during a call cancels the run once the call returns. The completed call and its result are recorded before cancellation; no subsequent program statement or external effect executes. An outstanding `run.next` returns a terminal `stopped` pause with reason `"aborted by host"`.

Host integrations may wrap a bound adapter before and after a live tool call. A pre-call wrapper receives the evaluated verb and arguments, applies host authorization or runs a separate Jevscript decision with explicit inputs, and then permits or refuses the effect. A post-call wrapper observes the actual outcome under the same invocation identity. The parent run records the adapter result or error it receives; replay serves that record and does not call the wrapper again. Non-idempotent effects need a durable identity and a retry/recovery rule so a post-call failure does not cause the original action to execute twice.

For a host-level event such as skill suggestion or run completion, the host may launch a separate Jevscript program with its own explicit inputs and recording, correlated to the parent run and bounded by host policy. The host consumes `done`, `stopped`, or an unresolved `escalate` according to its own workflow; an event notification alone does not guarantee exactly-once external delivery. These are host integrations, not implicit language hooks named `pre_tool_use`, `post_tool_use`, or `finished`.

---

## 10. Runtime model

### 10.1 Runs

A run is created from a compiled program, a task name, inputs, bindings, and either a recording sink or a replay source. It advances only when the host steps it. It is always in exactly one of:

```
created -> running -> paused(kind) -> running -> ... -> ended(kind)
```

`running` is transient; the host never observes it directly. The host observes pauses.

The `record` and `replay` options are mutually exclusive; supplying both is a `type_error` before execution. A host can change an answer during replay to continue live (10.4), but this does not create a new file recording.

### 10.2 Pause kinds and payloads

Every pause carries `kind`, `run_id`, `step`, `task`, `source` (a source location), and `recording_offset`. Kind-specific fields:

| Kind | Terminal | Fields | Host resumes with |
| --- | --- | --- | --- |
| `confirm` | no | `message`, `options` (list of text, default `["yes", "no"]`), `context` (record) | `{ answer: text, text?: text }` |
| `escalate` | by default | `reason`, `context` | `{ resume: true }` to continue, else abort |
| `waiting` | no | `on` (capability name), `condition`, `timeout_minutes` | nothing; auto-resumes. Host may `inject` a message. |
| `budget` | no | `key`, `used`, `limit` | `{ extend: { key: n } }` to raise the limit |
| `error` | depends | `code`, `message`, `retryable` | `{ retry: true }` if retryable |
| `stopped` | yes | `reason` | nothing |
| `done` | yes | `outputs` (record), `verified` (bool: final declared task predicate satisfied), `usage` (record) | nothing |

`usage` on `done` reports calls, tokens, estimated USD, minutes, and steps.

### 10.3 Recording

A recording is a JSONL file. Every line is an event with `ts`, `run_id`, `seq`, and `event`. Event kinds:

```
start        program, task, inputs, bindings (names and kinds only), ir_version, ir, profile, sample
request      request_id, state, questions          # what was sent to Jev
answers      request_id, answers, usage, latency_ms
generate     capability, instruction, using, output, usage
call         capability, verb, args, result, latency_ms
observe      capability, handle, observation
effect_error operation, identity, code, message, retryable, source
draw         kind (random | now), value
warning      code, message, source
log          level, message, fields, task, source   (section 5.8)
pause        kind, payload
resume       payload
abort        reason                              # host cancellation, not a program stop
step         the trail record (section 7.5)
machine_step machine, state, enabled, chosen, probabilities, confidence, to   (section 7.8)
end          kind, outputs, verified, usage
```

The `start` event embeds the linked `ir`, the resolved model `profile` (including its concrete model id), and the effective run `sample` option. Together with its inputs and task, these make a recording self-contained for `jevscript replay <recording.jsonl>`; replay does not recompile source files or resolve profiles from the current environment. The IR version must be supported. Recordings lacking this execution metadata may be replayed by an embedding host that explicitly supplies it, but the standalone CLI reports `replay_diverged` rather than guessing a source file or profile.

A failed external attempt records `effect_error` instead of its success event, before retry handling, an error pause, or host cancellation. `operation` is exactly `request`, `call`, `observe`, or `generate`. Its `identity` contains respectively `{request_id}` (the preceding request already records its inputs), `{capability, verb, args}`, `{capability, handle}`, or `{capability, instruction, using}` with the same values and JSON forms as the successful event. The remaining fields reproduce the runtime error, including its section 12 code, message, retryability and source location. Replay validates this identity and returns the recorded error without invoking the external service, so failed attempts and their retry decisions occur at the same boundaries. This includes failure of an in-flight callback when an abort is pending: the failure is recorded before `abort`, and cancellation wins over retry handling. Failure to initialize an external client also belongs to its attempted operation; request inputs are recorded before client initialization, so replay does not depend on current credentials or configuration. Local deterministic validation failures before an external operation do not invent an external attempt.

A file recording contains one run: exactly one leading `start`, the same `run_id` on every line, and contiguous `seq` values starting at zero. Replay validates this envelope and agreement between the start's `ir_version` and the embedded IR before execution; an invalid envelope or unsupported version is `replay_diverged`. The file recorder creates a new primary file and, when redacting, a new companion. It refuses an existing destination before execution without overwriting or appending to it. If creating either member of the pair fails, any pre-existing file remains untouched and no external effect runs.

State and observation payloads are recorded in full by default. A host may set `redact` to store a hash and a token count instead in the primary recording. This includes observations returned by `agent.wait`, even when they occur as capability-call results.

A redacted payload is encoded as the exact object `{"redacted":{"hash":<text>,"tokens":<nonnegative integer>}}`. Full payloads normally retain their plain JSON form. If a full payload itself has exactly that marker shape, or is an object whose sole key is `$recorded_full`, the recorder escapes it as `{"$recorded_full":<original payload>}`. Reading this escape unwraps exactly once directly into the payload type. Extra keys make an object ordinary full data, not a marker. Thus arbitrary full data round-trips without being mistaken for redaction or another escape.

Hashes alone cannot reproduce code that reads observation values. To preserve exact replay, the reference file recorder therefore also writes a private, unredacted replay companion at `<recording-path>.replay.jsonl` whenever `redact` is enabled. This companion contains the original events, has owner-only permissions where supported, and is explicitly identified by the CLI as sensitive replay data. A redacted recording and its companion together are the replay artifact; only the primary file is redacted. A `log` event's message and each of its field values are redacted too, because a log can carry state or an observation (section 5.8). This option does not redact arbitrary inputs, outputs, call arguments, or generated text.

Replay loads the companion only when needed for a redacted payload and validates that its run id, sequence, event identity, hashes and token counts match the primary recording. A missing or mismatched companion raises `replay_diverged` before any external effect. It never invents placeholder observations. A custom recording sink must likewise retain an explicit private replay source if it redacts values needed for execution. A full, unredacted recording remains self-contained in one file.

Host cancellation records `abort` with reason `"aborted by host"`, followed by the terminal `stopped` pause and matching `end`. Cancellation of a created run first records its `start` metadata without executing program code. Cancellation of a paused run follows the open pause without inventing a `resume`. In both cases `Run.abort()` still returns no value, ends the run, and exposes the terminal pause through its current state and event stream. An abort requested during a capability call is recorded after that call returns, before further execution. Replay consumes the recorded abort at that exact boundary and produces the same terminal pause without a new host abort; after an earlier nonterminal pause, the next replay step may therefore consume an abort instead of a resume. A host abort while replaying discards the unused replay suffix and ends at the host's chosen boundary.

### 10.4 Replay

In replay mode the runtime reads the recording instead of calling Jev, LLMs, adapters, the clock, or the random source. Each replayed event must match the next expected event's kind and identity (request shape, capability and verb); a mismatch pauses with `error` of kind `replay_diverged` and names the source location. Pauses replay in order and are surfaced to the host exactly as they were the first time, with the recorded resume applied automatically unless the host chooses to answer differently, in which case the run leaves replay at that point and continues live.

Determinism guarantee: given the same program, inputs and recording, control flow is identical up to the first host answer that differs.

### 10.5 Concurrency

A run is single-threaded. Jev questions in one request are parallel inside Jev; nothing else in the language runs in parallel. A host that wants parallelism starts several runs.

### 10.6 Model profiles

Everything the runtime needs to know about a model is a profile, keyed by model id. Profiles are the only place token limits, request caps and prices live; programs never state them and the spec never hard-codes them.

```json
{
  "model": "jev-1.13.0",
  "endpoint": "https://api.typesafe.ai/v1/systemone",
  "total_tokens": 64000,
  "state_plus_question_tokens": 32000,
  "max_questions_per_request": 64,
  "max_criteria_per_question": 255,
  "tokenizer": "chars4",
  "price_per_million_input_usd": 0.042,
  "price_per_million_output_usd": 0.0
}
```

| Field | Used by |
| --- | --- |
| `total_tokens` | The request size check in section 6.10. |
| `state_plus_question_tokens` | The per-question check in section 6.10. |
| `max_questions_per_request` | Splitting `each` over long lists (section 6.5). |
| `max_criteria_per_question` | A model-profile ceiling checked against `pick` labels and `pick among` candidates at execution. The language's static two-to-eight `pick` and two-to-ten `rate` arities remain separate compile-time rules. |
| `tokenizer` | The estimator `tokens()` uses: `chars4` (characters divided by four) or `tiktoken:<encoding>`. |
| `price_per_million_*` | The `usd` budget and `usage` reporting. |

The runtime bundles profiles for the model ids it was built against; their provider limits and prices are taken from the TypeSafe documentation for that version and the bundle records the date and sources. Where a provider does not document a limit, the bundle must identify its value as a conservative runtime policy rather than a measured or documented provider maximum. The reference bundle uses a questions-per-request cap of 64 under that policy. Its `chars4` tokenizer is an estimator, not a claim about the provider's tokenizer. A host overrides or adds profiles with the SDK option `profiles: <path>` or the environment variable `JEVSCRIPT_PROFILES`, pointing at a JSON file holding a list of profile records. A run whose model id has no profile fails to start with `profile_missing`. The `model` option on `task.start` and `judgment.run` selects the profile; the default is `jev-latest`, which the bundle maps to a concrete version.

A tokenizer named by a profile must be implemented by the runtime. Unknown tokenizer strings or unsupported `tiktoken` encodings make the profile unusable and fail profile loading with `profile_missing`, naming the tokenizer; they never silently fall back to `chars4`. The reference runtime supports the offline encodings `o200k_harmony`, `o200k_base`, `cl100k_base`, `p50k_base`, `p50k_edit`, `r50k_base`, and `gpt2` with the `tiktoken:` prefix. Token estimation treats special-token-looking input as ordinary text.

Limits with other owners have different exhaustion behavior:

| Limit | Owner | Exhaustion |
| --- | --- | --- |
| `loop max N`, `until ... max N`, machine call `max N` | Program, for that loop or invocation | The local iteration or machine call ends. |
| `budget calls/minutes/usd/steps` | Task or machine, constrained by callers | A `budget` pause; the host may extend the named frame. Omitted `calls` has the 0.1 default of 50. |
| `shape` field `max N`, `focus ... max N` | Program, for a constructed value | Truncation, reduction, or the specified strict error. |
| Request size, question count and criteria cap | Selected model profile | Pre-send error, except the permitted `each` chunking. |
| Def recursion depth | Runtime execution rule | `recursion_limit` error. |
| Gate thresholds and machine guards | Program policy | A verdict or enabled-event decision, not resource exhaustion. |

---

## 11. Host contract

### 11.1 Compilation

The compiler turns one `.jev` file into one IR document, a JSON object with `ir_version`, the program's declarations, and a tree of nodes. The IR is the interface between compiler and runtime, and the SDKs never expose it to application code. In the reference implementation the IR is a set of Rust types with `serde` derives, and the JSON form is their serialization; a JSON Schema generated from those types is the IR's published schema (`spec/ir.schema.json`). Two invariants:

- The IR contains every label, level, and threshold as data, so that a host can list a program's judgments, their answer spaces, and its declared capabilities without executing anything.
- The IR records source locations for every node so that pauses and errors can point back to lines.

### 11.2 SDK surface

Both SDKs expose the same surface, named idiomatically for the host language.

```
load(path | source, { paths? })            -> Program
Program.name
Program.inputs                            -> declared inputs
Program.needs                             -> declared capabilities with kinds
Program.judgments                         -> names and answer spaces
Program.judgment(name).run(state, { model?, profiles?, on_log? }) -> record of answers
Program.task(name).start(options)         -> Run
Run.next()                                -> Pause          (JS: async iterable; Python: iterator)
Run.resume(payload)
Run.abort()
Run.inject(capability, message)           -> during `waiting`
Run.events()                              -> stream of recording events as they are written
Run.logs()                                -> stream of this run's `log` lines (section 5.8)
```

`start` options:

```
inputs:   record matching `in`
bind:     record from capability name to adapter; a tool adapter may carry `manifest` (section 9.4)
record:   path to write a recording
replay:   path to read a recording
redact:   bool
model:    Jev model id override
sample:   bool | { seed: number }        (section 6.11)
profiles: path to a profiles file       (section 10.6)
on_log:   callback receiving each `log` line as it is written (section 5.8)
```

A log line, as `Run.logs()`, `on_log` and a standalone judgment's callback deliver it, is `{ level, message, fields, task, source }`, plus `run_id` and `seq` when it comes from a run. `fields` keeps the tagged host JSON of structured values. A replayed line is not delivered again (section 5.8). The option is named idiomatically: `on_log` in Python, `onLog` in JavaScript.

The `paths` option belongs to `load`, because module resolution precedes task start. A legacy empty `paths` array on `task.start` is accepted; a non-empty one is rejected as invalid parameters with a message directing the caller to `program.load`, rather than silently ignored or used to replace a loaded program. The `profiles` option on a standalone judgment is layered over bundled and environment profiles before `model` is resolved, as for task start.

Program metadata describes the public declarations without exposing executable IR. Each input is `{ name, shape }`: a bare shape is `{ shape: "type", name: <type name> }`, and a record shape is `{ shape: "record", fields: [{ name, shape? }] }`, recursively. Each capability is `{ name, kind }`. Each judgment is `{ name, params: [<parameter name>...], shape_hash, results: [...] }`. Every result has `name`, `each` (bool), and `verb` (`"feels"`, `"pick"`, `"rate"`, or `"pick_among"`). A `pick` additionally has `labels: [<label name>...]` in declaration order; a `rate` has `levels: [{ index, name? }...]` in order, with zero-based indices; a `pick_among` has `allow_none` (bool), since its item labels depend on runtime input. A `feels` needs no additional answer-space fields. These metadata objects contain no source spans, question expressions, subject paths, descriptions as expression trees, or request-group annotations. The `program.load` result uses exactly these declaration shapes.

### 11.3 Running a judgment alone

`Program.judgment(name).run(state)` performs one logical judgment request and returns the answers; an individually oversized `each` may split it under section 6.6. It needs no bindings. Every declared parameter must be present in `state` or the call raises `type_error`. The 0.1 runtime selects only declared parameters from that host record: additional keys are ignored and never sent to Jev. A host that wants more context must declare another parameter in the judgment. This is the path for evaluating questions against labelled examples, and a runtime should ship a small harness that takes a judgment name and a JSONL of `{ state, expected }` rows and reports per-question accuracy with the full probabilities on misses.

### 11.4 Answer-shape stability

Once host code reads `j.next.label` or `l is unchanged`, the labels and level names in a program are an API. The compiler emits a `shape hash` per judgment in the IR. A host can pin it and refuse to run a program whose judgment shapes changed, in the same way it would refuse a database migration it did not expect.

### 11.5 Runtime boundary

Version 0.1 runs the runtime as a local process, the `jevscript serve` subcommand of the Rust binary, and the SDKs speak JSON-RPC 2.0 to it over stdio, one JSON object per line. Methods, host to runtime:

```
program.load        { source | path, paths? }            -> { program_id, name, inputs, needs, judgments }
judgment.run        { program_id, name, state, model?, profiles? } -> { answers, logs? }
task.start          { program_id, name, inputs, bindings: [{name, kind, manifest?}], record?, replay?, redact?, model?, sample?, profiles? } -> { run_id }
run.next            { run_id }                           -> pause
run.resume          { run_id, payload }                  -> ok
run.abort           { run_id }                           -> ok
run.inject          { run_id, capability, message }      -> ok
```

Notifications and requests, runtime to host:

```
capability.call     { run_id, capability, verb, args }   -> { result } | { error }
capability.observe  { run_id, capability, handle }       -> { observation }
event               { run_id, event }                    (notification, no reply)
```

`judgment.run` includes `logs`, the judgment's log lines in the order they ran, only when it wrote any. A run's `log` events reach the host as `event` notifications like every other event. Adapters therefore live in the host process, and the runtime never links against tmux, a browser, or an agent CLI. In-process embedding through native bindings (`napi-rs` for Node, `PyO3` for Python) or a WebAssembly build is a later version and does not change this method set; the JSON-RPC methods are the contract, and native bindings expose the same calls.

### 11.6 Command-line tool

The reference implementation ships one binary, `jevscript`, with these subcommands:

| Subcommand | Does |
| --- | --- |
| `compile <file.jev>` | Emits the IR as JSON to stdout, or the compile errors to stderr with source ranges. Exit code 1 on error. |
| `check <file.jev> [--tools <manifest.json>]` | Compiles and reports warnings and errors without emitting IR. With `--tools`, also checks every tool verb against the manifest (section 9.4). |
| `judge <file.jev> <judgment> --state <json>` | Runs one judgment against a state and prints the answers. Needs `TYPESAFE_API_KEY`. |
| `eval <file.jev> <judgment> --cases <jsonl>` | Runs a judgment over `{ state, expected }` rows and reports per-question accuracy with probabilities on misses. |
| `run <file.jev> --input <json> --record <path>` | Runs `main` with adapters supplied as subprocess commands or as built-in stubs, answering pauses on the terminal. |
| `replay <recording.jsonl>` | Replays a recording with no model calls. |
| `serve` | The JSON-RPC stdio server the SDKs use. |
| `setup --agent <codex|claude-code>... [--global] [--copy] [--project <dir>]` | Installs the bundled Jevscript coding-agent Skill offline for explicitly named agents. |

`setup` defaults to the current project directory; `--project` selects an existing
project directory and cannot be combined with `--global`. `--global` selects the
current user's home directory. At least one `--agent` is required; only `codex`
and `claude-code` are supported in 0.1, and repeated flags select both. Project
installation uses `.agents/skills/jevscript` as the canonical Skill directory:
Codex reads it there, and Claude Code reads `.claude/skills/jevscript`.
Global installation uses `~/.agents/skills/jevscript` as the canonical copy,
with agent entries at `~/.codex/skills/jevscript` and
`~/.claude/skills/jevscript`. Agent entries link to the canonical copy by
default; `--copy` makes independent copies. The installed `SKILL.md` bytes
are bundled in the CLI and are identical in the npm and PyPI packages.

`setup` never downloads a Skill, runs a package-manager hook, or selects an
agent implicitly. It refuses a symlink in a destination's parent path, an
unmanaged existing directory, a conflicting link, or changed installed bytes;
it leaves collisions untouched. A same-version rerun reports an already
installed entry. An interrupted setup may leave already completed entries;
the bundled content receipt lets a rerun finish missing owned entries without
replacing a different Skill. The command reports the scope and exact paths.

For `run`, each `--bind NAME=COMMAND` binds one declared capability to a subprocess; `COMMAND` is a host-supplied shell command, never program-generated text. The process stays open for that run. Each request is one JSON line on its stdin: `{ operation: "call", capability, verb, args }` or `{ operation: "observe", capability, handle }`. It replies with one JSON line containing `{ result }`, `{ observation }`, or `{ error: { message, retryable } }`; stdout is protocol-only and diagnostics go to stderr. `args` carries the runtime's positional and named arguments, and values use the same JSON representation as the SDK boundary. Malformed replies and unexpected process exit raise `adapter_error`. The CLI closes and reaps its adapter processes when the run ends.

`run` prints each `log` line on stderr as it is written, as `log <level> <task> <line>:<column>: <message> <fields as JSON>`; stdout stays pauses only. `judge` prints a judgment's log lines on stderr the same way. `replay` prints the same lines from the recording file as execution reaches them, so a redacted payload prints as its redaction marker. It prints a line only after replay has checked its identity: a line that diverged, and anything after it, is never printed.

Built-in demonstration adapters require an explicit `--stub NAME` for each capability. They are visibly identified as stubs; missing bindings still fail with `binding_missing`. Duplicate or undeclared binding names are rejected before execution. `person` confirmation is answered on the terminal using the pause's declared options.

---

## 12. Errors

An error code is the stable machine-readable identity of a failure; its message is contextual and may name the values or declarations involved. A compile diagnostic carries `code`, `severity`, `message`, `span`, and, once known, `file`. A runtime error surfaces as an `error` pause carrying `code`, `message`, `retryable`, and the common pause `source` location.

Every human-facing renderer must make the diagnostic actionable. It shows the primary file and source excerpt with the failing range marked, explains the immediate cause in plain language, gives a concrete correction when one is known, and links to stable documentation for that code. If the failure relates two source locations, such as a duplicate and its first declaration, the renderer also points to the related location when the compiler or runtime has it. Static explanation, repair advice, and documentation links may be looked up by `code` instead of being repeated in every wire payload; source-specific facts remain in `message` and the locations. The reference explanations and fixes are in the [error reference](../docs/error-reference.md), where a code's stable local anchor is `#<code>`.

Compile error codes:

| Code | Cause |
| --- | --- |
| `syntax` | Source that does not match the grammar, when no more specific compile error applies. |
| `indent` | Inconsistent indentation or a tab. |
| `unbounded_loop` | `loop` or `until` without `max`. |
| `pick_no_other` | A `pick` without an `other` or `none` label. |
| `pick_arity` | Fewer than two or more than eight labels. |
| `rate_arity` | Fewer than two or more than ten levels. |
| `rate_bare_degree` | A level that is only a number or a degree word. |
| `subject_not_path` | A judgment subject that is not a variable or field path. |
| `judgment_side_effect` | A capability call, gate, or loop inside a `judgment`. |
| `def_side_effect` | A capability call, gate, or pause inside a `def`. |
| `verb_unknown` | A verb not defined for the capability's kind (not raised for `tool`). |
| `verify_twice` | More than one `verify` in a task. |
| `uppercase_identifier` | An identifier containing an uppercase letter. |
| `unassigned_read` | A variable read before assignment. |
| `use_not_found` | The `use` path did not resolve. |
| `use_cycle` | Modules import each other. |
| `use_has_inputs` | An imported file declares `in` or `out`. |
| `use_needs_unmapped` | A library `needs` has no entry in the `with` clause, or the kinds differ. |
| `use_private` | A reference to a `_`-prefixed name in another module. |
| `main_has_params` | `task main` declares parameters. |
| `verb_arity` | A tool call with the wrong number of positional arguments for its declared signature. |
| `machine_unknown_state` | A `->` target that is not a declared state. |
| `machine_no_done` | A machine with no terminal state. |
| `event_no_description` | An `on` line without a description text. |
| `machine_gate` | A `gate` inside a machine action block. |
| `pick_among_each` | `each` combined with `pick among`. |
| `gate_args` | A `gate` without both `risk` and `confidence` (7.6). |
| `duplicate_name` | A repeated unit name, module alias, state name within a machine, event name within a state, result name within a judgment, detail key within one detail block, gate argument, gate arm, or field name within one `log`. These names and keys must be unique in their respective scopes. |
| `assign_immutable` | An assignment to an input (3.2) or a capability name (3.4). |
| `loop_control_outside` | `break` or `continue` outside a loop. |
| `log_level` | A `log` whose level is not `debug`, `info`, `warn` or `error`, or a `log` written without a level (5.8). |
| `log_in_question` | A `log` inside a judgment expression's subject index, condition, labels, levels, detail or `pick among` question (5.8). |

Every diagnostic also names the file it was found in, so a library's errors are reported against the library (3.9).

Compile warning codes, which never fail a compile:

| Code | Cause |
| --- | --- |
| `bare_prob_condition` | A `prob` used bare in a condition (4.1). |
| `uncapped_field` | A `shape` or `observe` field without `max` whose value is not a number, bool or short list (7.2). |
| `prelude_shadowed` | A user unit with the same name as a prelude def (3.9). |
| `machine_unreachable_done` | A machine state with no path to any terminal state (7.8). |
| `detail_ignored` | A detail key in a position it does not apply to (6.8). |
| `log_shadowed` | A unit named `log`, which turns the `log` expression off in its file (5.8). |

Runtime error codes:

| Code | Retryable | Cause |
| --- | --- | --- |
| `binding_missing` | no | A `needs` was not bound at start. |
| `state_too_large` | no | Section 6.10. |
| `jev_unavailable` | yes | Network failure, a 5xx, or a 429 rate limit from Jev. |
| `jev_rejected` | no | Any other 4xx from Jev, with the message. |
| `verb_missing` | no | A `tool` verb the adapter did not implement. |
| `adapter_error` | adapter decides | The adapter raised. |
| `type_error` | no | A runtime type mismatch. |
| `recursion_limit` | no | A `def` recursed past the limit. |
| `replay_diverged` | no | Section 10.4. |
| `pick_too_many` | no | A `pick among` list longer than the profile's criteria cap. |
| `profile_missing` | no | No usable profile for the selected model id, including an invalid profile or unsupported tokenizer. |
| `no_enabled_events` | no | Raised as an `escalate` pause, not an error, when a machine state has no enabled event (section 7.8). Listed here for completeness. |

---

## 13. Grammar

EBNF. Indentation is handled by the lexer, which emits `INDENT`, `DEDENT`, and `NEWLINE` tokens as Python's does. `NAME` is an identifier, `TEXT` a text literal, `NUMBER` a numeric or percent literal.

```
program      = "program" NAME NEWLINE { use_decl } { decl } { unit } ;

use_decl     = "use" TEXT "as" NAME [ "with" mapping { "," mapping } ] NEWLINE ;
mapping      = NAME [ ":" NAME ] ;

decl         = "in" NAME ":" shape NEWLINE
             | "out" NAME NEWLINE
             | "needs" NAME ":" kind [ ":" NEWLINE INDENT { signature } DEDENT ] NEWLINE ;
signature    = NAME "(" [ NAME { "," NAME } ] ")" "->" ret_type NEWLINE ;
ret_type     = "text" | "number" | "bool" | "list" | "record" | "handle" | "none" ;
shape        = "text" | "number" | "bool" | "list" | "record"
             | "{" [ field_decl { "," field_decl } ] "}" ;
field_decl   = NAME [ ":" shape ] ;
kind         = "agent" | "person" | "llm" | "tool" ;

unit         = judgment | task | def | machine ;

judgment     = "judgment" NAME "(" [ NAME { "," NAME } ] ")" ":" NEWLINE
               INDENT { NAME "=" judge_expr NEWLINE | log_expr NEWLINE } DEDENT ;

task         = "task" NAME [ "(" [ param { "," param } ] ")" ] [ "budget" budget_list ] [ "thresholds" threshold_list ] ":" block ;
budget_list  = budget_item { "," budget_item } ;
budget_item  = ( "calls" | "minutes" | "usd" | "steps" ) NUMBER ;
threshold_list = threshold_item { "," threshold_item } ;
threshold_item = NAME NUMBER ;

def          = "def" NAME "(" [ param { "," param } ] ")" ":" block ;

machine      = "machine" NAME "(" [ param { "," param } ] ")" [ "budget" budget_list ] [ "thresholds" threshold_list ] ":" NEWLINE
               INDENT [ "goal" TEXT NEWLINE ] [ "initial" NAME NEWLINE ] [ observe ] { state } DEDENT ;
observe      = "observe" ":" NEWLINE INDENT { shape_field } DEDENT ;
state        = "state" NAME "done" NEWLINE
             | "state" NAME ":" NEWLINE INDENT { transition } DEDENT ;
transition   = "on" NAME TEXT "->" NAME [ "when" expr ] [ "risky" ] ( NEWLINE | ":" block ) ;
param        = NAME [ "=" expr ] ;

block        = NEWLINE INDENT { statement } DEDENT ;

statement    = assign | call_stmt | if_stmt | for_stmt | loop_stmt | until_stmt
             | gate_stmt | shape_assign | log_expr NEWLINE | "return" [ expr ] NEWLINE
             | "continue" NEWLINE | "break" NEWLINE
             | "stop" TEXT NEWLINE | "escalate" TEXT NEWLINE ;

assign       = target "=" ( judge_expr | command | expr ) NEWLINE ;
target       = NAME { "." NAME } ;          (* the first NAME may be a module alias *)
call_stmt    = command NEWLINE ;
command      = target [ cmd_args ] ;
cmd_args     = cmd_arg { "," cmd_arg } [ [ "," ] "using" expr ] ;
cmd_arg      = NAME expr | expr ;

if_stmt      = "if" expr ":" block { "elif" expr ":" block } [ "else" ":" block ] ;
for_stmt     = "for" NAME [ "," NAME ] "in" expr ":" block ;
loop_stmt    = "loop" "max" NUMBER ":" block ;
until_stmt   = "until" ( "verify" "(" expr ")" | expr ) "," "max" NUMBER ":" block ;

gate_stmt    = "gate" gate_arg { "," gate_arg } ":" NEWLINE INDENT { gate_arm } DEDENT ;
gate_arg     = ( "risk" | "confidence" | "done" ) expr ;
gate_arm     = ( "proceed" | "confirm" | "escalate" | "stop" ) "->" ( statement | block ) ;

shape_assign = NAME "=" "shape" [ "strict" ] ":" NEWLINE INDENT { shape_field } DEDENT ;
shape_field  = NAME ( command | expr ) [ "," "max" NUMBER [ "," "tail" ] ] NEWLINE ;

judge_expr   = [ "each" ] subject judge_verb
             | subject "pick" "among" TEXT [ ":" NEWLINE INDENT [ "by" NAME NEWLINE ] [ "none" NEWLINE ] DEDENT ] ;
subject      = NAME { "." NAME | "[" expr "]" } ;
judge_verb   = "feels" TEXT [ detail_block ]
             | "pick" ":" NEWLINE INDENT { pick_label | shared_detail } DEDENT
             | "rate" ":" NEWLINE INDENT { rate_level | shared_detail } DEDENT ;
pick_label   = NAME [ TEXT | detail_block ] NEWLINE | ( "other" | "none" ) NEWLINE ;
shared_detail = "focus" TEXT NEWLINE | "compare" list NEWLINE | "sample" ( "true" | "false" ) NEWLINE ;
rate_level   = [ NAME ] TEXT NEWLINE ;
detail_block = ":" NEWLINE INDENT { detail_item } DEDENT ;
detail_item  = ( "focus" | "note" ) TEXT NEWLINE
             | "sample" ( "true" | "false" ) NEWLINE
             | "compare" list NEWLINE
             | ( "yes" | "no" | "examples" ) list NEWLINE
             | ( "what" | "not_for" ) TEXT NEWLINE ;

expr         = or_expr ;
or_expr      = and_expr { "or" and_expr } ;
and_expr     = not_expr { "and" not_expr } ;
not_expr     = "not" not_expr | cmp_expr ;
cmp_expr     = add_expr [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) add_expr | "is" NAME ] ;
add_expr     = mul_expr { ( "+" | "-" ) mul_expr } ;
mul_expr     = unary { ( "*" | "/" | "%" ) unary } ;
unary        = "-" unary | postfix ;
postfix      = primary { "." NAME | "[" expr "]" | "(" [ fn_args ] ")" } ;
fn_args      = fn_arg { "," fn_arg } [ [ "," ] "using" expr ] ;
fn_arg       = NAME ":" expr | NAME expr | expr ;
primary      = NUMBER | TEXT | "true" | "false" | "none" | NAME
             | list | record | "(" expr ")" | comprehension
             | "focus" expr "on" TEXT "," "max" NUMBER
             | "trail" NUMBER
             | log_expr ;
log_expr     = "log" log_level expr [ record ] ;
log_level    = "debug" | "info" | "warn" | "error" ;
list         = "[" [ expr { "," expr } ] "]" ;
record       = "{" [ rec_field { "," rec_field } ] "}" ;
rec_field    = NAME [ ":" expr ] ;
comprehension = "[" expr "for" NAME [ "," NAME ] "in" expr [ "if" expr ] "]" ;
```

Builtins such as `count(off, above 0.6)` and `top(chunks, by keep, n 3)` use the `NAME expr` named-argument form inside function-form calls, which is why `fn_arg` admits it. Where an argument or a statement begins with `log`, a word and then the start of an expression, it is a `log_expr` rather than a named argument or a command-form call, unless the file declares a unit named `log` (section 5.8). `using` is the named argument of `llm.write` (section 9.3) and may follow the last argument without a comma, as section 14.3 writes it. A `pick:` or `rate:` block takes its detail (section 6.8) as `shared_detail` lines among the labels or levels; `note` is not one of them because `note "..."` would read as a label. A reserved word is accepted where a field name or an argument name is expected (`dev.stop`, `spawn in tree`, `review(dev, max 20)`).

Contextual words are recognized in these positions. Outside those positions they remain ordinary identifiers (section 2.5):

| Word | Grammar position |
| --- | --- |
| `as`, `with` | Module alias and capability mapping in `use_decl`. |
| `using` | The optional named context argument at the end of a command or function call. |
| `above`, `below` | Named threshold arguments of `count`. |
| `by` | Description field after `pick among`, or paired scores in `top`. |
| `prompt` | Named argument of `agent.spawn`. |
| `done` | Terminal machine state marker and optional gate argument. |
| `risky` | Machine transition marker after its target and optional guard. |
| `sample` | Judgment detail item or shared `pick`/`rate` detail line. |
| `goal`, `observe` | Machine header fields. |
| `strict` | After `shape` to reject overflow. |
| `tail` | Shape field overflow policy after `, max N,`. |
| `log` | The start of a `log_expr`, followed by a word and the start of an expression, in a file with no unit named `log` (5.8). |
| `debug`, `info`, `warn`, `error` | The level directly after `log`. |

---

## 14. Examples

### 14.1 A coding harness

```
program fix_issue

in  issue: { title, body, branch }
out pr_url

needs claude: agent
needs tree:   tool
needs me:     person

judgment read_agent(summary, files, tests, recent):
  claims_done = summary feels "states the work is complete":
    focus "An explicit statement that the work is finished, not a plan."
  off_scope   = each files feels "is unrelated to the issue: {issue.title}"
  next        = summary pick:
    keep_working  "making progress and needs nothing"
    stuck         "looping or unsure how to proceed"
    needs_me      "asks a question only a person can answer"
    other

task main budget calls 40, minutes 30 thresholds risk_confirm 0.2, min_confidence 0.5:
  tree.create issue.branch
  dev = claude.spawn in tree, prompt issue.body

  until verify(tree.tests_pass), max 6:
    dev.wait idle, minutes 5

    obs = shape:
      summary  focus dev.observe.last_message on "what the agent says it did", max 2k
      files    tree.diff.files, max 500
      tests    tree.test_summary, max 300
      recent   trail 6

    j = read_agent obs

    if stuck(obs.recent) or j.next is stuck:
      dev.send "Stop. In three lines, what is blocking you?"
      continue

    if j.next is needs_me:
      answer = me.ask obs.summary
      dev.send answer.text
      continue

    gate risk max(j.off_scope), confidence j.next.confidence:
      confirm  -> me.ask "Agent touched {count(j.off_scope, above 0.6)} unrelated files. Continue?"
      escalate -> me.take_over
      proceed  ->
        if j.claims_done > 0.8 and not tree.tests_pass:
          dev.send "Tests fail:\n{obs.tests}"

  dev.stop
  pr_url = tree.open_pr
```

### 14.1a The same harness, split into a library and a program

`lib/agent_loop.jev`, a library with no `in` or `out`:

```
program agent_loop

needs claude: agent
needs tree:   tool
needs me:     person

judgment read_agent(summary, files, tests, recent, title):
  claims_done = summary feels "states the work is complete"
  off_scope   = each files feels "is unrelated to the issue: {title}"
  next        = summary pick:
    keep_working  "making progress and needs nothing"
    stuck         "looping or unsure how to proceed"
    needs_me      "asks a question only a person can answer"
    other

task watch(dev, title) budget calls 30 thresholds risk_confirm 0.2, min_confidence 0.5:
  until verify(tree.tests_pass), max 6:
    dev.wait idle, minutes 5
    obs = shape:
      summary  focus dev.observe.last_message on "what the agent says it did", max 2k
      files    tree.diff.files, max 500
      tests    tree.test_summary, max 300
      recent   trail 6
    j = read_agent(summary: obs.summary, files: obs.files, tests: obs.tests, recent: obs.recent, title: title)
    if stuck(obs.recent) or j.next is stuck:
      dev.send "Stop. In three lines, what is blocking you?"
      continue
    gate risk max(j.off_scope), confidence j.next.confidence:
      confirm  -> me.ask "Agent touched {count(j.off_scope, above 0.6)} unrelated files. Continue?"
      escalate -> me.take_over
      proceed  ->
        if j.claims_done > 0.8 and not tree.tests_pass:
          dev.send "Tests fail:\n{obs.tests}"
  return tree.tests_pass
```

`fix_issue.jev`, the program:

```
program fix_issue

use "./lib/agent_loop.jev" as harness with claude, tree, me

in  issue: { title, body, branch }
out pr_url

needs claude: agent
needs tree:   tool
needs me:     person

task main budget calls 40, minutes 30:
  tree.create issue.branch
  dev = claude.spawn in tree, prompt issue.body
  passed = harness.watch(dev, issue.title)
  dev.stop
  if passed:
    pr_url = tree.open_pr
```

The library declares the capabilities it needs and the program hands them over by name. The host still binds three adapters and sees one flat program whose judgments are `harness.read_agent` and whose pauses name `harness.watch` as their source.

### 14.2 A judgment-only program

```
program inbox_triage

in message: text

judgment triage(message):
  urgent  = message feels "needs a response within the hour"
  owner   = message pick:
    code      "asks for a code change or reports a bug"
    customer  "a customer asking for help or a reply"
    schedule  "asks to find or move a meeting time"
    other
  risky   = message feels "acting on it could send, pay, delete or commit something"
  effort  = message rate:
    trivial  "answerable in one line"
    hour     "an hour of focused work"
    project  "a multi-day project"
```

A host runs `triage` directly, with no bindings, and reads four typed answers from one request.

### 14.3 A meta-agent dispatcher

```
program chief_of_staff

in  events: list
out dispatched

needs claude: agent
needs writer: llm
needs me:     person
needs mux:    tool

task main budget calls 60, minutes 20 thresholds risk_confirm 0.2, min_confidence 0.6:
  dispatched = []
  for ev in events:
    e = shape:
      subject  ev.subject, max 100
      body     ev.body, max 1500
      source   ev.source

    urgent = e.body feels "needs a response within the hour"
    owner  = e.body pick:
      code      "asks for a code change or reports a bug"
      customer  "a customer asking for help or a reply"
      schedule  "asks to find or move a meeting time"
      other
    risky  = e.body feels "acting on it could send, pay, delete or commit something"
    log info "routed {e.subject}" { owner: owner.label, confidence: owner.confidence, urgent }

    gate risk risky, confidence owner.confidence:
      proceed ->
        if owner is code:
          pane = mux.pane "cos-{hash(e.subject)}"
          claude.spawn in pane, prompt e.body
          dispatched = dispatched + [{ subject: e.subject, to: "claude" }]
        elif owner is customer:
          draft = writer.write "Draft a reply. Commit to nothing. No signature." using e.body
          me.notify "Draft for {e.subject}:\n{draft}"
        elif owner is schedule:
          mux.enqueue "calendar", e
        else:
          me.notify "Unrouted: {e.subject}"
      confirm  -> me.ask "This looks risky: {e.subject}. Handle it yourself?"
      escalate -> me.notify "Unsure who owns: {e.subject}"

    if urgent > 0.8:
      me.notify "Urgent: {e.subject}"
```

Every judgment in the loop body over `e` batches into one request per event: three questions, one call. The `log` line after them records the routing for the host; it reads the answers but is not part of the request (section 5.8).

### 14.4 A review machine

```
program review_loop

needs claude: agent
needs tree:   tool:
  tests_pass() -> bool
  test_summary() -> text
needs me:     person

machine review(dev) budget calls 30 thresholds min_confidence 0.6, risk_confirm 0.5:
  goal "Get the agent's change to a passing, reviewable state without doing its work for it."

  observe:
    summary  focus dev.observe.last_message on "what the agent says it did", max 1500
    tests    tree.test_summary, max 300
    status   dev.observe.status

  state working:
    on finished "the agent reports it is finished and tests pass" -> reviewing when tree.tests_pass
    on claims_done "the agent reports it is finished but tests do not pass" -> nudging when not tree.tests_pass:
      dev.send "Tests fail:\n{obs.tests}"
    on stuck "the agent is looping or unsure how to proceed" -> nudging:
      dev.send "Stop. In three lines, what is blocking you?"
    on asks "the agent asks a question only a person can answer" -> waiting_on_me

  state nudging:
    on resumed "the agent has started working again" -> working

  state waiting_on_me:
    on answered "a reply has been passed to the agent" -> working:
      answer = me.ask obs.summary
      dev.send answer.text

  state reviewing:
    on approved "the change is ready to open as a pull request" -> approved when tree.tests_pass
    on rejected "the change needs more work" -> working risky:
      dev.send "Review found problems. Address them."

  state approved done

task main:
  dev = claude.spawn prompt "Fix the failing test in src/parser.rs"
  r = review(dev, max 20)
  if r.done and r.verified:
    me.notify "Ready: {r.steps} steps"
  else:
    me.notify "Stopped in {r.state} after {r.steps} steps"
```

Every step sends the state name, the goal, the three shaped fields and the last six events, and asks one Choice over the enabled events plus `stay`. The `when` guards keep `finished` and `approved` off the menu until tests actually pass, so the terminal state can only be entered on code's say-so, and the result reports `verified: true`.

---

## 15. Conformance

A conforming implementation:

1. Compiles every example in section 14 without error and rejects every program in the compiler's negative test suite with the listed code.
2. Batches judgments exactly as section 6.6 describes, verifiable from the recording's `request` events.
3. Sends Jev exactly the state section 6.9 describes and nothing more, verifiable from the recording.
4. Replays any recording it produced with identical pauses in identical order.
5. Exposes the SDK surface in section 11.2 over the JSON-RPC methods in section 11.5.
6. Offers a machine exactly the enabled events plus `stay` at every step, verifiable from the recording's `machine_step` events.
7. Reads model request/token/question/criteria limits and model prices from the selected profile (section 10.6), not from provider-specific literals in runtime code. Language arities, the 0.1 default call budget, loop bounds, shape caps, thresholds and the def recursion limit have different owners and are not model-profile fields.
8. Renders every compile and runtime diagnostic with the marked primary source location, cause, repair guidance when known, and a stable documentation link for its code; related locations are included when available (section 12).
9. Records every evaluated `log` as one `log` event, never while a Jev request is unanswered, and replays recorded logs with the same level, message and fields in the same order without emitting them to the host again (section 5.8), verifiable from the recording.

---

## 16. Decision log

Every question that was open in an earlier draft, with the decision and where it now lives. Nothing is open in 0.1. Reopen an entry by editing the section it points to, not this log.

| Decided | Question | Decision | Section |
| --- | --- | --- | --- |
| 2026-09-26 | Coding-agent Skill setup | Make `jevscript setup` an explicit offline CLI command with required Codex/Claude Code agent selection, project default and opt-in global scope, skills.sh-compatible paths, safe collision handling and content receipts. Bundle identical Skill bytes in the CLI, npm and PyPI; no install hook. | 11.6 |
| 2026-09-21 | Nested budgets | A callee's effective budget is the smaller of its own and the caller's remainder; usage counts against the whole stack; agents' own spend is reported by adapters, summed for information, never gated by the runtime. | 7.1 |
| 2026-09-21 | Choosing among runtime values | `pick among` chooses one element of a list, with `by` for the description field and an optional `none`; capped by the profile's criteria limit. | 6.4a |
| 2026-09-21 | Sampling | Per judgment with `sample true`, or per run with the `sample` option; affects only which label or level is reported; draws are recorded, so sampled runs replay. | 6.11 |
| 2026-09-21 | Typed tool adapters | Three optional layers: a signature block on `needs`, a manifest at bind time checked before start, and the run-time `verb_missing`. | 9.4 |
| 2026-09-21 | Model profiles | A JSON profile per model id holding limits, caps, tokenizer and prices; bundled defaults, overridable by file or environment variable; nothing hard-coded. | 10.6 |
| 2026-09-21 | Machines | A fourth unit in 0.1. States, described events, code guards, `stay`, `risky`, terminal states, `verified` only through a guarded entry. | 7.8, 14.4 |
| 2026-09-21 | Grammar gaps found by the parser | `using` follows the last argument with or without a comma; a label's detail block is `NAME detail_block` (one colon); `pick:` and `rate:` blocks take `focus`, `compare` and `sample` lines as their detail and `note` is `feels`-only; the `until` condition is required; reserved words are accepted as field and argument names. | 6.8, 13 |
| 2026-09-21 | Wire details found by the runtime | A 429 from Jev is `jev_unavailable` and retryable, as TypeSafe documents it; the bare escape label is sent with a fixed description; an indexed subject is sent as its list with `null` at unjudged positions; `compare` paths are placed like subjects. | 6.3, 6.9, 12 |
| 2026-09-21 | Findings from the compiler | Shape hashes cover a unit's own name, never its alias; section 12 gains codes for the prose-only errors (`gate_args`, `duplicate_name`, `assign_immutable`, `loop_control_outside`) and a table of warning codes; diagnostics carry a file; a `tail` overflow policy is written `, tail` after `max`. | 3.9, 7.2, 12, 13 |
| 2026-09-21 | Machine escalation resumption | Explicitly resuming a machine escalation consumes the step and re-observes without executing the selected action; no-enabled-event escalation similarly consumes a step without issuing a request. | 7.8 |
| 2026-09-21 | Standalone replay execution metadata | The start event embeds linked IR, the resolved profile and run sampling option; standalone replay uses them without source files or ambient profile resolution and refuses incomplete metadata instead of guessing. | 10.3, 10.4, 11.6 |
| 2026-09-21 | Recording file lifecycle | A recording is one run with a leading start and contiguous sequence; validate its envelope and supported IR before replay. File destinations are new and never appended or overwritten. Record and replay are mutually exclusive; changing a replay answer continues live without producing a suffix-only file. | 10.1, 10.3, 10.4 |
| 2026-09-21 | Replayable host cancellation | Record an explicit `abort` before stopped/end, including cancellation before the first step or at an open pause. In-flight capability abort waits for and records the current call, then stops before further execution. Replay consumes the same cancellation boundary automatically. | 9.6, 10.3, 10.4, 11.2 |
| 2026-09-21 | Failed external attempts | Record `effect_error` with operation, exact call identity and runtime error before retry, pause or abort, including client initialization failure; replay returns that failure without a live callback or current credentials. | 10.3, 10.4, 15 |
| 2026-09-21 | Public program metadata | Fix the declaration DTOs for inputs, capability names/kinds and judgment answer spaces; program.load and SDK properties expose those, never raw IR nodes or expressions. | 11.1, 11.2, 11.5 |
| 2026-09-21 | Bundled provider profile provenance | Use the documented concrete model id `jev-1.13.0` and Choice cap 255; identify the undocumented question cap as conservative runtime policy and `chars4` as an estimator. Record date and authoritative sources alongside the bundle. | 10.6 |
| 2026-09-21 | Unsupported profile tokenizers | Reject unknown tokenizer strings and unsupported encodings with `profile_missing`; never substitute `chars4`. Supported tiktoken encodings operate offline and encode input as ordinary text. | 10.6, 12 |
| 2026-09-21 | Duplicate declarations and keys | Module aliases, judgment results, detail keys, gate arguments and gate arms are unique in their respective scopes and use `duplicate_name`, alongside units, states and events. Grammar rejection uses `syntax` when no specific code applies. | 12, 13 |
| 2026-09-21 | CLI adapter subprocesses | `--bind NAME=COMMAND` uses a persistent JSON-line call/observe subprocess; `--stub NAME` explicitly selects demonstration adapters. Missing bindings never silently become stubs. | 11.6 |
| 2026-09-21 | Load-time module roots and standalone profiles | `paths` belongs to `load`/`program.load`, before linking and metadata; non-empty legacy task-start roots are rejected. Standalone judgment runs accept `profiles` as well as `model`. | 3.9, 11.2, 11.5 |
| 2026-09-21 | Request caps and named state | Only an individually oversized `each` permits splitting a logical group; other oversized groups fail with `state_too_large`. Named judgment chunks contain every declared parameter in full, while inline chunks contain only their paths. | 6.5, 6.6, 6.9 |
| 2026-09-21 | Redaction and exact replay | A hash cannot reproduce arbitrary observation-driven control flow. Redacted file recordings have an explicitly sensitive owner-only full replay companion, validated against the primary file; missing data fails rather than substituting observations. Wait observations are redacted too. | 10.3, 10.4 |
| 2026-09-21 | Machine request and no-decision identity | Machine Choice uses id `event`, subject `obs`, and compares state, goal and recent. No-enabled-event steps record an explicit empty-distribution stay sentinel; gated decisions retain their chosen label and record the permitted destination. | 7.8 |
| 2026-09-21 | Verify scope and bypassed execution | The single verify loop is directly in its task body so final evaluation has task scope. Returning before reaching it yields unverified completion without evaluating the skipped condition. | 7.4 |
| 2026-09-21 | Recording payload marker collisions | Exact redaction-marker and escaped-full shapes in ordinary data use a single `$recorded_full` escape; normal payload JSON remains unchanged and decoding unwraps once. | 10.3 |
| 2026-09-21 | Machine action control flow | Return is valid only in defs and tasks, not machine actions; break and continue target explicit action loops. Completed decisions blocked by a gate enter the effective history as stay, while machine_step retains the model choice. | 5.6, 7.8 |
| 2026-09-21 | Packages | None in the language. A module is a file; distribution is the host's package manager or git; a JS or Python package that ships `.jev` files registers a search root through the SDK `paths` option. | 3.9 |
| 2026-09-22 | Clarifying existing value and machine-call semantics | Document multiline text, contextual words, numeric-text conversion, the concrete `choice` and `level` shapes, interval notation, provider-profile limits, subject examples, and the existing ability of machine actions to call tasks and machines under nested budgets. | 2.5-2.7, 4, 5.1, 5.7, 6.1, 6.3-6.4, 6.10, 7.8 |
| 2026-09-22 | Actionable diagnostics | Keep compact stable error codes and source-specific messages on machine boundaries; require human renderers to mark the source, explain the cause, suggest a concrete correction, and link to the code's error reference. Related source locations are shown when available. | 12; `docs/error-reference.md` |
| 2026-09-22 | Specification document boundaries | Keep this file as the sole 0.1 authority while semantic review is active. A future split gives language, runtime/recording, host, provider/profile, and schema contracts one normative home each and preserves an authority index and section migration map. | Introduction; `docs/specification-boundaries.md` |
| 2026-09-23 | Judgment and value clarifications | Define code and the before-request answer-space boundary; document indexed subjects, every abstract request form, ordinary versus tagged host JSON, the 0.1 provider/fallback confidence provenance, and language arities separately from model-profile caps. No provider-neutral execution contract is implied. | 1, 4, 6.1-6.12, 10.6, 11.3 |
| 2026-09-23 | Completion and execution clarifications | Host-started task return expressions are discarded while `out` values reach `done`; `verified` reports a satisfied declared predicate or guarded machine entry, whose evidence may be model-derived. The 50-call default is a 0.1 language budget, and the def recursion limit does not bound task/machine recursion. | 5.6-5.7, 7.1-7.4, 7.8, 8, 10.2, 15 |
| 2026-09-23 | Capability and host boundary clarifications | Distinguish immutable input data from host-bound authority, define shape field expressions and host-owned lifecycle integration, and specify that `llm.write using` forwards the evaluated value unchanged to its adapter. A portable context projection remains a later contract. | 3.2-3.4, 7.2, 9.3-9.6, 13 |
| 2026-09-26 | Logging expression | Add `log <level> <expr> [record]` as an expression and statement in every unit. It returns its value, never reaches Jev, never counts against a budget, is not a batching boundary or step, and records one `log` event. The IR gains the `log` expression node and judgment `logs`; the recording gains the `log` event kind. Both land before the 0.1 tag, under IR version `0.1`. | 5.8, 10.3, 11.2, 11.5, 12, 13, 15 |
| 2026-09-26 | Log message and fields | The logged expression is the message: its text form is the line's message and its value is the result, so the message is never a separate, optional slot. Structured fields are one record literal after the value, with unique names and the `{ name }` shorthand. Levels are `debug`, `info`, `warn`, `error`; any other word is `log_level`. | 5.8, 12 |
| 2026-09-26 | Log precedence and keyword status | The logged expression extends as far right as an expression can, as `focus`'s text does, and ends at a record literal, which is the fields. `log` and the level words are contextual, not reserved, so existing names stay valid. A file that declares a unit named `log` has no log expression, so its existing command-form calls such as `log info x` keep their meaning; the compiler warns `log_shadowed`. | 2.5, 5.8, 12, 13 |
| 2026-09-26 | Logs in judgments and questions | A judgment block may hold `log` lines, which run after its one request is answered and read the results above them; the shape hash ignores them. A `log` inside a question is `log_in_question`, because questions are built without effects. A standalone judgment run returns its log lines with the answers; they may call defs and judgments through its client and read `now()` and `random()` live, since nothing is recorded. | 5.8, 6.7, 11.3, 11.5, 12 |
| 2026-09-26 | Log replay and redaction | A log's level, message and fields are replay identity; a difference is `replay_diverged`. Replayed logs are checked, not re-emitted to the host. Under `redact`, the primary recording stores the message and each field value as markers, keeping level, field names, task and source; the host's live stream carries logs in full, as it does request state. `jevscript replay` prints only lines whose identity replay has checked. | 5.8, 10.3, 10.4, 11.6 |
