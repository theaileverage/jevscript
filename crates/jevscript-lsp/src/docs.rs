//! Hover text for the words the language defines.
//!
//! Every entry is a summary of the spec's own text and names the section it
//! comes from, so a hover is a pointer into
//! `spec/jevscript-language-specification.md`, not a second definition. When
//! the spec changes, these follow it.

use jevscript_syntax::ErrorCode;
use jevscript_syntax::ast::CapabilityKind;

/// One documented word: how it is written, what it means, where the spec
/// defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Doc {
    /// The form, as the spec writes it.
    pub signature: &'static str,
    /// What it does, in the spec's terms.
    pub summary: &'static str,
    /// The spec section.
    pub section: &'static str,
}

const fn doc(signature: &'static str, summary: &'static str, section: &'static str) -> Doc {
    Doc {
        signature,
        summary,
        section,
    }
}

/// The reserved words of spec section 2.5 and the contextual words listed
/// with them.
pub fn keyword(word: &str) -> Option<Doc> {
    Some(match word {
        "program" => doc(
            "program <name>",
            "Names the program; hosts load it by this name. A source file holds exactly one program.",
            "3.1",
        ),
        "use" => doc(
            "use \"<path>\" as <alias> with <mapping>",
            "Imports another `.jev` file as a module. Its exported units are reached as `<alias>.<name>`; every `needs` of the library must be mapped in `with`.",
            "3.9",
        ),
        "as" => doc(
            "use \"<path>\" as <alias>",
            "Names the module a `use` imports. An alias cannot be a reserved word.",
            "3.9",
        ),
        "with" => doc(
            "use ... with <inner>[: <outer>], ...",
            "Binds a library's `needs` to this program's capabilities. Nothing is bound implicitly, and kinds must match.",
            "3.9",
        ),
        "in" => doc(
            "in <name>: <shape>",
            "At the top level, declares an immutable input the host supplies when a task starts. In `for x in xs` it separates the loop variable, and in `spawn in <handle>` it is a named argument.",
            "3.2",
        ),
        "out" => doc(
            "out <name>",
            "Declares an output. A task assigns it, and the run's `done` pause carries every declared output; one never assigned is `none`.",
            "3.3",
        ),
        "needs" => doc(
            "needs <name>: agent | person | llm | tool",
            "Declares a capability the host binds. Its kind fixes which verbs the compiler accepts; capability names cannot be reassigned.",
            "3.4",
        ),
        "judgment" => doc(
            "judgment <name>(<param>, ...):",
            "A named block of judgment assignments over its parameters, which are the state Jev sees. It compiles to exactly one request and cannot call capabilities or loop.",
            "6.7",
        ),
        "task" => doc(
            "task <name>(<param>, ...) budget ... thresholds ...:",
            "A block that can observe, judge, gate and act. `main` is the entry point and takes no parameters; other tasks are called like functions under the caller's remaining budget.",
            "7",
        ),
        "def" => doc(
            "def <name>(<param>, <param> = <default>, ...):",
            "A helper without capability calls, gates or pauses. It may judge, and it may recurse up to the runtime's depth limit.",
            "8",
        ),
        "machine" => doc(
            "machine <name>(<param>, ...) budget ... thresholds ...:",
            "A state machine. At every step Jev picks one of exactly the enabled events, plus `stay`; guards and actions are code.",
            "7.8",
        ),
        "return" => doc(
            "return <expr>?",
            "Leaves the current def or task. A caller receives the value; a host-started task reports its `out` values instead.",
            "5.6",
        ),
        "feels" => doc(
            "<subject> feels \"<condition>\"",
            "Asks a yes/no question. Returns a `prob`: the probability the condition holds. Near 0.5 means unsure, not \"somewhat\".",
            "6.2",
        ),
        "pick" => doc(
            "<subject> pick:\n  <label> \"<description>\"\n  ...\n  other",
            "Asks Jev to choose one labelled description. Two to eight labels, exactly one bare `other` or `none`. Returns a `choice`; test it with `c is <label>`.",
            "6.3",
        ),
        "rate" => doc(
            "<subject> rate:\n  [<name>] \"<situation>\"\n  ...",
            "Places the subject on a spectrum of two to ten standalone situations, low to high. Returns a `level`; named levels allow `l is <name>`.",
            "6.4",
        ),
        "among" => doc(
            "<list> pick among \"<question>\":",
            "Chooses one element of a runtime list. Returns a `choice` with `index` and `item`. Cannot be combined with `each`.",
            "6.4a",
        ),
        "each" => doc(
            "each <list> feels | pick | rate ...",
            "Asks the same question once per element, in one request, and returns the answers in order.",
            "6.5",
        ),
        "other" => doc(
            "other",
            "The escape label of a `pick`, written bare. Jev chooses it when no other description fits.",
            "6.3",
        ),
        "on" => doc(
            "on <event> \"<description>\" -> <state> [when <expr>] [risky]",
            "Declares a machine event enabled in the enclosing state. In `focus <text> on \"<purpose>\"` it names the purpose.",
            "7.8",
        ),
        "shape" => doc(
            "<name> = shape [strict]:\n  <field> <expr>, max <tokens>",
            "Builds a record whose fields are token-capped. An overflow keeps the head (or `tail`) and warns; `shape strict` makes it an error pause.",
            "7.2",
        ),
        "strict" => doc(
            "shape strict:",
            "Makes a field overflow an `error` pause instead of a truncation.",
            "7.2",
        ),
        "focus" => doc(
            "focus <text> on \"<purpose>\", max <tokens>",
            "Reduces a text to the parts relevant to a purpose, using Jev recursively until it fits. In a detail block, `focus` says what to look at within the subject.",
            "7.3",
        ),
        "trail" => doc(
            "trail <n>",
            "The last n step records of the enclosing task, oldest first. The runtime writes them from its own calls, so no agent can steer them.",
            "7.5",
        ),
        "max" => doc(
            "max <n>",
            "A bound: the iteration cap of `loop` and `until`, a token cap in `shape` and `focus`, or the step limit of a machine call. As a function, `max(xs)` is the largest number.",
            "5.5",
        ),
        "tail" => doc(
            "<field> <expr>, max <tokens>, tail",
            "Keeps the last tokens of an overflowing `shape` field instead of the first. As a function, `tail(text, n)` is the last n characters.",
            "7.2",
        ),
        "state" => doc(
            "state <name>: | state <name> done",
            "Declares a machine state and the events enabled in it. A `done` state is terminal.",
            "7.8",
        ),
        "done" => doc(
            "state <name> done | gate ..., done <prob>",
            "Marks a terminal machine state. In a `gate`, the optional probability that the goal is done.",
            "7.8",
        ),
        "when" => doc(
            "on ... -> <state> when <expr>",
            "A code guard. A false guard removes the event from what Jev sees; a guarded entry into a terminal state is what makes a machine's result `verified`.",
            "7.8",
        ),
        "risky" => doc(
            "on ... -> <state> risky",
            "The machine's gate sees `risk` 1 for this event instead of 0.",
            "7.8",
        ),
        "goal" => doc(
            "goal \"<text>\"",
            "The machine's goal, sent to Jev on every step.",
            "7.8",
        ),
        "observe" => doc(
            "observe:\n  <field> <expr>, max <tokens>",
            "Evaluated before every machine step, exactly as `shape`, and bound to `obs` in guards and actions. On an agent handle, `.observe` returns an observation record.",
            "7.8",
        ),
        "stay" => doc(
            "stay",
            "The label every machine Choice carries after the enabled events: nothing in the observation calls for a transition yet.",
            "7.8",
        ),
        "initial" => doc(
            "initial <state>",
            "The machine's first state. Without it, the first state declared.",
            "7.8",
        ),
        "until" => doc(
            "until <cond>, max <n>: | until verify(<cond>), max <n>:",
            "Runs the body until the condition is true, at most n times. The condition is checked before each iteration and once after the last.",
            "5.5",
        ),
        "verify" => doc(
            "until verify(<cond>), max <n>:",
            "Marks the condition whose satisfaction the task reports: `done.verified` is true only if it held at the end. Once per task, directly in the task body.",
            "7.4",
        ),
        "loop" => doc(
            "loop max <n>:",
            "A loop with a fixed upper bound and no condition. There is no unbounded loop.",
            "5.5",
        ),
        "for" => doc(
            "for <name>[, <name>] in <list>:",
            "Iterates a list. Also the loop of a comprehension: `[x for x in xs if cond]`.",
            "5.5",
        ),
        "if" | "elif" | "else" => doc(
            "if <cond>: ... elif <cond>: ... else: ...",
            "A conditional. `cond` is an ordinary expression; an inline judgment is an assignment, not a condition.",
            "5.4",
        ),
        "and" | "or" | "not" => doc(
            "<a> and <b> | <a> or <b> | not <a>",
            "Boolean operators, lowest precedence: `or`, then `and`, then `not`.",
            "5.1",
        ),
        "is" => doc(
            "<choice> is <label>",
            "Compares a `choice` or a named `level` against a bare label.",
            "4.3",
        ),
        "continue" | "break" => doc(
            "continue | break",
            "Loop control, as in Python. Outside a loop it is `loop_control_outside`.",
            "5.5",
        ),
        "stop" => doc(
            "stop \"<reason>\"",
            "Ends the run with a terminal `stopped` pause. As a gate arm, the verdict when confidence is below `stop_confidence`; on a handle, `.stop` stops the agent.",
            "5.6",
        ),
        "escalate" => doc(
            "escalate \"<reason>\"",
            "Ends the run with an `escalate` pause the host may resume. Also a gate verdict.",
            "5.6",
        ),
        "gate" => doc(
            "gate risk <prob>, confidence <number>, done <prob>?:\n  proceed -> ...\n  confirm -> ...\n  escalate -> ...\n  stop -> ...",
            "Turns judgments into a verdict using the task's thresholds, then runs that verdict's arm. `risk` and `confidence` are required.",
            "7.6",
        ),
        "proceed" => doc(
            "proceed -> <statements>",
            "The gate verdict when no threshold is crossed.",
            "7.6",
        ),
        "confirm" => doc(
            "confirm -> <statements>",
            "The gate verdict when `risk >= risk_confirm`.",
            "7.6",
        ),
        "budget" => doc(
            "budget calls <n>, minutes <n>, usd <n>, steps <n>",
            "A task's or machine's limits. Exceeding one pauses with `budget`. `calls` defaults to 50; a callee never widens its caller's remainder.",
            "7.1",
        ),
        "thresholds" => doc(
            "thresholds risk_confirm <p>, min_confidence <p>, stop_confidence <p>, done <p>",
            "The thresholds every gate in the task reads. It is the only place policy thresholds live.",
            "7.6",
        ),
        "using" => doc(
            "<llm>.write \"<instruction>\" using <value>",
            "Forwards a value to the `llm` adapter alongside the instruction, as the named `using` argument.",
            "9.3",
        ),
        "prompt" => doc(
            "<agent>.spawn in <handle>?, prompt <text>",
            "The prompt a spawned agent starts with.",
            "9.1",
        ),
        "above" | "below" => doc(
            "count(<probs>, above <p>) | count(<probs>, below <p>)",
            "The strict threshold `count` compares each prob against.",
            "5.7",
        ),
        "by" => doc(
            "top(<items>, by <probs>, n <k>) | pick among ...: by <field>",
            "Which probs rank the items, or which field of each record Jev reads as the option's description.",
            "5.7",
        ),
        "sample" => doc(
            "sample true | false",
            "Draw the label or level from Jev's distribution instead of taking the argmax. `feels` is never sampled.",
            "6.11",
        ),
        "log" => doc(
            "log <level> <expr> { <field>, <field>: <expr>, ... }",
            "Records one line in the run's recording at a level (`debug`, `info`, `warn`, `error`) and evaluates to the expression's value unchanged. It never reaches Jev, never counts against a budget and never changes control flow.",
            "5.8",
        ),
        "true" | "false" => doc("true | false", "The two `bool` values.", "4"),
        "none" => doc(
            "none",
            "The absent value. In a `pick`, the bare escape label; in `pick among`, allows choosing nothing.",
            "4",
        ),
        _ => return None,
    })
}

/// The four `log` levels, lowest first (spec section 5.8).
pub fn log_level(word: &str) -> Option<Doc> {
    let summary = match word {
        "debug" => "The lowest `log` level: detail for whoever is debugging the program.",
        "info" => "A `log` level for ordinary progress.",
        "warn" => "A `log` level for something unexpected that the run survives.",
        "error" => "The highest `log` level.",
        _ => return None,
    };
    Some(doc(
        "log debug | info | warn | error <expr>",
        summary,
        "5.8",
    ))
}

/// The builtin functions of spec section 5.7.
pub fn builtin(name: &str) -> Option<Doc> {
    let section = "5.7";
    Some(match name {
        "len" => doc("len(x)", "Length of a list or text.", section),
        "count" => doc(
            "count(probs, above p)",
            "Number of entries in a list of probs strictly above `p`. Also `below p`.",
            section,
        ),
        "max" => doc(
            "max(xs)",
            "The largest of a list of numbers. An empty list gives `none`.",
            section,
        ),
        "min" => doc(
            "min(xs)",
            "The smallest of a list of numbers. An empty list gives `none`.",
            section,
        ),
        "sum" => doc("sum(xs)", "Sum of a list of numbers.", section),
        "top" => doc(
            "top(items, by probs, n k)",
            "The `k` items whose paired prob is highest, in descending order.",
            section,
        ),
        "join" => doc(
            "join(texts, sep)",
            "Concatenate texts with a separator. The default separator is a newline.",
            section,
        ),
        "split" => doc(
            "split(text, sep)",
            "Split text on a separator into a list.",
            section,
        ),
        "lines" => doc("lines(text)", "Split on newlines.", section),
        "head" => doc("head(text, n)", "The first `n` characters.", section),
        "tail" => doc("tail(text, n)", "The last `n` characters.", section),
        "tokens" => doc(
            "tokens(x)",
            "The runtime's token estimate for a value's text form.",
            section,
        ),
        "chunk" => doc(
            "chunk(text, tokens n)",
            "Split text into pieces of at most `n` tokens on line boundaries.",
            section,
        ),
        "hash" => doc(
            "hash(x)",
            "A stable short hash of a value's text form.",
            section,
        ),
        "now" => doc(
            "now()",
            "Current time as ISO text. Recorded and replayed.",
            section,
        ),
        "random" => doc(
            "random()",
            "A uniform number in [0, 1). The draw is recorded, and replay returns that exact value.",
            section,
        ),
        "zip" => doc(
            "zip(xs, ys)",
            "A list of two-element lists pairing `xs` and `ys`, as long as the shorter.",
            section,
        ),
        "keys" => doc("keys(r)", "A record's keys as a list.", section),
        "values" => doc("values(r)", "A record's values as a list.", section),
        "items" => doc(
            "items(r)",
            "A record's entries as a list of pairs.",
            section,
        ),
        "text" => doc("text(x)", "Conversion to text.", section),
        "number" => doc("number(x)", "Conversion to a number.", section),
        "bool" => doc("bool(x)", "Conversion to a bool.", section),
        _ => return None,
    })
}

/// The verbs of the fixed-verb capability kinds (spec sections 9.1-9.3).
pub fn verb(kind: CapabilityKind, verb: &str) -> Option<Doc> {
    Some(match (kind, verb) {
        (CapabilityKind::Agent, "spawn") => doc(
            "spawn in <handle>?, prompt <text>, ...",
            "Starts an agent and returns its `handle`. Adapter-specific named arguments after `prompt` pass through unchecked.",
            "9.1",
        ),
        (CapabilityKind::Agent, _) => return handle_verb(verb),
        (CapabilityKind::Person, "ask") => doc(
            "ask <text>, options [<text>...]?",
            "Pauses with `confirm` and returns a `pause_result`.",
            "9.2",
        ),
        (CapabilityKind::Person, "notify") => doc("notify <text>", "Sends without pausing.", "9.2"),
        (CapabilityKind::Person, "take_over") => doc("take_over", "Pauses with `escalate`.", "9.2"),
        (CapabilityKind::Llm, "write") => doc(
            "write \"<instruction>\" using <value>?",
            "Generates text. `using` is forwarded to the adapter as is; each `write` counts one call against the task budget.",
            "9.3",
        ),
        (CapabilityKind::Tool, _) => doc(
            "<tool>.<verb> ...",
            "A verb forwarded to the host adapter. Checked against a signature block if the `needs` declares one, against the adapter's manifest at `task.start`, and otherwise at the call (`verb_missing`).",
            "9.4",
        ),
        _ => return None,
    })
}

/// The verbs of an agent handle (spec section 9.1).
pub fn handle_verb(verb: &str) -> Option<Doc> {
    Some(match verb {
        "observe" => doc(
            "<handle>.observe",
            "Returns an observation record: `status`, `last_message`, `tail`, `exit_code`, and whatever the adapter adds. `tail` and `last_message` are agent-written; shape before judging them.",
            "9.1",
        ),
        "send" => doc("<handle>.send <text>", "Sends the agent a message.", "9.1"),
        "wait" => doc(
            "<handle>.wait idle, minutes <n>",
            "Waits for the agent to go idle and returns an observation record; pauses with `waiting`.",
            "9.1",
        ),
        "stop" => doc("<handle>.stop", "Stops the agent.", "9.1"),
        "spawn" => return self::verb(CapabilityKind::Agent, "spawn"),
        _ => return None,
    })
}

/// The capability kinds (spec section 9).
pub fn kind(kind: CapabilityKind) -> Doc {
    match kind {
        CapabilityKind::Agent => doc(
            "agent",
            "Something that runs a task in the world over time and can be observed. Verbs: `spawn`, `observe`, `send`, `wait`, `stop`.",
            "9.1",
        ),
        CapabilityKind::Person => doc(
            "person",
            "A human in the loop. Verbs: `ask`, `notify`, `take_over`.",
            "9.2",
        ),
        CapabilityKind::Llm => doc("llm", "A text generation model. Verb: `write`.", "9.3"),
        CapabilityKind::Tool => doc(
            "tool",
            "Anything else. Verbs are forwarded to the host adapter; a signature block makes the compiler check them.",
            "9.4",
        ),
    }
}

/// The spec section a compile error or warning code is defined by (spec
/// section 12 lists them; this is where each rule lives).
pub const fn code_section(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Syntax => "13",
        ErrorCode::Indent => "2.2",
        ErrorCode::UnboundedLoop => "5.5",
        ErrorCode::PickNoOther | ErrorCode::PickArity => "6.3",
        ErrorCode::RateArity | ErrorCode::RateBareDegree => "6.4",
        ErrorCode::SubjectNotPath => "6.1",
        ErrorCode::JudgmentSideEffect => "3.5",
        ErrorCode::DefSideEffect => "8",
        ErrorCode::VerbUnknown => "9",
        ErrorCode::VerifyTwice => "7.4",
        ErrorCode::UppercaseIdentifier => "2.4",
        ErrorCode::UnassignedRead => "5.3",
        ErrorCode::UseNotFound
        | ErrorCode::UseCycle
        | ErrorCode::UseHasInputs
        | ErrorCode::UseNeedsUnmapped
        | ErrorCode::UsePrivate
        | ErrorCode::PreludeShadowed => "3.9",
        ErrorCode::MainHasParams => "7",
        ErrorCode::VerbArity => "9.4",
        ErrorCode::MachineUnknownState
        | ErrorCode::MachineNoDone
        | ErrorCode::EventNoDescription
        | ErrorCode::MachineGate
        | ErrorCode::MachineUnreachableDone => "7.8",
        ErrorCode::PickAmongEach => "6.4a",
        ErrorCode::GateArgs => "7.6",
        ErrorCode::DuplicateName => "12",
        ErrorCode::AssignImmutable => "3.2",
        ErrorCode::LoopControlOutside => "5.5",
        ErrorCode::BareProbCondition => "4.1",
        ErrorCode::UncappedField => "7.2",
        ErrorCode::DetailIgnored => "6.8",
        ErrorCode::LogLevel | ErrorCode::LogInQuestion | ErrorCode::LogShadowed => "5.8",
    }
}

/// Render a doc as hover markdown.
pub fn markdown(doc: &Doc) -> String {
    format!(
        "```jevscript\n{}\n```\n{}\n\n*Spec section {}.*",
        doc.signature, doc.summary, doc.section
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevscript_compiler::check::{AGENT_VERBS, BUILTINS, LLM_VERBS, PERSON_VERBS};
    use jevscript_syntax::Keyword;
    use jevscript_syntax::token::CONTEXTUAL_KEYWORDS;

    #[test]
    fn every_reserved_and_contextual_word_is_documented() {
        // Spec section 2.5 lists the reserved words and the contextual ones.
        for (word, _) in Keyword::ALL {
            assert!(keyword(word).is_some(), "`{word}` has no hover");
        }
        for word in CONTEXTUAL_KEYWORDS {
            assert!(keyword(word).is_some(), "`{word}` has no hover");
        }
        for word in ["strict", "tail", "thresholds", "verify", "stay"] {
            assert!(keyword(word).is_some(), "`{word}` has no hover");
        }
    }

    #[test]
    fn every_builtin_and_verb_is_documented() {
        // Spec sections 5.7 and 9.1-9.3.
        for name in BUILTINS {
            assert!(builtin(name).is_some(), "builtin `{name}` has no hover");
        }
        for (kind, verbs) in [
            (CapabilityKind::Agent, &AGENT_VERBS[..]),
            (CapabilityKind::Person, &PERSON_VERBS[..]),
            (CapabilityKind::Llm, &LLM_VERBS[..]),
        ] {
            for name in verbs {
                assert!(verb(kind, name).is_some(), "verb `{name}` has no hover");
            }
        }
    }

    #[test]
    fn every_section_cited_exists_in_the_spec() {
        let spec = include_str!("../../../spec/jevscript-language-specification.md");
        let exists = |section: &str| {
            spec.lines().any(|line| {
                line.starts_with(&format!("## {section}. "))
                    || line.starts_with(&format!("### {section} "))
            })
        };
        for (word, _) in Keyword::ALL {
            let section = keyword(word).expect("documented").section;
            assert!(exists(section), "`{word}` cites missing section {section}");
        }
        for name in BUILTINS {
            let section = builtin(name).expect("documented").section;
            assert!(exists(section), "`{name}` cites missing section {section}");
        }
        for code in [
            ErrorCode::Syntax,
            ErrorCode::PickAmongEach,
            ErrorCode::DefSideEffect,
            ErrorCode::UseCycle,
        ] {
            assert!(exists(code_section(code)), "{code}");
        }
    }
}
