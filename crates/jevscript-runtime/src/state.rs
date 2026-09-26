//! State construction and question building (spec sections 6.1 to 6.10).
//!
//! A request's state is built from the subjects of every question in it, each
//! placed at its path, and nothing else (spec section 6.9). A variable in
//! scope that no subject names does not reach Jev; sending more is a
//! correctness bug, since Jev's accuracy falls as unrelated state grows.
//!
//! [`build_request`] turns a batch of judgments into one or more
//! [`JevRequest`]s: an `each` becomes one question per element (6.5), the
//! batch is split when it exceeds the profile's question cap, a `pick among`
//! becomes one Choice over `i0..iN` (6.4a), and every request passes the size
//! check (6.10) before it is returned. The [`Plan`] it returns is what
//! [`crate::answer::answers_to_values`] needs to turn answers back into values.

use jevscript_ir::{Detail, Expr, Judge, JudgeVerb, Subject, SubjectStep};
use serde_json::Value as Json;
use std::collections::BTreeMap;

use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::eval::{Env, Evaluator, PureEffects};
use crate::jev::{ChoiceLabel, Instruction, JevRequest, Question, ScoreLevel};
use crate::profile::Profile;
use crate::size;
use crate::value::Value;

/// One judgment to ask, and the name its answer lands under: the assigned
/// variable in a task, or the result name in a `judgment` block (spec 6.7).
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    /// The result name. Question ids are this name, or `name[i]` for `each`.
    pub name: &'a str,
    /// The judgment.
    pub judge: &'a Judge,
}

/// What kind of answer an ask expects, with what the answer needs to become a
/// value: label order for sampling, level names for `is`, the items of a
/// `pick among` for `item`.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedKind {
    /// `feels`: a `prob`.
    Feels,
    /// `pick`: a `choice` over these labels, in declared order.
    Pick {
        /// The labels as declared.
        labels: Vec<String>,
    },
    /// `rate`: a `level` over this many levels, named or not.
    Rate {
        /// How many levels.
        levels: usize,
        /// Their names, or empty when unnamed.
        names: Vec<String>,
    },
    /// `pick among`: a `choice` over `i0..iN` plus `none` when allowed.
    PickAmong {
        /// The labels in order: `i0`, `i1`, ..., then `none` if allowed.
        labels: Vec<String>,
        /// The runtime list, so the answer can carry `item`.
        items: Vec<Value>,
    },
}

/// One ask as planned: how many questions it became and what they return.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAsk {
    /// The result name.
    pub name: String,
    /// What the answers become.
    pub kind: PlannedKind,
    /// For `each`, how many elements were asked about; the answers come back
    /// as a list of that length (spec section 6.5).
    pub each: Option<usize>,
    /// Whether the detail block said `sample true` (spec section 6.11).
    pub sample: bool,
}

impl PlannedAsk {
    /// The question ids this ask's answers are keyed by, in order.
    pub fn question_ids(&self) -> Vec<String> {
        match self.each {
            Some(count) => (0..count).map(|i| format!("{}[{i}]", self.name)).collect(),
            None => vec![self.name.clone()],
        }
    }
}

/// The requests a batch of judgments became, and how to read their answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// One or more requests, split at the profile's question cap. Every one
    /// has passed the size check.
    pub requests: Vec<JevRequest>,
    /// One entry per ask, in the order given.
    pub asks: Vec<PlannedAsk>,
}

impl Plan {
    /// How many questions the plan asks in total.
    pub fn question_count(&self) -> usize {
        self.requests.iter().map(|r| r.questions.len()).sum()
    }
}

/// One step of a resolved subject path.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Field(String),
    Index(usize),
}

/// A subject's value at its path, ready to be placed in a state.
#[derive(Debug, Clone)]
struct Placement {
    root: String,
    steps: Vec<Step>,
    value: Json,
}

/// A question together with the placements its state needs.
struct Planned {
    question: Question,
    placements: Vec<Placement>,
}

/// Build the requests for an inline group of `asks` against `env` (spec
/// sections 6.5, 6.6, 6.9, 6.10): the state is exactly the subject and
/// `compare` paths of the questions in each request.
///
/// # Errors
///
/// `type_error` if a subject is not what its verb needs, `pick_too_many` if a
/// `pick among` list or a `pick` exceeds the profile's criteria cap, and
/// `state_too_large` if a request fails the size check or the group expands
/// past the profile's question cap with no `each` over the cap on its own.
pub fn build_request(
    asks: &[Ask<'_>],
    env: &mut Env,
    profile: &Profile,
) -> Result<Plan, RuntimeError> {
    build(asks, None, env, profile)
}

/// Build the requests for a named judgment (spec sections 6.7 and 6.9): one
/// request whose state is every declared parameter in full, even one no
/// subject reads, and nothing else. An `each` over the cap is the sole reason
/// the judgment becomes several requests, and every one of them carries the
/// same parameter state.
///
/// # Errors
///
/// As [`build_request`].
pub fn build_judgment_request(
    asks: &[Ask<'_>],
    params: &[String],
    env: &mut Env,
    profile: &Profile,
) -> Result<Plan, RuntimeError> {
    build(asks, Some(params), env, profile)
}

fn build(
    asks: &[Ask<'_>],
    params: Option<&[String]>,
    env: &mut Env,
    profile: &Profile,
) -> Result<Plan, RuntimeError> {
    let mut pure = PureEffects;
    let mut evaluator = Evaluator::new(&mut pure, profile);
    let mut planned_asks = Vec::with_capacity(asks.len());
    let mut questions: Vec<Planned> = Vec::new();
    for ask in asks {
        let (planned, mut asked) = plan_ask(ask, env, &mut evaluator, profile)?;
        planned_asks.push(planned);
        questions.append(&mut asked);
    }
    // Spec 6.6: the question cap never splits a group on its own. Only an
    // `each` that is over the cap by itself permits consecutive chunks, in
    // question order; any other oversized group fails before anything is
    // sent.
    let cap = (profile.max_questions_per_request as usize).max(1);
    let oversized_each = planned_asks
        .iter()
        .any(|ask| ask.each.is_some_and(|count| count > cap));
    if questions.len() > cap && !oversized_each {
        let names: Vec<&str> = planned_asks.iter().map(|a| a.name.as_str()).collect();
        return Err(RuntimeError::new(
            RuntimeErrorCode::StateTooLarge,
            format!(
                "the request group {names:?} expands to {} questions, over the {cap} per request allowed by `{}`, and no `each` in it is over the cap on its own; ask fewer questions together",
                questions.len(),
                profile.model
            ),
        )
        .at(asks.first().map_or_else(jevscript_syntax::Span::default, |a| a.judge.span)));
    }
    let named_state: Option<BTreeMap<String, Json>> = params.map(|params| {
        params
            .iter()
            .map(|param| {
                (
                    param.clone(),
                    env.get(param).map_or(Json::Null, Value::to_json),
                )
            })
            .collect()
    });
    let mut requests = Vec::new();
    for chunk in questions.chunks(cap) {
        let state = match &named_state {
            Some(state) => state.clone(),
            None => {
                let mut state: BTreeMap<String, Json> = BTreeMap::new();
                for planned in chunk {
                    for placement in &planned.placements {
                        place(&mut state, placement);
                    }
                }
                state
            }
        };
        let request = JevRequest {
            state,
            model: profile.model.clone(),
            questions: chunk.iter().map(|p| p.question.clone()).collect(),
        };
        size::check_request(&request, profile)?;
        requests.push(request);
    }
    Ok(Plan {
        requests,
        asks: planned_asks,
    })
}

fn plan_ask(
    ask: &Ask<'_>,
    env: &mut Env,
    evaluator: &mut Evaluator<'_>,
    profile: &Profile,
) -> Result<(PlannedAsk, Vec<Planned>), RuntimeError> {
    let judge = ask.judge;
    let span = judge.span;
    let (subject, root, steps) = resolve_subject(&judge.subject, env, evaluator)?;
    let path = path_text(&root, &steps);
    let instruction = instruction_of(judge.detail.as_ref(), env, evaluator)?;
    let compare = compare_placements(instruction.as_ref(), env)?;
    let sample = judge
        .detail
        .as_ref()
        .and_then(|d| d.sample)
        .unwrap_or(false);
    // Spec 6.9: an `each` question's subject is one element, `path[i]`, so
    // only that element is placed and the rest of the list is `null`. A
    // request that holds part of a split `each` therefore carries exactly
    // what it judges, and nothing else.
    let placements = |i: usize| {
        let placement = match (&subject, judge.each) {
            (Value::List(items), true) => Placement {
                root: root.clone(),
                steps: steps.iter().cloned().chain([Step::Index(i)]).collect(),
                value: items.get(i).map_or(Json::Null, Value::to_json),
            },
            _ => Placement {
                root: root.clone(),
                steps: steps.clone(),
                value: subject.to_json(),
            },
        };
        let mut all = vec![placement];
        all.extend(compare.iter().cloned());
        all
    };

    // `each`: one question per element, at `path[i]` (spec section 6.5).
    let targets: Vec<(String, String)> = if judge.each {
        let Value::List(items) = &subject else {
            return Err(Value::type_error("a list for `each`", &subject).at(span));
        };
        (0..items.len())
            .map(|i| (format!("{}[{i}]", ask.name), format!("{path}[{i}]")))
            .collect()
    } else {
        vec![(ask.name.to_string(), path.clone())]
    };
    let each = judge.each.then_some(targets.len());

    let (kind, questions) = match &judge.verb {
        JudgeVerb::Feels { condition } => {
            let condition = evaluator.eval_text(condition, env)?;
            let questions = targets
                .iter()
                .map(|(id, path)| Question::Noul {
                    id: id.clone(),
                    path: path.clone(),
                    condition: condition.clone(),
                    instruction: instruction.clone(),
                })
                .collect();
            (PlannedKind::Feels, questions)
        }
        JudgeVerb::Pick { labels } => {
            check_criteria(labels.len(), "a `pick`", profile, span)?;
            let labels = labels
                .iter()
                .map(|label| {
                    Ok(ChoiceLabel {
                        name: label.name.clone(),
                        description: optional_text(label.description.as_ref(), env, evaluator)?,
                        what: optional_text(label.what.as_ref(), env, evaluator)?,
                        not_for: optional_text(label.not_for.as_ref(), env, evaluator)?,
                        examples: optional_texts(label.examples.as_ref(), env, evaluator)?,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            let questions = targets
                .iter()
                .map(|(id, path)| Question::Choice {
                    id: id.clone(),
                    path: path.clone(),
                    question: None,
                    labels: labels.clone(),
                    instruction: instruction.clone(),
                })
                .collect();
            (
                PlannedKind::Pick {
                    labels: labels.iter().map(|l| l.name.clone()).collect(),
                },
                questions,
            )
        }
        JudgeVerb::Rate { levels } => {
            let levels = levels
                .iter()
                .map(|level| {
                    Ok(ScoreLevel {
                        name: level.name.clone(),
                        situation: evaluator.eval_text(&level.situation, env)?,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            let names: Vec<String> = if levels.iter().all(|l| l.name.is_some()) {
                levels.iter().filter_map(|l| l.name.clone()).collect()
            } else {
                Vec::new()
            };
            let questions = targets
                .iter()
                .map(|(id, path)| Question::Score {
                    id: id.clone(),
                    path: path.clone(),
                    levels: levels.clone(),
                    instruction: instruction.clone(),
                })
                .collect();
            (
                PlannedKind::Rate {
                    levels: levels.len(),
                    names,
                },
                questions,
            )
        }
        JudgeVerb::PickAmong {
            question,
            by,
            allow_none,
        } => {
            // Spec 6.4a: `each` cannot combine with `pick among` (the compiler
            // rejects it), and the list is capped by the profile.
            let Value::List(items) = &subject else {
                return Err(Value::type_error("a list for `pick among`", &subject).at(span));
            };
            if items.is_empty() {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    "`pick among` needs at least one element",
                )
                .at(span));
            }
            check_criteria(items.len(), "a `pick among` list", profile, span)?;
            let mut labels = Vec::with_capacity(items.len() + 1);
            for (i, item) in items.iter().enumerate() {
                let description = match by {
                    Some(field) => match item {
                        Value::Record(fields) => {
                            fields.get(field).map(Value::to_text).ok_or_else(|| {
                                RuntimeError::new(
                                    RuntimeErrorCode::TypeError,
                                    format!("`pick among` element {i} has no field `{field}`"),
                                )
                                .at(span)
                            })?
                        }
                        other => {
                            return Err(Value::type_error(
                                &format!("a record with field `{field}` for `pick among ... by`"),
                                other,
                            )
                            .at(span));
                        }
                    },
                    None => item.to_text(),
                };
                labels.push(ChoiceLabel::described(format!("i{i}"), description));
            }
            if *allow_none {
                labels.push(ChoiceLabel::escape("none"));
            }
            let question = Question::Choice {
                id: ask.name.to_string(),
                path: path.clone(),
                question: Some(evaluator.eval_text(question, env)?),
                labels: labels.clone(),
                instruction: instruction.clone(),
            };
            (
                PlannedKind::PickAmong {
                    labels: labels.iter().map(|l| l.name.clone()).collect(),
                    items: items.clone(),
                },
                vec![question],
            )
        }
    };
    let questions = questions
        .into_iter()
        .enumerate()
        .map(|(i, question)| Planned {
            question,
            placements: placements(i),
        })
        .collect();
    Ok((
        PlannedAsk {
            name: ask.name.to_string(),
            kind,
            each,
            sample,
        },
        questions,
    ))
}

/// The `pick` label cap and the `pick among` list cap both come from the
/// profile (spec sections 6.4a and 10.6).
fn check_criteria(
    count: usize,
    what: &str,
    profile: &Profile,
    span: jevscript_syntax::Span,
) -> Result<(), RuntimeError> {
    let cap = profile.max_criteria_per_question as usize;
    if count > cap {
        return Err(RuntimeError::new(
            RuntimeErrorCode::PickTooMany,
            format!("{what} has {count} options, over the {cap} allowed by `{}`; filter and rank in code first", profile.model),
        )
        .at(span));
    }
    Ok(())
}

/// A subject's value and its resolved path (spec section 6.1).
fn resolve_subject(
    subject: &Subject,
    env: &mut Env,
    evaluator: &mut Evaluator<'_>,
) -> Result<(Value, String, Vec<Step>), RuntimeError> {
    let mut steps = Vec::with_capacity(subject.path.len());
    for step in &subject.path {
        steps.push(match step {
            SubjectStep::Field { name } => Step::Field(name.clone()),
            SubjectStep::Index { index } => {
                let index = evaluator.eval(index, env)?;
                Step::Index(whole_index(&index).map_err(|e| e.at(subject.span))?)
            }
        });
    }
    let value = value_at(env, &subject.root, &steps).map_err(|e| e.at(subject.span))?;
    Ok((value, subject.root.clone(), steps))
}

/// The value a root and steps reach in `env`.
fn value_at(env: &Env, root: &str, steps: &[Step]) -> Result<Value, RuntimeError> {
    let mut current = env.get(root).cloned().ok_or_else(|| {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("unknown subject `{root}`"),
        )
    })?;
    for step in steps {
        current = match (step, &current) {
            (Step::Field(name), Value::Record(fields)) => {
                fields.get(name).cloned().ok_or_else(|| {
                    RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!("subject `{root}` has no field `{name}`"),
                    )
                })?
            }
            (Step::Field(name), other) => {
                return Err(Value::type_error(&format!("a record for `.{name}`"), other));
            }
            (Step::Index(i), Value::List(items)) => items.get(*i).cloned().ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("index {i} is out of range for subject `{root}`"),
                )
            })?,
            (Step::Index(_), other) => return Err(Value::type_error("a list to index", other)),
        };
    }
    Ok(current)
}

fn whole_index(value: &Value) -> Result<usize, RuntimeError> {
    match value.as_number() {
        Some(n) if n >= 0.0 && n.fract() == 0.0 => Ok(n as usize),
        _ => Err(Value::type_error("a whole non-negative index", value)),
    }
}

/// The path as Jev sees it: `obs.summary`, `files[3]`.
fn path_text(root: &str, steps: &[Step]) -> String {
    let mut path = root.to_string();
    for step in steps {
        match step {
            Step::Field(name) => {
                path.push('.');
                path.push_str(name);
            }
            Step::Index(i) => path.push_str(&format!("[{i}]")),
        }
    }
    path
}

/// Parse a path written as text, as a `compare` entry is (spec section 6.8).
fn parse_path(text: &str) -> Result<(String, Vec<Step>), RuntimeError> {
    let bad = || {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("`{text}` is not a state path"),
        )
    };
    let mut chars = text.char_indices().peekable();
    let ident = |chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>| -> String {
        let mut name = String::new();
        while let Some((_, c)) = chars.peek() {
            if c.is_ascii_alphanumeric() || *c == '_' {
                name.push(*c);
                chars.next();
            } else {
                break;
            }
        }
        name
    };
    let root = ident(&mut chars);
    if root.is_empty() {
        return Err(bad());
    }
    let mut steps = Vec::new();
    while let Some((_, c)) = chars.next() {
        match c {
            '.' => {
                let name = ident(&mut chars);
                if name.is_empty() {
                    return Err(bad());
                }
                steps.push(Step::Field(name));
            }
            '[' => {
                let mut digits = String::new();
                for (_, d) in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                    digits.push(d);
                }
                steps.push(Step::Index(digits.parse().map_err(|_| bad())?));
            }
            _ => return Err(bad()),
        }
    }
    Ok((root, steps))
}

/// The state paths a `compare` names are read alongside the subject, so they
/// must be in the state too (spec section 6.8).
fn compare_placements(
    instruction: Option<&Instruction>,
    env: &Env,
) -> Result<Vec<Placement>, RuntimeError> {
    let Some(instruction) = instruction else {
        return Ok(Vec::new());
    };
    instruction
        .compare
        .iter()
        .map(|path| {
            let (root, steps) = parse_path(path)?;
            let value = value_at(env, &root, &steps)?;
            Ok(Placement {
                root,
                steps,
                value: value.to_json(),
            })
        })
        .collect()
}

/// The detail block as an [`Instruction`] (spec section 6.8).
fn instruction_of(
    detail: Option<&Detail>,
    env: &mut Env,
    evaluator: &mut Evaluator<'_>,
) -> Result<Option<Instruction>, RuntimeError> {
    let Some(detail) = detail else {
        return Ok(None);
    };
    let instruction = Instruction {
        focus: optional_text(detail.focus.as_ref(), env, evaluator)?,
        note: optional_text(detail.note.as_ref(), env, evaluator)?,
        compare: optional_texts(detail.compare.as_ref(), env, evaluator)?,
        yes: optional_texts(detail.yes.as_ref(), env, evaluator)?,
        no: optional_texts(detail.no.as_ref(), env, evaluator)?,
    };
    Ok(if instruction.is_empty() {
        None
    } else {
        Some(instruction)
    })
}

fn optional_text(
    expr: Option<&Expr>,
    env: &mut Env,
    evaluator: &mut Evaluator<'_>,
) -> Result<Option<String>, RuntimeError> {
    expr.map(|e| evaluator.eval_text(e, env)).transpose()
}

/// A list of texts, or one text as a one-element list.
fn optional_texts(
    expr: Option<&Expr>,
    env: &mut Env,
    evaluator: &mut Evaluator<'_>,
) -> Result<Vec<String>, RuntimeError> {
    let Some(expr) = expr else {
        return Ok(Vec::new());
    };
    Ok(match evaluator.eval(expr, env)? {
        Value::List(items) => items.iter().map(Value::to_text).collect(),
        Value::None => Vec::new(),
        other => vec![other.to_text()],
    })
}

/// Place a subject's value at its path in the state, creating the objects and
/// arrays on the way (spec section 6.9). A list index not otherwise named is
/// `null`, so the path still addresses the value Jev is told to inspect. A
/// whole value already present at a prefix is kept, since it contains the
/// part being placed.
fn place(state: &mut BTreeMap<String, Json>, placement: &Placement) {
    let node = state.entry(placement.root.clone()).or_insert(Json::Null);
    place_at(node, &placement.steps, &placement.value);
}

fn place_at(node: &mut Json, steps: &[Step], value: &Json) {
    let Some((step, rest)) = steps.split_first() else {
        *node = value.clone();
        return;
    };
    match step {
        Step::Field(name) => {
            if node.is_null() {
                *node = Json::Object(serde_json::Map::new());
            }
            let Json::Object(fields) = node else {
                return;
            };
            place_at(
                fields.entry(name.clone()).or_insert(Json::Null),
                rest,
                value,
            );
        }
        Step::Index(i) => {
            if node.is_null() {
                *node = Json::Array(Vec::new());
            }
            let Json::Array(items) = node else {
                return;
            };
            if items.len() <= *i {
                items.resize(i + 1, Json::Null);
            }
            place_at(&mut items[*i], rest, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Tokenizer;
    use jevscript_ir::{PickLabel, RateLevel, TextPart};
    use jevscript_syntax::Span;
    use serde_json::json;

    fn profile() -> Profile {
        // Test data, not limits the runtime knows.
        Profile {
            model: "jev-test".into(),
            endpoint: "http://localhost".into(),
            total_tokens: 100_000,
            state_plus_question_tokens: 50_000,
            max_questions_per_request: 3,
            max_criteria_per_question: 4,
            tokenizer: Tokenizer::Chars4,
            price_per_million_input_usd: 0.0,
            price_per_million_output_usd: 0.0,
            aliases: None,
        }
    }

    fn text(t: &str) -> Expr {
        Expr::Text {
            parts: vec![TextPart::Literal { value: t.into() }],
            span: Span::default(),
        }
    }

    fn subject(root: &str, path: Vec<SubjectStep>) -> Subject {
        Subject {
            root: root.into(),
            path,
            state_path: root.into(),
            span: Span::default(),
        }
    }

    fn feels(root: &str, path: Vec<SubjectStep>, condition: &str) -> Judge {
        Judge {
            each: false,
            subject: subject(root, path),
            verb: JudgeVerb::Feels {
                condition: text(condition),
            },
            detail: None,
            request_group: 0,
            span: Span::default(),
        }
    }

    fn env() -> Env {
        let mut env = Env::new();
        env.assign(
            "obs",
            Value::Record(BTreeMap::from([
                ("summary".to_string(), Value::Text("done".into())),
                ("tests".to_string(), Value::Text("pass".into())),
                (
                    "files".to_string(),
                    Value::List(vec![Value::Text("a.rs".into())]),
                ),
            ])),
        );
        env.assign(
            "files",
            Value::List(vec![
                Value::Text("a.rs".into()),
                Value::Text("b.rs".into()),
                Value::Text("c.rs".into()),
                Value::Text("d.rs".into()),
            ]),
        );
        env.assign("unrelated", Value::Text("never sent".into()));
        env
    }

    #[test]
    fn the_state_is_exactly_the_named_fields() {
        // Spec 6.9: judge `obs.summary` from a three-field record and only
        // that field reaches Jev; `unrelated` never does.
        let judge = feels(
            "obs",
            vec![SubjectStep::Field {
                name: "summary".into(),
            }],
            "states the work is complete",
        );
        let plan = build_request(
            &[Ask {
                name: "claims_done",
                judge: &judge,
            }],
            &mut env(),
            &profile(),
        )
        .expect("builds");
        assert_eq!(plan.requests.len(), 1);
        let request = &plan.requests[0];
        assert_eq!(
            request.state,
            BTreeMap::from([("obs".to_string(), json!({"summary": "done"}))])
        );
        assert_eq!(request.model, "jev-test");
        assert_eq!(
            request.questions,
            vec![Question::Noul {
                id: "claims_done".into(),
                path: "obs.summary".into(),
                condition: "states the work is complete".into(),
                instruction: None,
            }]
        );
        assert_eq!(plan.asks[0].kind, PlannedKind::Feels);
        assert_eq!(plan.asks[0].each, None);
    }

    #[test]
    fn subjects_in_one_request_merge_into_one_state_tree() {
        let a = feels(
            "obs",
            vec![SubjectStep::Field {
                name: "summary".into(),
            }],
            "x",
        );
        let b = feels(
            "obs",
            vec![SubjectStep::Field {
                name: "tests".into(),
            }],
            "y",
        );
        let index = Expr::Number {
            value: 2.0,
            span: Span::default(),
        };
        let c = feels("files", vec![SubjectStep::Index { index }], "z");
        let plan = build_request(
            &[
                Ask {
                    name: "a",
                    judge: &a,
                },
                Ask {
                    name: "b",
                    judge: &b,
                },
                Ask {
                    name: "c",
                    judge: &c,
                },
            ],
            &mut env(),
            &profile(),
        )
        .expect("builds");
        let state = &plan.requests[0].state;
        assert_eq!(state["obs"], json!({"summary": "done", "tests": "pass"}));
        // `files[2]` sits at index 2; the unnamed slots are null so the path
        // still addresses it.
        assert_eq!(state["files"], json!([null, null, "c.rs"]));
        assert_eq!(plan.requests[0].questions[2].path(), "files[2]");
    }

    #[test]
    fn each_asks_one_question_per_element_and_splits_at_the_profile_cap() {
        // Spec 6.5: paths `files[0]`, `files[1]`, ...; split across requests
        // at `max_questions_per_request` (3 in this profile) and recorded as
        // one logical judgment.
        let judge = Judge {
            each: true,
            ..feels("files", vec![], "is unrelated to the issue")
        };
        let plan = build_request(
            &[Ask {
                name: "off",
                judge: &judge,
            }],
            &mut env(),
            &profile(),
        )
        .expect("builds");
        assert_eq!(plan.requests.len(), 2);
        assert_eq!(plan.requests[0].questions.len(), 3);
        assert_eq!(plan.requests[1].questions.len(), 1);
        let ids: Vec<&str> = plan
            .requests
            .iter()
            .flat_map(|r| r.questions.iter().map(Question::id))
            .collect();
        assert_eq!(ids, vec!["off[0]", "off[1]", "off[2]", "off[3]"]);
        assert_eq!(plan.requests[1].questions[0].path(), "files[3]");
        // Spec 6.9: each request carries exactly the elements it judges,
        // with `null` at the positions before them it does not, so the paths
        // still hold; positions after the last judged element are omitted.
        assert_eq!(
            plan.requests[0].state["files"],
            json!(["a.rs", "b.rs", "c.rs"])
        );
        assert_eq!(
            plan.requests[1].state["files"],
            json!([null, null, null, "d.rs"])
        );
        assert_eq!(plan.asks[0].each, Some(4));
        assert_eq!(
            plan.asks[0].question_ids(),
            vec!["off[0]", "off[1]", "off[2]", "off[3]"]
        );
    }

    #[test]
    fn each_over_a_non_list_is_a_type_error() {
        let judge = Judge {
            each: true,
            ..feels("unrelated", vec![], "x")
        };
        let error = build_request(
            &[Ask {
                name: "e",
                judge: &judge,
            }],
            &mut env(),
            &profile(),
        )
        .expect_err("not a list");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
    }

    #[test]
    fn pick_labels_and_named_levels_are_evaluated_and_ordered() {
        let pick = Judge {
            verb: JudgeVerb::Pick {
                labels: vec![
                    PickLabel {
                        name: "billing".into(),
                        escape: false,
                        description: None,
                        what: Some(text("charges")),
                        not_for: Some(text("tracking")),
                        examples: Some(Expr::List {
                            items: vec![text("I was charged twice")],
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    },
                    PickLabel {
                        name: "other".into(),
                        escape: true,
                        description: None,
                        what: None,
                        not_for: None,
                        examples: None,
                        span: Span::default(),
                    },
                ],
            },
            ..feels(
                "obs",
                vec![SubjectStep::Field {
                    name: "summary".into(),
                }],
                "",
            )
        };
        let rate = Judge {
            verb: JudgeVerb::Rate {
                levels: vec![
                    RateLevel {
                        name: Some("unchanged".into()),
                        situation: text("each step leaves the state unchanged"),
                        span: Span::default(),
                    },
                    RateLevel {
                        name: Some("advancing".into()),
                        situation: text("the state is moving toward the goal"),
                        span: Span::default(),
                    },
                ],
            },
            ..feels(
                "obs",
                vec![SubjectStep::Field {
                    name: "summary".into(),
                }],
                "",
            )
        };
        let plan = build_request(
            &[
                Ask {
                    name: "dept",
                    judge: &pick,
                },
                Ask {
                    name: "progress",
                    judge: &rate,
                },
            ],
            &mut env(),
            &profile(),
        )
        .expect("builds");
        let Question::Choice {
            labels, question, ..
        } = &plan.requests[0].questions[0]
        else {
            panic!()
        };
        assert_eq!(question, &None);
        assert_eq!(labels[0].what.as_deref(), Some("charges"));
        assert_eq!(labels[0].not_for.as_deref(), Some("tracking"));
        assert_eq!(labels[0].examples, vec!["I was charged twice"]);
        assert_eq!(labels[1], ChoiceLabel::escape("other"));
        let Question::Score { levels, .. } = &plan.requests[0].questions[1] else {
            panic!()
        };
        assert_eq!(levels[1].situation, "the state is moving toward the goal");
        assert_eq!(
            plan.asks[0].kind,
            PlannedKind::Pick {
                labels: vec!["billing".into(), "other".into()]
            }
        );
        assert_eq!(
            plan.asks[1].kind,
            PlannedKind::Rate {
                levels: 2,
                names: vec!["unchanged".into(), "advancing".into()]
            }
        );
    }

    #[test]
    fn pick_among_labels_items_by_index_and_caps_at_the_profile() {
        // Spec 6.4a: labels `i0..iN`, descriptions from the `by` field, `none`
        // when allowed, `pick_too_many` over `max_criteria_per_question`.
        let mut env = Env::new();
        env.assign(
            "prs",
            Value::List(vec![
                Value::Record(BTreeMap::from([(
                    "title".to_string(),
                    Value::Text("Fix typo".into()),
                )])),
                Value::Record(BTreeMap::from([(
                    "title".to_string(),
                    Value::Text("Rewrite auth".into()),
                )])),
            ]),
        );
        let judge = Judge {
            verb: JudgeVerb::PickAmong {
                question: text("Which is safest to merge first?"),
                by: Some("title".into()),
                allow_none: true,
            },
            ..feels("prs", vec![], "")
        };
        let plan = build_request(
            &[Ask {
                name: "next",
                judge: &judge,
            }],
            &mut env,
            &profile(),
        )
        .expect("builds");
        let Question::Choice {
            labels,
            question,
            path,
            ..
        } = &plan.requests[0].questions[0]
        else {
            panic!()
        };
        assert_eq!(path, "prs");
        assert_eq!(question.as_deref(), Some("Which is safest to merge first?"));
        assert_eq!(
            labels,
            &vec![
                ChoiceLabel::described("i0", "Fix typo"),
                ChoiceLabel::described("i1", "Rewrite auth"),
                ChoiceLabel::escape("none"),
            ]
        );
        let PlannedKind::PickAmong { labels, items } = &plan.asks[0].kind else {
            panic!()
        };
        assert_eq!(labels, &vec!["i0", "i1", "none"]);
        assert_eq!(items.len(), 2);

        // Without `by`, the text form describes each element.
        env.assign(
            "names",
            Value::List(vec![Value::Text("x".into()), Value::Number(2.0)]),
        );
        let plain = Judge {
            verb: JudgeVerb::PickAmong {
                question: text("q"),
                by: None,
                allow_none: false,
            },
            ..feels("names", vec![], "")
        };
        let plan = build_request(
            &[Ask {
                name: "n",
                judge: &plain,
            }],
            &mut env,
            &profile(),
        )
        .expect("builds");
        let Question::Choice { labels, .. } = &plan.requests[0].questions[0] else {
            panic!()
        };
        assert_eq!(
            labels,
            &vec![
                ChoiceLabel::described("i0", "x"),
                ChoiceLabel::described("i1", "2")
            ]
        );

        // Over the cap (4 in this profile).
        env.assign(
            "many",
            Value::List((0..5).map(|i| Value::Number(f64::from(i))).collect()),
        );
        let many = Judge {
            verb: JudgeVerb::PickAmong {
                question: text("q"),
                by: None,
                allow_none: false,
            },
            ..feels("many", vec![], "")
        };
        let error = build_request(
            &[Ask {
                name: "m",
                judge: &many,
            }],
            &mut env,
            &profile(),
        )
        .expect_err("too many");
        assert_eq!(error.code, RuntimeErrorCode::PickTooMany);
        assert!(!error.retryable);

        // Empty list, and a missing `by` field.
        env.assign("none_at_all", Value::List(vec![]));
        let empty = Judge {
            verb: JudgeVerb::PickAmong {
                question: text("q"),
                by: None,
                allow_none: true,
            },
            ..feels("none_at_all", vec![], "")
        };
        assert_eq!(
            build_request(
                &[Ask {
                    name: "e",
                    judge: &empty
                }],
                &mut env,
                &profile()
            )
            .expect_err("empty")
            .code,
            RuntimeErrorCode::TypeError
        );
        let wrong_field = Judge {
            verb: JudgeVerb::PickAmong {
                question: text("q"),
                by: Some("nope".into()),
                allow_none: true,
            },
            ..feels("prs", vec![], "")
        };
        assert_eq!(
            build_request(
                &[Ask {
                    name: "w",
                    judge: &wrong_field
                }],
                &mut env,
                &profile()
            )
            .expect_err("field")
            .code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn detail_blocks_become_instructions_and_compare_paths_join_the_state() {
        // Spec 6.8: focus/note/compare/yes/no; `compare` paths are read
        // alongside the subject so they must be in the state.
        let judge = Judge {
            detail: Some(Detail {
                focus: Some(text("Look for an explicit statement.")),
                note: Some(text("The agent writes in first person.")),
                compare: Some(Expr::List {
                    items: vec![text("obs.tests")],
                    span: Span::default(),
                }),
                yes: Some(Expr::List {
                    items: vec![text("Done.")],
                    span: Span::default(),
                }),
                no: Some(text("I will now run the tests.")),
                sample: Some(true),
            }),
            ..feels(
                "obs",
                vec![SubjectStep::Field {
                    name: "summary".into(),
                }],
                "states the work is complete",
            )
        };
        let plan = build_request(
            &[Ask {
                name: "claims_done",
                judge: &judge,
            }],
            &mut env(),
            &profile(),
        )
        .expect("builds");
        let request = &plan.requests[0];
        assert_eq!(
            request.state["obs"],
            json!({"summary": "done", "tests": "pass"})
        );
        assert_eq!(
            request.questions[0].instruction(),
            Some(&Instruction {
                focus: Some("Look for an explicit statement.".into()),
                note: Some("The agent writes in first person.".into()),
                compare: vec!["obs.tests".into()],
                yes: vec!["Done.".into()],
                no: vec!["I will now run the tests.".into()],
            })
        );
        assert!(plan.asks[0].sample);
    }

    #[test]
    fn the_size_check_runs_on_every_request() {
        // Spec 6.10, through the profile's limits.
        let mut env = env();
        env.assign("big", Value::Text("x".repeat(4000)));
        let judge = feels("big", vec![], "is long");
        let small = Profile {
            total_tokens: 500,
            state_plus_question_tokens: 500,
            ..profile()
        };
        let error = build_request(
            &[Ask {
                name: "b",
                judge: &judge,
            }],
            &mut env,
            &small,
        )
        .expect_err("too large");
        assert_eq!(error.code, RuntimeErrorCode::StateTooLarge);
        assert!(error.message.contains("`big`"), "{}", error.message);
    }

    #[test]
    fn a_subject_that_does_not_resolve_is_a_type_error() {
        let judge = feels(
            "obs",
            vec![SubjectStep::Field {
                name: "nope".into(),
            }],
            "x",
        );
        let error = build_request(
            &[Ask {
                name: "n",
                judge: &judge,
            }],
            &mut env(),
            &profile(),
        )
        .expect_err("no field");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        let judge = feels("missing", vec![], "x");
        assert_eq!(
            build_request(
                &[Ask {
                    name: "n",
                    judge: &judge
                }],
                &mut env(),
                &profile()
            )
            .expect_err("no root")
            .code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn compare_paths_parse_fields_and_indexes() {
        let (root, steps) = parse_path("obs.files[0].name").expect("parses");
        assert_eq!(root, "obs");
        assert_eq!(
            steps,
            vec![
                Step::Field("files".into()),
                Step::Index(0),
                Step::Field("name".into())
            ]
        );
        assert!(parse_path("").is_err());
        assert!(parse_path("obs.").is_err());
        assert!(parse_path("obs[x]").is_err());
    }
}
