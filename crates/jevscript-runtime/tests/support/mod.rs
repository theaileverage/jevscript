//! Constructors for hand-built IR, fake adapters and a scripted Jev, shared
//! by the interpreter tests.
//!
//! The compiler is being built in parallel; until `compile_file` lands, the
//! tests build the IR the compiler would emit and run the compiler's real
//! batching pass over it so that `request_group` is right (spec section 6.6).

#![allow(dead_code, reason = "each test file uses the subset it needs")]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use jevscript_ir::{
    Arg, BinaryOp, Branch, Budget, CallForm, CapabilityKind, Def, DefParam, Detail, Expr, GateArm,
    Ir, Judge, JudgeVerb, Judgment, JudgmentResult, Machine, MachineState, Need, Output, PickLabel,
    RateLevel, RecordField, ReturnType, ShapeField, ShapePolicy, Stmt, Subject, SubjectStep, Task,
    TextPart, Thresholds, ToolSignature, Transition, UnaryOp, Verdict,
};
use jevscript_runtime::capability::{
    AgentStatus, Bindings, CallArgs, Capability, CapabilityKind as RuntimeKind, Observation,
    ToolManifest,
};
use jevscript_runtime::jev::{JevAnswer, JevError, JevRequest, JevResponse, JevUsage, Question};
use jevscript_runtime::profile::{Profile, Tokenizer};
use jevscript_runtime::record::Event;
use jevscript_runtime::run::{Run, RunOptions};
use jevscript_runtime::{Handle, JevClient, Pause, RuntimeError, RuntimeErrorCode, Value};
use jevscript_syntax::{Pos, Span};

/* -------------------------------------------------------------------------- */
/* Spans                                                                       */
/* -------------------------------------------------------------------------- */

/// A span on `line`, so that pauses can be told apart by source.
pub fn at(line: u32) -> Span {
    Span::new(
        Pos::new(line, 0, line * 100),
        Pos::new(line, 10, line * 100 + 10),
    )
}

/* -------------------------------------------------------------------------- */
/* Expressions                                                                 */
/* -------------------------------------------------------------------------- */

pub fn num(value: f64) -> Expr {
    Expr::Number {
        value,
        span: Span::default(),
    }
}

pub fn text(value: &str) -> Expr {
    Expr::Text {
        parts: vec![TextPart::Literal {
            value: value.to_string(),
        }],
        span: Span::default(),
    }
}

/// A text with `{name}` holes: `interp(&["Hello ", "{who}", "!"])` reads a
/// part starting with `{` as an interpolated expression of that name.
pub fn interp(parts: &[&str]) -> Expr {
    Expr::Text {
        parts: parts
            .iter()
            .map(
                |part| match part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                    Some(path) => TextPart::Interpolation {
                        expr: path_expr(path),
                    },
                    None => TextPart::Literal {
                        value: (*part).to_string(),
                    },
                },
            )
            .collect(),
        span: Span::default(),
    }
}

pub fn boolean(value: bool) -> Expr {
    Expr::Bool {
        value,
        span: Span::default(),
    }
}

pub fn none() -> Expr {
    Expr::None {
        span: Span::default(),
    }
}

pub fn name(name: &str) -> Expr {
    Expr::Name {
        name: name.to_string(),
        span: Span::default(),
    }
}

pub fn list(items: Vec<Expr>) -> Expr {
    Expr::List {
        items,
        span: Span::default(),
    }
}

pub fn record(fields: Vec<(&str, Expr)>) -> Expr {
    Expr::Record {
        fields: fields
            .into_iter()
            .map(|(name, value)| RecordField {
                name: name.to_string(),
                value,
            })
            .collect(),
        span: Span::default(),
    }
}

pub fn field(target: Expr, name: &str) -> Expr {
    Expr::Field {
        target: Box::new(target),
        name: name.to_string(),
        span: Span::default(),
    }
}

/// `a.b.c` as nested field accesses.
pub fn path_expr(path: &str) -> Expr {
    let mut parts = path.split('.');
    let mut expr = name(parts.next().expect("a root"));
    for part in parts {
        expr = field(expr, part);
    }
    expr
}

pub fn index(target: Expr, index: Expr) -> Expr {
    Expr::Index {
        target: Box::new(target),
        index: Box::new(index),
        span: Span::default(),
    }
}

pub fn call(callee: Expr, args: Vec<Expr>) -> Expr {
    Expr::Call {
        callee: Box::new(callee),
        args: args
            .into_iter()
            .map(|value| Arg { name: None, value })
            .collect(),
        form: CallForm::Function,
        span: Span::default(),
    }
}

pub fn call_named(callee: Expr, positional: Vec<Expr>, named: Vec<(&str, Expr)>) -> Expr {
    let mut args: Vec<Arg> = positional
        .into_iter()
        .map(|value| Arg { name: None, value })
        .collect();
    args.extend(named.into_iter().map(|(name, value)| Arg {
        name: Some(name.to_string()),
        value,
    }));
    Expr::Call {
        callee: Box::new(callee),
        args,
        form: CallForm::Command,
        span: Span::default(),
    }
}

/// `cap.verb(args...)`.
pub fn verb(capability: &str, verb: &str, args: Vec<Expr>) -> Expr {
    call(field(name(capability), verb), args)
}

/// `cap.verb positional..., name value...`.
pub fn verb_named(
    capability: &str,
    verb: &str,
    positional: Vec<Expr>,
    named: Vec<(&str, Expr)>,
) -> Expr {
    call_named(field(name(capability), verb), positional, named)
}

/// `f(args...)` for a def, judgment or task.
pub fn call_unit(unit: &str, args: Vec<Expr>) -> Expr {
    call(path_expr(unit), args)
}

pub fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
        span: Span::default(),
    }
}

pub fn not(operand: Expr) -> Expr {
    Expr::Unary {
        op: UnaryOp::Not,
        operand: Box::new(operand),
        span: Span::default(),
    }
}

pub fn is(target: Expr, label: &str) -> Expr {
    Expr::Is {
        target: Box::new(target),
        label: label.to_string(),
        span: Span::default(),
    }
}

pub fn focus(text: Expr, on: &str, max: f64) -> Expr {
    Expr::Focus {
        text: Box::new(text),
        on: Box::new(self::text(on)),
        max,
        span: Span::default(),
    }
}

pub fn trail(count: f64) -> Expr {
    Expr::Trail {
        count,
        span: Span::default(),
    }
}

/* -------------------------------------------------------------------------- */
/* Judgments                                                                   */
/* -------------------------------------------------------------------------- */

/// A subject written as `root.field[0].field`.
pub fn subject(path: &str) -> Subject {
    let root_end = path.find(['.', '[']).unwrap_or(path.len());
    let root = &path[..root_end];
    let mut steps = Vec::new();
    let mut rest = &path[root_end..];
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            steps.push(SubjectStep::Field {
                name: after[..end].to_string(),
            });
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']').expect("a closing bracket");
            steps.push(SubjectStep::Index {
                index: num(after[..end].parse().expect("a number")),
            });
            rest = &after[end + 1..];
        } else {
            panic!("bad subject path {path}");
        }
    }
    Subject {
        root: root.to_string(),
        path: steps,
        state_path: path.to_string(),
        span: Span::default(),
    }
}

pub fn feels(path: &str, condition: &str) -> Expr {
    judge(
        false,
        path,
        JudgeVerb::Feels {
            condition: text(condition),
        },
    )
}

pub fn each_feels(path: &str, condition: &str) -> Expr {
    judge(
        true,
        path,
        JudgeVerb::Feels {
            condition: text(condition),
        },
    )
}

pub fn pick(path: &str, labels: &[(&str, &str)]) -> Expr {
    judge(false, path, pick_verb(labels))
}

pub fn pick_verb(labels: &[(&str, &str)]) -> JudgeVerb {
    let mut all: Vec<PickLabel> = labels
        .iter()
        .map(|(name, description)| PickLabel {
            name: (*name).to_string(),
            escape: false,
            description: Some(text(description)),
            what: None,
            not_for: None,
            examples: None,
            span: Span::default(),
        })
        .collect();
    all.push(PickLabel {
        name: "other".to_string(),
        escape: true,
        description: None,
        what: None,
        not_for: None,
        examples: None,
        span: Span::default(),
    });
    JudgeVerb::Pick { labels: all }
}

pub fn rate(path: &str, levels: &[(&str, &str)]) -> Expr {
    judge(
        false,
        path,
        JudgeVerb::Rate {
            levels: levels
                .iter()
                .map(|(name, situation)| RateLevel {
                    name: (!name.is_empty()).then(|| (*name).to_string()),
                    situation: text(situation),
                    span: Span::default(),
                })
                .collect(),
        },
    )
}

pub fn pick_among(path: &str, question: &str, by: Option<&str>, allow_none: bool) -> Expr {
    judge(
        false,
        path,
        JudgeVerb::PickAmong {
            question: text(question),
            by: by.map(str::to_string),
            allow_none,
        },
    )
}

pub fn judge(each: bool, path: &str, verb: JudgeVerb) -> Expr {
    Expr::Judge(Box::new(Judge {
        each,
        subject: subject(path),
        verb,
        detail: None,
        request_group: 0,
        span: Span::default(),
    }))
}

/// The same judgment with `sample true`.
pub fn sampled(expr: Expr) -> Expr {
    match expr {
        Expr::Judge(mut judge) => {
            judge.detail = Some(Detail {
                sample: Some(true),
                ..Detail::default()
            });
            Expr::Judge(judge)
        }
        other => other,
    }
}

/* -------------------------------------------------------------------------- */
/* Statements                                                                  */
/* -------------------------------------------------------------------------- */

pub fn assign(root: &str, value: Expr) -> Stmt {
    Stmt::Assign {
        root: root.to_string(),
        path: Vec::new(),
        value,
        span: Span::default(),
    }
}

pub fn assign_field(root: &str, path: &[&str], value: Expr) -> Stmt {
    Stmt::Assign {
        root: root.to_string(),
        path: path.iter().map(|p| (*p).to_string()).collect(),
        value,
        span: Span::default(),
    }
}

pub fn expr(expr: Expr) -> Stmt {
    Stmt::Expr {
        expr,
        span: Span::default(),
    }
}

/// A statement placed on `line`, for pauses that must name their source.
pub fn on_line(stmt: Stmt, line: u32) -> Stmt {
    let span = at(line);
    match stmt {
        Stmt::Assign {
            root, path, value, ..
        } => Stmt::Assign {
            root,
            path,
            value,
            span,
        },
        Stmt::Expr { expr, .. } => Stmt::Expr { expr, span },
        Stmt::Gate {
            risk,
            confidence,
            done,
            arms,
            ..
        } => Stmt::Gate {
            risk,
            confidence,
            done,
            arms,
            span,
        },
        Stmt::Stop { reason, .. } => Stmt::Stop { reason, span },
        Stmt::Escalate { reason, .. } => Stmt::Escalate { reason, span },
        other => other,
    }
}

pub fn if_else(test: Expr, body: Vec<Stmt>, otherwise: Option<Vec<Stmt>>) -> Stmt {
    Stmt::If {
        branches: vec![Branch {
            test,
            body,
            span: Span::default(),
        }],
        otherwise,
        span: Span::default(),
    }
}

pub fn if_elif(branches: Vec<(Expr, Vec<Stmt>)>, otherwise: Option<Vec<Stmt>>) -> Stmt {
    Stmt::If {
        branches: branches
            .into_iter()
            .map(|(test, body)| Branch {
                test,
                body,
                span: Span::default(),
            })
            .collect(),
        otherwise,
        span: Span::default(),
    }
}

pub fn for_in(names: &[&str], iterable: Expr, body: Vec<Stmt>) -> Stmt {
    Stmt::For {
        names: names.iter().map(|n| (*n).to_string()).collect(),
        iterable,
        body,
        span: Span::default(),
    }
}

pub fn loop_max(max: f64, body: Vec<Stmt>) -> Stmt {
    Stmt::Loop {
        max,
        body,
        span: Span::default(),
    }
}

pub fn until(test: Expr, max: f64, body: Vec<Stmt>) -> Stmt {
    Stmt::Until {
        test,
        verify: false,
        max,
        body,
        span: Span::default(),
    }
}

pub fn until_verify(test: Expr, max: f64, body: Vec<Stmt>) -> Stmt {
    Stmt::Until {
        test,
        verify: true,
        max,
        body,
        span: Span::default(),
    }
}

pub fn gate(
    risk: Expr,
    confidence: Expr,
    done: Option<Expr>,
    arms: Vec<(Verdict, Vec<Stmt>)>,
) -> Stmt {
    Stmt::Gate {
        risk: Some(risk),
        confidence: Some(confidence),
        done,
        arms: arms
            .into_iter()
            .map(|(verdict, body)| GateArm {
                verdict,
                body,
                span: Span::default(),
            })
            .collect(),
        span: Span::default(),
    }
}

pub fn shape(target: &str, strict: bool, fields: Vec<ShapeField>) -> Stmt {
    Stmt::Shape {
        target: target.to_string(),
        strict,
        fields,
        span: Span::default(),
    }
}

pub fn shape_field(name: &str, value: Expr, max: Option<f64>, policy: ShapePolicy) -> ShapeField {
    ShapeField {
        name: name.to_string(),
        value,
        max,
        policy,
        span: Span::default(),
    }
}

pub fn ret(value: Option<Expr>) -> Stmt {
    Stmt::Return {
        value,
        span: Span::default(),
    }
}

pub fn stop(reason: &str) -> Stmt {
    Stmt::Stop {
        reason: text(reason),
        span: Span::default(),
    }
}

pub fn escalate(reason: &str) -> Stmt {
    Stmt::Escalate {
        reason: text(reason),
        span: Span::default(),
    }
}

pub fn cont() -> Stmt {
    Stmt::Continue {
        span: Span::default(),
    }
}

pub fn brk() -> Stmt {
    Stmt::Break {
        span: Span::default(),
    }
}

/* -------------------------------------------------------------------------- */
/* Units and programs                                                          */
/* -------------------------------------------------------------------------- */

pub fn need(name: &str, kind: CapabilityKind) -> Need {
    Need {
        name: name.to_string(),
        kind,
        signatures: Vec::new(),
        span: Span::default(),
    }
}

pub fn signature(name: &str, params: &[&str], returns: ReturnType) -> ToolSignature {
    ToolSignature {
        name: name.to_string(),
        params: params.iter().map(|p| (*p).to_string()).collect(),
        returns,
        span: Span::default(),
    }
}

pub fn param(name: &str) -> DefParam {
    DefParam {
        name: name.to_string(),
        default: None,
    }
}

pub fn param_default(name: &str, default: Expr) -> DefParam {
    DefParam {
        name: name.to_string(),
        default: Some(default),
    }
}

pub fn def(name: &str, params: Vec<DefParam>, body: Vec<Stmt>) -> Def {
    Def {
        name: name.to_string(),
        params,
        body,
        span: Span::default(),
    }
}

pub fn task(
    name: &str,
    params: Vec<DefParam>,
    budget: Budget,
    thresholds: Thresholds,
    body: Vec<Stmt>,
) -> Task {
    Task {
        name: name.to_string(),
        params,
        budget,
        thresholds,
        body,
        span: at(1),
    }
}

pub fn transition(
    event: &str,
    description: &str,
    target: &str,
    when: Option<Expr>,
    risky: bool,
    body: Vec<Stmt>,
) -> Transition {
    Transition {
        event: event.to_string(),
        description: text(description),
        target: target.to_string(),
        when,
        risky,
        body,
        span: Span::default(),
    }
}

pub fn machine_state(name: &str, done: bool, transitions: Vec<Transition>) -> MachineState {
    MachineState {
        name: name.to_string(),
        done,
        transitions,
        span: Span::default(),
    }
}

pub fn machine(
    name: &str,
    params: Vec<DefParam>,
    budget: Budget,
    thresholds: Thresholds,
    goal: &str,
    observe: Vec<ShapeField>,
    states: Vec<MachineState>,
) -> Machine {
    let initial = states
        .first()
        .map(|state| state.name.clone())
        .unwrap_or_default();
    Machine {
        name: name.to_string(),
        params,
        budget,
        thresholds,
        goal: Some(text(goal)),
        initial,
        observe,
        states,
        shape_hash: String::new(),
        span: Span::default(),
    }
}

pub fn judgment(name: &str, params: &[&str], results: Vec<(&str, Expr)>) -> Judgment {
    Judgment {
        name: name.to_string(),
        params: params.iter().map(|p| (*p).to_string()).collect(),
        results: results
            .into_iter()
            .map(|(name, question)| JudgmentResult {
                name: name.to_string(),
                question: match question {
                    Expr::Judge(judge) => *judge,
                    _ => panic!("a judgment result is a judgment expression"),
                },
                span: Span::default(),
            })
            .collect(),
        logs: Vec::new(),
        shape_hash: String::new(),
        span: Span::default(),
    }
}

pub fn budget(
    calls: Option<f64>,
    minutes: Option<f64>,
    usd: Option<f64>,
    steps: Option<f64>,
) -> Budget {
    Budget {
        calls,
        minutes,
        usd,
        steps,
    }
}

pub fn thresholds(
    risk_confirm: Option<f64>,
    min_confidence: Option<f64>,
    stop_confidence: Option<f64>,
    done: Option<f64>,
) -> Thresholds {
    Thresholds {
        risk_confirm,
        min_confidence,
        stop_confidence,
        done,
        other: Vec::new(),
    }
}

/// A program with `main` as given and the batching pass run over every unit
/// (spec section 6.6).
pub fn program(needs: Vec<Need>, outputs: &[&str], main: Vec<Stmt>) -> Ir {
    let mut ir = Ir::empty("test");
    ir.needs = needs;
    ir.outputs = outputs
        .iter()
        .map(|name| Output {
            name: (*name).to_string(),
            span: Span::default(),
        })
        .collect();
    ir.tasks.push(task(
        "main",
        Vec::new(),
        Budget {
            calls: Some(50.0),
            ..Budget::default()
        },
        Thresholds::default(),
        main,
    ));
    batch(&mut ir);
    ir
}

/// Run the compiler's batching pass over every unit.
pub fn batch(ir: &mut Ir) {
    let capabilities: BTreeSet<String> = ir.needs.iter().map(|n| n.name.clone()).collect();
    for task in &mut ir.tasks {
        jevscript_compiler::batch::assign_request_groups(&mut task.body, &capabilities);
    }
    for def in &mut ir.defs {
        jevscript_compiler::batch::assign_request_groups(&mut def.body, &capabilities);
    }
    for judgment in &mut ir.judgments {
        jevscript_compiler::batch::assign_judgment_block(judgment);
    }
    for machine in &mut ir.machines {
        jevscript_compiler::batch::assign_machine_groups(machine, &capabilities);
    }
}

/* -------------------------------------------------------------------------- */
/* Profiles                                                                    */
/* -------------------------------------------------------------------------- */

/// A profile with test-sized caps: four questions per request, four
/// criteria, and a price of one dollar per million tokens. Every number here
/// is fixture data.
pub fn tiny_profile() -> Profile {
    Profile {
        model: "jev-test".to_string(),
        endpoint: "http://localhost/none".to_string(),
        total_tokens: 100_000,
        state_plus_question_tokens: 50_000,
        max_questions_per_request: 4,
        max_criteria_per_question: 4,
        tokenizer: Tokenizer::Chars4,
        price_per_million_input_usd: 1.0,
        price_per_million_output_usd: 0.0,
        aliases: None,
    }
}

/// A profiles file holding [`tiny_profile`], and the options that select it.
pub fn tiny_profile_options(dir: &std::path::Path) -> RunOptions {
    let path = dir.join("profiles.json");
    std::fs::write(
        &path,
        serde_json::to_string(&vec![tiny_profile()]).expect("json"),
    )
    .expect("writes the profiles file");
    RunOptions {
        model: Some("jev-test".to_string()),
        profiles: Some(path),
        ..RunOptions::default()
    }
}

/// A fresh directory for one test's files.
pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "jevscript-interp-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("creates the temp dir");
    dir
}

/* -------------------------------------------------------------------------- */
/* A scripted Jev                                                              */
/* -------------------------------------------------------------------------- */

/// Answers every question from a table keyed by question id (without any
/// `[i]` suffix), so a test says what Jev thinks of each judgment.
#[derive(Clone, Default)]
pub struct Answers {
    pub probs: BTreeMap<String, f64>,
    pub labels: BTreeMap<String, (String, f64)>,
    pub levels: BTreeMap<String, (u32, f64)>,
    /// Per-element probabilities for `each`, by base id.
    pub each: BTreeMap<String, Vec<f64>>,
}

impl Answers {
    pub fn prob(mut self, id: &str, p: f64) -> Self {
        self.probs.insert(id.to_string(), p);
        self
    }

    pub fn label(mut self, id: &str, label: &str, confidence: f64) -> Self {
        self.labels
            .insert(id.to_string(), (label.to_string(), confidence));
        self
    }

    pub fn level(mut self, id: &str, level: u32, confidence: f64) -> Self {
        self.levels.insert(id.to_string(), (level, confidence));
        self
    }

    pub fn each(mut self, id: &str, probs: &[f64]) -> Self {
        self.each.insert(id.to_string(), probs.to_vec());
        self
    }

    /// The scripted client answering from this table.
    pub fn client(self) -> jevscript_runtime::ScriptedJevClient {
        jevscript_runtime::ScriptedJevClient::from_fn(move |request| Ok(self.respond(request)))
    }

    pub fn respond(&self, request: &JevRequest) -> JevResponse {
        let answers = request
            .questions
            .iter()
            .map(|question| self.answer(question))
            .collect();
        JevResponse {
            answers,
            usage: JevUsage {
                tokens: 1000,
                usd: None,
            },
            latency_ms: 5,
        }
    }

    fn answer(&self, question: &Question) -> JevAnswer {
        let id = question.id().to_string();
        let (base, element) = match id.split_once('[') {
            Some((base, rest)) => (
                base.to_string(),
                rest.trim_end_matches(']').parse::<usize>().ok(),
            ),
            None => (id.clone(), None),
        };
        match question {
            Question::Noul { .. } => {
                let prob = element
                    .and_then(|i| self.each.get(&base).and_then(|ps| ps.get(i).copied()))
                    .or_else(|| self.probs.get(&base).copied())
                    .unwrap_or(0.1);
                JevAnswer::Noul { id, prob }
            }
            Question::Choice { labels, .. } => {
                let (label, confidence) = self
                    .labels
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| (labels[0].name.clone(), 0.9));
                let share = (1.0 - confidence.min(0.99)) / labels.len().max(1) as f64;
                let probabilities = labels
                    .iter()
                    .map(|l| {
                        let p = if l.name == label {
                            confidence.min(0.99) + share
                        } else {
                            share
                        };
                        (l.name.clone(), p)
                    })
                    .collect();
                JevAnswer::Choice {
                    id,
                    label,
                    confidence,
                    probabilities,
                }
            }
            Question::Score { levels, .. } => {
                let (level, confidence) = self.levels.get(&base).copied().unwrap_or((0, 0.9));
                let named = levels.iter().all(|l| l.name.is_some());
                let probabilities = levels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let key = if named {
                            l.name.clone().unwrap_or_default()
                        } else {
                            i.to_string()
                        };
                        (
                            key,
                            if i as u32 == level {
                                0.8
                            } else {
                                0.2 / (levels.len() - 1).max(1) as f64
                            },
                        )
                    })
                    .collect();
                JevAnswer::Score {
                    id,
                    level,
                    score: f64::from(level),
                    confidence,
                    probabilities,
                }
            }
        }
    }
}

/// A Jev client that must never be called: what a replay runs against.
pub struct NeverJev;

impl JevClient for NeverJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        panic!(
            "replay reached Jev with {} questions",
            request.questions.len()
        );
    }
}

/// A Jev client that fails every request as unavailable until it has failed
/// `failures` times, then answers from `answers`.
pub struct FlakyJev {
    pub failures: Mutex<u32>,
    pub answers: Answers,
}

impl JevClient for FlakyJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let mut failures = self.failures.lock().expect("not poisoned");
        if *failures > 0 {
            *failures -= 1;
            return Err(JevError::Unavailable("503".to_string()));
        }
        Ok(self.answers.respond(request))
    }
}

/* -------------------------------------------------------------------------- */
/* Fake adapters                                                               */
/* -------------------------------------------------------------------------- */

/// What every fake adapter logs: `capability.verb(args)` in order.
pub type CallLog = Arc<Mutex<Vec<String>>>;

pub fn call_log() -> CallLog {
    Arc::new(Mutex::new(Vec::new()))
}

pub fn logged(log: &CallLog) -> Vec<String> {
    log.lock().expect("not poisoned").clone()
}

fn log_call(log: &CallLog, capability: &str, verb: &str, args: &CallArgs) {
    let positional: Vec<String> = args.positional.iter().map(Value::to_text).collect();
    let named: Vec<String> = args
        .named
        .iter()
        .map(|(k, v)| format!("{k}={}", v.to_text()))
        .collect();
    log.lock().expect("not poisoned").push(format!(
        "{capability}.{verb}({})",
        positional
            .into_iter()
            .chain(named)
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

/// A scripted `tool`: each verb answers from a list of values in order, and
/// repeats its last value when the list runs out. An unknown verb is
/// `verb_missing`.
pub struct FakeTool {
    pub name: String,
    pub verbs: BTreeMap<String, Vec<Value>>,
    pub calls: BTreeMap<String, usize>,
    pub log: CallLog,
    pub manifest: Option<ToolManifest>,
    /// Verbs that fail with a retryable `adapter_error` this many times first.
    pub flaky: BTreeMap<String, u32>,
    /// Verbs that block for this long before answering, so that wall time
    /// passes while the runtime waits on the adapter (spec section 9.6).
    pub slow: BTreeMap<String, std::time::Duration>,
}

impl FakeTool {
    pub fn new(name: &str, log: &CallLog) -> Self {
        Self {
            name: name.to_string(),
            verbs: BTreeMap::new(),
            calls: BTreeMap::new(),
            log: Arc::clone(log),
            manifest: None,
            flaky: BTreeMap::new(),
            slow: BTreeMap::new(),
        }
    }

    pub fn slow(mut self, verb: &str, millis: u64) -> Self {
        self.slow
            .insert(verb.to_string(), std::time::Duration::from_millis(millis));
        self
    }

    pub fn verb(mut self, verb: &str, values: Vec<Value>) -> Self {
        self.verbs.insert(verb.to_string(), values);
        self
    }

    pub fn flaky(mut self, verb: &str, failures: u32) -> Self {
        self.flaky.insert(verb.to_string(), failures);
        self
    }

    pub fn with_manifest(mut self, verbs: &[&str]) -> Self {
        self.manifest = Some(ToolManifest {
            verbs: verbs
                .iter()
                .map(|v| {
                    (
                        (*v).to_string(),
                        jevscript_runtime::capability::ManifestVerb {
                            params: Vec::new(),
                            returns: None,
                        },
                    )
                })
                .collect(),
        });
        self
    }
}

impl Capability for FakeTool {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Tool
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        if let Some(delay) = self.slow.get(verb) {
            std::thread::sleep(*delay);
        }
        if let Some(failures) = self.flaky.get_mut(verb)
            && *failures > 0
        {
            *failures -= 1;
            return Err(RuntimeError::new(
                RuntimeErrorCode::AdapterError,
                format!("`{verb}` flaked"),
            )
            .retryable(true));
        }
        let Some(values) = self.verbs.get(verb) else {
            return Err(RuntimeError::new(
                RuntimeErrorCode::VerbMissing,
                format!("`{}` does not implement `{verb}`", self.name),
            ));
        };
        let n = self.calls.entry(verb.to_string()).or_insert(0);
        let value = values
            .get(*n)
            .or_else(|| values.last())
            .cloned()
            .unwrap_or(Value::None);
        *n += 1;
        Ok(value)
    }

    fn manifest(&self) -> Option<ToolManifest> {
        self.manifest.clone()
    }
}

/// A scripted `agent`: `spawn` mints handles, `observe` and `wait` answer
/// from a list of observations in order (repeating the last), `send` and
/// `stop` are logged.
pub struct FakeAgent {
    pub name: String,
    pub observations: Vec<Observation>,
    pub observed: usize,
    pub spawned: u32,
    pub log: CallLog,
}

impl FakeAgent {
    pub fn new(name: &str, log: &CallLog, observations: Vec<Observation>) -> Self {
        Self {
            name: name.to_string(),
            observations,
            observed: 0,
            spawned: 0,
            log: Arc::clone(log),
        }
    }

    fn next_observation(&mut self) -> Observation {
        let observation = self
            .observations
            .get(self.observed)
            .or_else(|| self.observations.last())
            .cloned()
            .unwrap_or_else(|| observation("running", "", ""));
        self.observed += 1;
        observation
    }
}

pub fn observation(status: &str, last_message: &str, tail: &str) -> Observation {
    Observation {
        status: match status {
            "waiting" => AgentStatus::Waiting,
            "exited" => AgentStatus::Exited,
            _ => AgentStatus::Running,
        },
        last_message: last_message.to_string(),
        tail: tail.to_string(),
        exit_code: None,
        fields: BTreeMap::new(),
    }
}

impl Capability for FakeAgent {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Agent
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        match verb {
            "spawn" => {
                self.spawned += 1;
                Ok(Value::Handle(Handle {
                    capability: self.name.clone(),
                    id: format!("agent-{}", self.spawned),
                    fields: BTreeMap::new(),
                }))
            }
            "wait" => {
                let observation = self.next_observation();
                Ok(Value::from_json(
                    &serde_json::to_value(observation).expect("serializes"),
                ))
            }
            "send" | "stop" => Ok(Value::None),
            other => Err(RuntimeError::new(
                RuntimeErrorCode::VerbMissing,
                format!("agent has no verb `{other}`"),
            )),
        }
    }

    fn observe(&mut self, handle: &Handle) -> Result<Observation, RuntimeError> {
        self.log
            .lock()
            .expect("not poisoned")
            .push(format!("{}.observe({})", self.name, handle.id));
        Ok(self.next_observation())
    }
}

/// A `person` whose `notify` is logged. `ask` and `take_over` never reach
/// the adapter: they are pauses (spec section 9.2).
pub struct FakePerson {
    pub name: String,
    pub log: CallLog,
}

impl FakePerson {
    pub fn new(name: &str, log: &CallLog) -> Self {
        Self {
            name: name.to_string(),
            log: Arc::clone(log),
        }
    }
}

impl Capability for FakePerson {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Person
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        match verb {
            "notify" => Ok(Value::None),
            other => Err(RuntimeError::new(
                RuntimeErrorCode::AdapterError,
                format!("`{other}` should have been a pause, not an adapter call"),
            )),
        }
    }
}

/// An `llm` that writes a fixed prefix plus its context's text form.
pub struct FakeLlm {
    pub name: String,
    pub log: CallLog,
}

impl FakeLlm {
    pub fn new(name: &str, log: &CallLog) -> Self {
        Self {
            name: name.to_string(),
            log: Arc::clone(log),
        }
    }
}

impl Capability for FakeLlm {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Llm
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        let using = args.named("using").map(Value::to_text).unwrap_or_default();
        Ok(Value::Text(format!("draft: {using}")))
    }
}

/* -------------------------------------------------------------------------- */
/* Running                                                                     */
/* -------------------------------------------------------------------------- */

pub fn bindings(adapters: Vec<(&str, Box<dyn Capability>)>) -> Bindings {
    adapters
        .into_iter()
        .map(|(name, adapter)| (name.to_string(), adapter))
        .collect()
}

/// A run over `ir` with `answers` as Jev and the tiny profile.
pub fn start(ir: Ir, options: RunOptions, bindings: Bindings, answers: Answers) -> Run {
    Run::create(ir, "main", options, bindings)
        .expect("starts")
        .with_client(Box::new(answers.client()))
}

/// Step a run to its end, answering every pause with `answer`, and return
/// every pause seen. A pause the host does not answer is stepped past
/// anyway, which a `waiting` pause and a replayed pause allow; the run's
/// refusal to continue ends the drive.
pub fn drive(
    run: &mut Run,
    mut answer: impl FnMut(&Pause) -> Option<jevscript_runtime::Resume>,
) -> Vec<Pause> {
    let mut pauses = Vec::new();
    loop {
        let pause = match run.next() {
            Ok(pause) => pause,
            Err(jevscript_runtime::RunError::WrongPayload { .. }) => return pauses,
            Err(error) => panic!("stepping failed: {error}"),
        };
        let terminal = pause.is_terminal();
        pauses.push(pause.clone());
        if terminal {
            return pauses;
        }
        if let Some(resume) = answer(&pause) {
            run.resume(resume).expect("resumes");
        }
    }
}

/// The recorded events of a kind, from a drained event list.
pub fn events_of<'a>(events: &'a [Event], kind: &str) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| serde_json::to_value(e).expect("serializes")["event"] == kind)
        .collect()
}

pub fn text_value(s: &str) -> Value {
    Value::Text(s.to_string())
}

pub fn number_value(n: f64) -> Value {
    Value::Number(n)
}
