//! Expression evaluation (spec sections 2.7, 4.3, 5.1, 5.2, 5.7 and 8.2).
//!
//! The evaluator is pure except for the effects it delegates through
//! [`Effects`]: judgments, def calls, capability calls, `focus`, `trail`, the
//! two recorded draws behind `now()` and `random()`, and `log`. The statement
//! interpreter implements that trait; anything that only needs to evaluate a
//! detail text or a label description uses [`PureEffects`], which refuses every
//! effect, so a question can never reach Jev or an adapter from inside a
//! question.
//!
//! Token estimation for `tokens()` and `chunk()` comes from the profile the
//! evaluator is built with (spec section 10.6), never from a literal.

use jevscript_ir::{Arg, BinaryOp, Expr, Judge, TextPart, UnaryOp};
use jevscript_syntax::Span;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::capability::CallArgs;
use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::profile::Profile;
use crate::record::LogRecord;
use crate::tokens::{TokenEstimator, estimator_for};
use crate::value::{Handle, Value};

/// The builtin function names (spec section 5.7). They are always available
/// and are resolved before defs, so a def cannot shadow one.
pub const BUILTINS: &[&str] = &[
    "len", "count", "max", "min", "sum", "top", "join", "split", "lines", "head", "tail", "tokens",
    "chunk", "hash", "now", "random", "zip", "keys", "values", "items", "text", "number", "bool",
];

/// Whether `name` is a builtin function.
pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// What a capability call is aimed at (spec section 5.2): a declared
/// capability by name, or a handle an adapter minted.
#[derive(Debug, Clone, PartialEq)]
pub enum CallTarget {
    /// `claude.spawn(...)`: the capability itself.
    Capability(String),
    /// `dev.send(...)`: a handle, which knows which capability owns it.
    Handle(Handle),
}

impl CallTarget {
    /// The capability the call reaches, which for a handle is its owner.
    pub fn capability(&self) -> &str {
        match self {
            CallTarget::Capability(name) => name,
            CallTarget::Handle(handle) => &handle.capability,
        }
    }
}

/// The effects an expression can have. The statement interpreter implements
/// this; the evaluator itself never records, pauses or calls anything.
pub trait Effects {
    /// Ask Jev (spec section 6). The interpreter batches consecutive judgments
    /// by their `request_group`, records the request and the answers, and
    /// samples if asked, which is why this is not the evaluator's job.
    fn judge(&mut self, judge: &Judge, env: &mut Env) -> Result<Value, RuntimeError>;

    /// Call a def (spec section 8). `name` is qualified when the call was
    /// (`std.stuck`). The implementer enforces the recursion limit.
    fn call_def(&mut self, name: &str, args: CallArgs) -> Result<Value, RuntimeError>;

    /// Call a capability verb (spec section 9.6). Recorded as a `call` event.
    fn call_capability(
        &mut self,
        target: CallTarget,
        verb: &str,
        args: CallArgs,
    ) -> Result<Value, RuntimeError>;

    /// `focus <text> on "<purpose>", max <tokens>` (spec section 7.3).
    fn focus(&mut self, text: Value, purpose: String, max: f64) -> Result<Value, RuntimeError>;

    /// `trail <n>` (spec section 7.5): the last `n` step records.
    fn trail(&mut self, n: usize) -> Result<Value, RuntimeError>;

    /// `now()`: ISO text from the clock, recorded as a `draw` event of kind
    /// `now` so that replay returns the same text (spec section 10.3).
    fn draw_now(&mut self) -> Result<String, RuntimeError>;

    /// `random()` and every sampling draw: a number in `[0, 1)` from the run's
    /// random source, recorded as a `draw` event of kind `random`.
    fn draw_random(&mut self) -> Result<f64, RuntimeError>;

    /// `log` (spec section 5.8): record the evaluated line. It returns nothing
    /// to the program, which receives the logged value unchanged. Contexts
    /// with nowhere to record a log refuse it.
    ///
    /// # Errors
    ///
    /// `type_error` by default; `replay_diverged` when a replayed log differs.
    fn log(&mut self, record: LogRecord) -> Result<(), RuntimeError> {
        Err(RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("a `log` ({}) is not allowed here", record.level.as_str()),
        )
        .at(record.source))
    }
}

/// Effects for contexts that must stay pure: detail texts, label descriptions,
/// `pick among` questions. Every method is a `type_error`.
#[derive(Debug, Default, Clone, Copy)]
pub struct PureEffects;

impl PureEffects {
    fn refuse(what: &str) -> RuntimeError {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("{what} is not allowed inside a question"),
        )
    }
}

impl Effects for PureEffects {
    fn judge(&mut self, _judge: &Judge, _env: &mut Env) -> Result<Value, RuntimeError> {
        Err(Self::refuse("a judgment"))
    }

    fn call_def(&mut self, name: &str, _args: CallArgs) -> Result<Value, RuntimeError> {
        Err(Self::refuse(&format!("calling `{name}`")))
    }

    fn call_capability(
        &mut self,
        target: CallTarget,
        verb: &str,
        _args: CallArgs,
    ) -> Result<Value, RuntimeError> {
        Err(Self::refuse(&format!(
            "calling `{}.{verb}`",
            target.capability()
        )))
    }

    fn focus(&mut self, _text: Value, _purpose: String, _max: f64) -> Result<Value, RuntimeError> {
        Err(Self::refuse("`focus`"))
    }

    fn trail(&mut self, _n: usize) -> Result<Value, RuntimeError> {
        Err(Self::refuse("`trail`"))
    }

    fn draw_now(&mut self) -> Result<String, RuntimeError> {
        Err(Self::refuse("`now()`"))
    }

    fn draw_random(&mut self) -> Result<f64, RuntimeError> {
        Err(Self::refuse("`random()`"))
    }
}

/// Lexical scopes (spec section 5.3), plus the read-only inputs and the
/// capability names a program declared.
///
/// Variables are block-scoped. Assignment to an outer variable from an inner
/// block updates the outer one; a first assignment lands in the innermost
/// block. Reading before assignment is a compile error, so an unknown name here
/// is a `type_error` rather than `unassigned_read`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Env {
    scopes: Vec<BTreeMap<String, Value>>,
    inputs: BTreeMap<String, Value>,
    capabilities: BTreeSet<String>,
}

impl Env {
    /// An environment with one empty scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
            inputs: BTreeMap::new(),
            capabilities: BTreeSet::new(),
        }
    }

    /// The same environment with the program's inputs readable by name.
    #[must_use]
    pub fn with_inputs(mut self, inputs: BTreeMap<String, Value>) -> Self {
        self.inputs = inputs;
        self
    }

    /// The same environment with these capability names declared.
    #[must_use]
    pub fn with_capabilities<I: IntoIterator<Item = S>, S: Into<String>>(
        mut self,
        names: I,
    ) -> Self {
        self.capabilities = names.into_iter().map(Into::into).collect();
        self
    }

    /// Open a block.
    pub fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    /// Close a block, dropping its variables. The outermost scope stays.
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// How many blocks are open.
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }

    /// A variable, innermost block first, then an input.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.inputs.get(name))
    }

    /// Whether `name` is a declared capability.
    pub fn is_capability(&self, name: &str) -> bool {
        self.capabilities.contains(name)
    }

    /// Whether `name` is a program input.
    pub fn is_input(&self, name: &str) -> bool {
        self.inputs.contains_key(name)
    }

    /// Bind `name` in the innermost block, shadowing any outer binding. This
    /// is what loop variables, comprehension variables and parameters do.
    pub fn define(&mut self, name: impl Into<String>, value: Value) {
        self.scopes
            .last_mut()
            .expect("at least one scope")
            .insert(name.into(), value);
    }

    /// `x = expr` (spec section 5.3): update the nearest block that already
    /// binds `name`, else bind it in the innermost block.
    pub fn assign(&mut self, name: &str, value: Value) {
        if let Some(scope) = self.scopes.iter_mut().rev().find(|s| s.contains_key(name)) {
            scope.insert(name.to_string(), value);
        } else {
            self.define(name, value);
        }
    }

    /// `x.a.b = expr` (spec section 5.3): only on records held in variables,
    /// never on inputs or capability results.
    ///
    /// # Errors
    ///
    /// `type_error` if `root` is not a variable holding a record, or a step of
    /// the path is not a record.
    pub fn assign_field(
        &mut self,
        root: &str,
        path: &[String],
        value: Value,
    ) -> Result<(), RuntimeError> {
        let Some(scope) = self.scopes.iter_mut().rev().find(|s| s.contains_key(root)) else {
            let what = if self.inputs.contains_key(root) {
                format!("input `{root}` is read-only")
            } else {
                format!("`{root}` is not a variable")
            };
            return Err(RuntimeError::new(RuntimeErrorCode::TypeError, what));
        };
        let mut target = scope.get_mut(root).expect("checked above");
        let (last, steps) = path.split_last().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::TypeError,
                "a field assignment needs a field",
            )
        })?;
        for step in steps {
            let Value::Record(fields) = target else {
                return Err(Value::type_error("a record", target));
            };
            target = fields.get_mut(step).ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("record has no field `{step}`"),
                )
            })?;
        }
        let Value::Record(fields) = target else {
            return Err(Value::type_error("a record", target));
        };
        fields.insert(last.clone(), value);
        Ok(())
    }
}

/// Evaluates expressions against an [`Env`], delegating effects.
pub struct Evaluator<'a> {
    effects: &'a mut dyn Effects,
    estimator: Box<dyn TokenEstimator>,
}

impl<'a> Evaluator<'a> {
    /// An evaluator whose `tokens()` and `chunk()` use `profile`'s estimator.
    pub fn new(effects: &'a mut dyn Effects, profile: &Profile) -> Self {
        Self {
            effects,
            estimator: estimator_for(&profile.tokenizer),
        }
    }

    /// The estimator this evaluator measures text with.
    pub fn estimator(&self) -> &dyn TokenEstimator {
        self.estimator.as_ref()
    }

    /// Evaluate `expr`.
    ///
    /// # Errors
    ///
    /// `type_error` for any mismatch section 4 or 5 names, and whatever an
    /// effect raises.
    pub fn eval(&mut self, expr: &Expr, env: &mut Env) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Number { value, .. } => Ok(Value::Number(*value)),
            Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
            Expr::None { .. } => Ok(Value::None),
            Expr::Text { parts, span } => self.eval_text_parts(parts, env).map_err(at(*span)),
            Expr::Name { name, span } => match env.get(name) {
                Some(value) => Ok(value.clone()),
                // A capability in value position, as in `claude.spawn in
                // tree` (spec section 14.1), is a handle to the capability
                // itself, so an adapter can be told which one is meant.
                None if env.is_capability(name) => Ok(Value::Handle(Handle {
                    capability: name.clone(),
                    id: name.clone(),
                    fields: BTreeMap::new(),
                })),
                None => Err(RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("unknown name `{name}`"),
                )
                .at(*span)),
            },
            Expr::List { items, .. } => Ok(Value::List(
                items
                    .iter()
                    .map(|item| self.eval(item, env))
                    .collect::<Result<_, _>>()?,
            )),
            Expr::Record { fields, .. } => {
                let mut record = BTreeMap::new();
                for field in fields {
                    record.insert(field.name.clone(), self.eval(&field.value, env)?);
                }
                Ok(Value::Record(record))
            }
            Expr::Field { target, name, span } => {
                // A property on a capability or a handle is a zero-argument
                // verb call (spec section 5.2): `tree.tests_pass`,
                // `dev.observe`, `tree.diff.files`. The compiler keeps them
                // as field accesses, so they are resolved here.
                if let Expr::Name { name: root, .. } = &**target
                    && env.is_capability(root)
                    && env.get(root).is_none()
                {
                    return self.effects.call_capability(
                        CallTarget::Capability(root.clone()),
                        name,
                        CallArgs::default(),
                    );
                }
                let target = self.eval(target, env)?;
                match (&target, field(&target, name)) {
                    (Value::Handle(handle), Err(_)) => self.effects.call_capability(
                        CallTarget::Handle(handle.clone()),
                        name,
                        CallArgs::default(),
                    ),
                    (_, result) => result.map_err(at(*span)),
                }
            }
            Expr::Index {
                target,
                index,
                span,
            } => {
                let target = self.eval(target, env)?;
                let index = self.eval(index, env)?;
                index_into(&target, &index).map_err(at(*span))
            }
            Expr::Call {
                callee, args, span, ..
            } => self.eval_call(callee, args, *span, env),
            Expr::Unary { op, operand, span } => {
                let operand = self.eval(operand, env)?;
                match op {
                    UnaryOp::Not => Ok(Value::Bool(!operand.is_truthy())),
                    UnaryOp::Neg => {
                        operand
                            .as_number()
                            .map(|n| Value::Number(-n))
                            .ok_or_else(|| {
                                RuntimeError::new(
                                    RuntimeErrorCode::TypeError,
                                    format!(
                                        "unary `-` needs a number, got {}",
                                        operand.type_name()
                                    ),
                                )
                                .at(*span)
                            })
                    }
                }
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.eval_binary(*op, left, right, *span, env),
            Expr::Is {
                target,
                label,
                span,
            } => {
                let target = self.eval(target, env)?;
                is_label(&target, label).map_err(at(*span))
            }
            Expr::Comprehension {
                expr,
                names,
                iterable,
                test,
                span,
            } => {
                let items = match self.eval(iterable, env)? {
                    Value::List(items) => items,
                    other => return Err(Value::type_error("a list to iterate", &other).at(*span)),
                };
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    env.push_scope();
                    let result = bind_names(env, names, item)
                        .map_err(at(*span))
                        .and_then(|()| match test {
                            Some(test) => self.eval(test, env).map(|v| v.is_truthy()),
                            None => Ok(true),
                        })
                        .and_then(|keep| {
                            if keep {
                                self.eval(expr, env).map(Some)
                            } else {
                                Ok(None)
                            }
                        });
                    env.pop_scope();
                    if let Some(value) = result? {
                        out.push(value);
                    }
                }
                Ok(Value::List(out))
            }
            Expr::Focus { text, on, max, .. } => {
                let text = self.eval(text, env)?;
                let purpose = self.eval(on, env)?.to_text();
                self.effects.focus(text, purpose, *max)
            }
            Expr::Trail { count, .. } => self.effects.trail(count.max(0.0) as usize),
            Expr::Judge(judge) => self.effects.judge(judge, env),
            Expr::Log {
                level,
                value,
                fields,
                span,
            } => {
                let value = self.eval(value, env)?;
                let mut record = BTreeMap::new();
                for field in fields {
                    record.insert(field.name.clone(), self.eval(&field.value, env)?);
                }
                self.effects.log(LogRecord {
                    level: *level,
                    message: value.to_text(),
                    fields: record,
                    task: String::new(),
                    source: *span,
                })?;
                Ok(value)
            }
        }
    }

    /// Evaluate `expr` and take its text form (spec section 4.2).
    ///
    /// # Errors
    ///
    /// As [`Evaluator::eval`].
    pub fn eval_text(&mut self, expr: &Expr, env: &mut Env) -> Result<String, RuntimeError> {
        self.eval(expr, env).map(|v| v.to_text())
    }

    /// Evaluate `expr` and test its truthiness (spec section 4.1).
    ///
    /// # Errors
    ///
    /// As [`Evaluator::eval`].
    pub fn eval_condition(&mut self, expr: &Expr, env: &mut Env) -> Result<bool, RuntimeError> {
        self.eval(expr, env).map(|v| v.is_truthy())
    }

    /// Evaluate call arguments into positional and named (spec section 5.2).
    ///
    /// # Errors
    ///
    /// As [`Evaluator::eval`].
    pub fn eval_args(&mut self, args: &[Arg], env: &mut Env) -> Result<CallArgs, RuntimeError> {
        let mut out = CallArgs::default();
        for arg in args {
            let value = self.eval(&arg.value, env)?;
            match &arg.name {
                Some(name) => {
                    out.named.insert(name.clone(), value);
                }
                None => out.positional.push(value),
            }
        }
        Ok(out)
    }

    fn eval_text_parts(
        &mut self,
        parts: &[TextPart],
        env: &mut Env,
    ) -> Result<Value, RuntimeError> {
        let mut text = String::new();
        for part in parts {
            match part {
                TextPart::Literal { value } => text.push_str(value),
                TextPart::Interpolation { expr } => text.push_str(&self.eval(expr, env)?.to_text()),
            }
        }
        Ok(Value::Text(text))
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        span: Span,
        env: &mut Env,
    ) -> Result<Value, RuntimeError> {
        // `and` / `or` short-circuit and answer with the deciding operand's
        // truthiness (spec section 5.1).
        match op {
            BinaryOp::And => {
                let left = self.eval(left, env)?;
                if !left.is_truthy() {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(self.eval(right, env)?.is_truthy()));
            }
            BinaryOp::Or => {
                let left = self.eval(left, env)?;
                if left.is_truthy() {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(self.eval(right, env)?.is_truthy()));
            }
            _ => {}
        }
        let left = self.eval(left, env)?;
        let right = self.eval(right, env)?;
        binary(op, &left, &right).map_err(at(span))
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        span: Span,
        env: &mut Env,
    ) -> Result<Value, RuntimeError> {
        // `<handle>.wait idle, minutes <n>` (spec section 9.1): `idle` is the
        // mode word of the call, not a variable, and the compiler keeps it as
        // a bare name. It reaches the adapter as text.
        let is_wait = matches!(callee, Expr::Field { name, .. } if name == "wait");
        let call_args = if is_wait {
            let mut out = CallArgs::default();
            for arg in args {
                let value = match (&arg.name, &arg.value) {
                    (None, Expr::Name { name, .. }) if env.get(name).is_none() => {
                        Value::Text(name.clone())
                    }
                    _ => self.eval(&arg.value, env)?,
                };
                match &arg.name {
                    Some(name) => {
                        out.named.insert(name.clone(), value);
                    }
                    None => out.positional.push(value),
                }
            }
            out
        } else {
            self.eval_args(args, env)?
        };
        match callee {
            Expr::Name { name, .. } => {
                if is_builtin(name) {
                    return self.call_builtin(name, &call_args).map_err(at(span));
                }
                if env.is_capability(name) {
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!("capability `{name}` needs a verb: `{name}.<verb>`"),
                    )
                    .at(span));
                }
                self.effects.call_def(name, call_args)
            }
            Expr::Field {
                target, name: verb, ..
            } => {
                if let Expr::Name { name: root, .. } = &**target
                    && env.get(root).is_none()
                {
                    if env.is_capability(root) {
                        return self.effects.call_capability(
                            CallTarget::Capability(root.clone()),
                            verb,
                            call_args,
                        );
                    }
                    // Not a variable, not a capability: a module alias, so the
                    // call is to the qualified def `alias.name` (spec 3.9).
                    return self.effects.call_def(&format!("{root}.{verb}"), call_args);
                }
                match self.eval(target, env)? {
                    Value::Handle(handle) => {
                        self.effects
                            .call_capability(CallTarget::Handle(handle), verb, call_args)
                    }
                    other => Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!(
                            "`.{verb}(...)` needs a capability or handle, got {}",
                            other.type_name()
                        ),
                    )
                    .at(span)),
                }
            }
            _ => Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                "only a name, a def, a capability verb or a handle verb can be called",
            )
            .at(span)),
        }
    }

    /// The builtins of spec section 5.7.
    fn call_builtin(&mut self, name: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        let arg = |index: usize, named: &str| -> Result<&Value, RuntimeError> {
            args.positional(index)
                .or_else(|| args.named(named))
                .ok_or_else(|| {
                    RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!("`{name}` needs an argument `{named}`"),
                    )
                })
        };
        let optional =
            |index: usize, named: &str| args.positional(index).or_else(|| args.named(named));
        match name {
            "len" => match arg(0, "x")? {
                Value::List(items) => Ok(Value::Number(items.len() as f64)),
                Value::Text(text) => Ok(Value::Number(text.chars().count() as f64)),
                other => Err(Value::type_error("a list or text for `len`", other)),
            },
            "count" => {
                let probs = numbers(arg(0, "probs")?, "count")?;
                match (args.named("above"), args.named("below")) {
                    (Some(above), None) => {
                        let p = number(above, "`count(..., above p)`")?;
                        Ok(Value::Number(
                            probs.iter().filter(|x| **x > p).count() as f64
                        ))
                    }
                    (None, Some(below)) => {
                        let p = number(below, "`count(..., below p)`")?;
                        Ok(Value::Number(
                            probs.iter().filter(|x| **x < p).count() as f64
                        ))
                    }
                    _ => Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        "`count` needs exactly one of `above` or `below`",
                    )),
                }
            }
            "max" | "min" => {
                let xs = numbers(arg(0, "xs")?, name)?;
                let pick = if name == "max" { f64::max } else { f64::min };
                Ok(xs
                    .into_iter()
                    .reduce(pick)
                    .map_or(Value::None, Value::Number))
            }
            "sum" => Ok(Value::Number(numbers(arg(0, "xs")?, "sum")?.iter().sum())),
            "top" => {
                let items = list(arg(0, "items")?, "top")?;
                let probs = numbers(arg(1, "by")?, "top")?;
                let k = number(arg(2, "n")?, "`top(..., n k)`")?;
                if items.len() != probs.len() {
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!(
                            "`top` pairs {} items with {} probs",
                            items.len(),
                            probs.len()
                        ),
                    ));
                }
                let mut order: Vec<usize> = (0..items.len()).collect();
                order.sort_by(|a, b| {
                    probs[*b]
                        .partial_cmp(&probs[*a])
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                Ok(Value::List(
                    order
                        .into_iter()
                        .take(k.max(0.0) as usize)
                        .map(|i| items[i].clone())
                        .collect(),
                ))
            }
            "join" => {
                let texts = list(arg(0, "texts")?, "join")?;
                let sep = optional(1, "sep").map_or_else(|| "\n".to_string(), Value::to_text);
                Ok(Value::Text(
                    texts
                        .iter()
                        .map(Value::to_text)
                        .collect::<Vec<_>>()
                        .join(&sep),
                ))
            }
            "split" => {
                let text = text(arg(0, "text")?, "split")?;
                let sep = text_of(arg(1, "sep")?, "split")?;
                if sep.is_empty() {
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        "`split` needs a non-empty separator",
                    ));
                }
                Ok(Value::List(
                    text.split(sep.as_str())
                        .map(|s| Value::Text(s.to_string()))
                        .collect(),
                ))
            }
            "lines" => Ok(Value::List(
                text(arg(0, "text")?, "lines")?
                    .lines()
                    .map(|s| Value::Text(s.to_string()))
                    .collect(),
            )),
            "head" | "tail" => {
                let text = text(arg(0, "text")?, name)?;
                let n = number(arg(1, "n")?, name)?.max(0.0) as usize;
                let count = text.chars().count();
                let taken: String = if name == "head" {
                    text.chars().take(n).collect()
                } else {
                    text.chars().skip(count.saturating_sub(n)).collect()
                };
                Ok(Value::Text(taken))
            }
            "tokens" => Ok(Value::Number(
                self.estimator.estimate(&arg(0, "x")?.to_text()) as f64,
            )),
            "chunk" => {
                let text = text(arg(0, "text")?, "chunk")?;
                let n = number(arg(1, "tokens")?, "`chunk(..., tokens n)`")?;
                if n < 1.0 {
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        "`chunk` needs a token count of at least one",
                    ));
                }
                Ok(Value::List(
                    chunk(text, n as u64, self.estimator.as_ref())
                        .into_iter()
                        .map(Value::Text)
                        .collect(),
                ))
            }
            "hash" => Ok(Value::Text(short_hash(&arg(0, "x")?.to_text()))),
            "now" => self.effects.draw_now().map(Value::Text),
            "random" => self.effects.draw_random().map(Value::Number),
            "zip" => {
                let xs = list(arg(0, "xs")?, "zip")?;
                let ys = list(arg(1, "ys")?, "zip")?;
                Ok(Value::List(
                    xs.iter()
                        .zip(ys)
                        .map(|(x, y)| Value::List(vec![x.clone(), y.clone()]))
                        .collect(),
                ))
            }
            "keys" | "values" | "items" => {
                let record = record(arg(0, "r")?, name)?;
                Ok(Value::List(match name {
                    "keys" => record.keys().map(|k| Value::Text(k.clone())).collect(),
                    "values" => record.values().cloned().collect(),
                    _ => record
                        .iter()
                        .map(|(k, v)| Value::List(vec![Value::Text(k.clone()), v.clone()]))
                        .collect(),
                }))
            }
            "text" => Ok(Value::Text(arg(0, "x")?.to_text())),
            "number" => to_number(arg(0, "x")?),
            "bool" => Ok(Value::Bool(arg(0, "x")?.is_truthy())),
            _ => Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{name}` is not a builtin"),
            )),
        }
    }
}

fn at(span: Span) -> impl Fn(RuntimeError) -> RuntimeError {
    move |error| {
        if error.span == Span::default() {
            error.at(span)
        } else {
            error
        }
    }
}

/// Bind comprehension or loop variables to one item (spec section 8.2): one
/// name takes the item, several names destructure a list of that length.
///
/// # Errors
///
/// `type_error` if the item cannot be destructured into `names`.
pub fn bind_names(env: &mut Env, names: &[String], item: Value) -> Result<(), RuntimeError> {
    match names {
        [name] => {
            env.define(name.clone(), item);
            Ok(())
        }
        _ => match item {
            Value::List(parts) if parts.len() == names.len() => {
                for (name, part) in names.iter().zip(parts) {
                    env.define(name.clone(), part);
                }
                Ok(())
            }
            other => Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!(
                    "cannot unpack {} into {} names",
                    other.type_name(),
                    names.len()
                ),
            )),
        },
    }
}

/// `x.name` on any value that has fields.
fn field(target: &Value, name: &str) -> Result<Value, RuntimeError> {
    let missing = |kind: &str| {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("{kind} has no field `{name}`"),
        )
    };
    let probs = |probabilities: &BTreeMap<String, f64>| {
        Value::Record(
            probabilities
                .iter()
                .map(|(k, p)| (k.clone(), Value::Prob(*p)))
                .collect(),
        )
    };
    match target {
        Value::Record(fields) => fields.get(name).cloned().ok_or_else(|| missing("record")),
        Value::Choice(choice) => match name {
            "label" => Ok(Value::Text(choice.label.clone())),
            "confidence" => Ok(Value::Number(choice.confidence)),
            "probabilities" => Ok(probs(&choice.probabilities)),
            "index" => Ok(choice
                .index
                .map_or(Value::None, |i| Value::Number(f64::from(i)))),
            "item" => Ok(choice.item.as_deref().cloned().unwrap_or(Value::None)),
            _ => Err(missing("choice")),
        },
        Value::Level(level) => match name {
            "level" => Ok(Value::Number(f64::from(level.level))),
            "score" => Ok(Value::Number(level.score)),
            "normalized" => Ok(Value::Number(level.normalized)),
            "confidence" => Ok(Value::Number(level.confidence)),
            "probabilities" => Ok(probs(&level.probabilities)),
            _ => Err(missing("level")),
        },
        Value::Handle(handle) => match name {
            "capability" => Ok(Value::Text(handle.capability.clone())),
            "id" => Ok(Value::Text(handle.id.clone())),
            _ => handle
                .fields
                .get(name)
                .map(Value::from_json)
                .ok_or_else(|| missing("handle")),
        },
        Value::PauseResult(result) => match name {
            "answer" => Ok(Value::Text(result.answer.clone())),
            "text" => Ok(result.text.clone().map_or(Value::None, Value::Text)),
            _ => Err(missing("pause_result")),
        },
        other => Err(RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("cannot read field `{name}` of {}", other.type_name()),
        )),
    }
}

/// `x[i]`: a list by whole number, or a record by text key.
fn index_into(target: &Value, index: &Value) -> Result<Value, RuntimeError> {
    match (target, index) {
        (Value::List(items), Value::Number(n) | Value::Prob(n)) => {
            if n.fract() != 0.0 || *n < 0.0 {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("list index must be a whole non-negative number, got {n}"),
                ));
            }
            items.get(*n as usize).cloned().ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("index {n} is out of range for a list of {}", items.len()),
                )
            })
        }
        (Value::List(_), other) => Err(Value::type_error("a number to index a list", other)),
        (Value::Record(fields), Value::Text(key)) => fields.get(key).cloned().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("record has no field `{key}`"),
            )
        }),
        (Value::Record(_), other) => Err(Value::type_error("a text to index a record", other)),
        (other, _) => Err(Value::type_error("a list or record to index", other)),
    }
}

/// `c is label` (spec section 4.3).
fn is_label(target: &Value, label: &str) -> Result<Value, RuntimeError> {
    match target {
        Value::Choice(choice) => Ok(Value::Bool(choice.label == label)),
        Value::Level(level) if !level.names.is_empty() => Ok(Value::Bool(
            level
                .names
                .get(level.level as usize)
                .is_some_and(|n| n == label),
        )),
        Value::Level(_) => Err(RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("`is {label}` needs a level whose levels were named"),
        )),
        other => Err(RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!(
                "`is {label}` needs a choice or a named level, got {}",
                other.type_name()
            ),
        )),
    }
}

/// The binary operators of spec section 5.1, other than the short-circuiting
/// `and` and `or`.
fn binary(op: BinaryOp, left: &Value, right: &Value) -> Result<Value, RuntimeError> {
    let mismatch = |wants: &str| {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!(
                "`{}` needs {wants}, got {} and {}",
                op_symbol(op),
                left.type_name(),
                right.type_name()
            ),
        )
    };
    match op {
        BinaryOp::Eq => Ok(Value::Bool(left.equals(right))),
        BinaryOp::NotEq => Ok(Value::Bool(!left.equals(right))),
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
            let ordering = match (left, right) {
                (Value::Number(a) | Value::Prob(a), Value::Number(b) | Value::Prob(b)) => {
                    a.partial_cmp(b)
                }
                (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
                _ => return Err(mismatch("two numbers or two texts")),
            };
            let Some(ordering) = ordering else {
                return Ok(Value::Bool(false));
            };
            Ok(Value::Bool(match op {
                BinaryOp::Lt => ordering.is_lt(),
                BinaryOp::LtEq => ordering.is_le(),
                BinaryOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinaryOp::Add => match (left, right) {
            (Value::Number(a) | Value::Prob(a), Value::Number(b) | Value::Prob(b)) => {
                Ok(Value::Number(a + b))
            }
            (Value::Text(a), Value::Text(b)) => Ok(Value::Text(format!("{a}{b}"))),
            (Value::List(a), Value::List(b)) => {
                Ok(Value::List(a.iter().chain(b).cloned().collect()))
            }
            _ => Err(mismatch("two numbers, two texts or two lists")),
        },
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
            let (Some(a), Some(b)) = (left.as_number(), right.as_number()) else {
                return Err(mismatch("two numbers"));
            };
            Ok(Value::Number(match op {
                BinaryOp::Sub => a - b,
                BinaryOp::Mul => a * b,
                BinaryOp::Div => a / b,
                _ => a % b,
            }))
        }
        BinaryOp::And | BinaryOp::Or => unreachable!("short-circuited by the evaluator"),
    }
}

const fn op_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Or => "or",
        BinaryOp::And => "and",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
    }
}

fn number(value: &Value, what: &str) -> Result<f64, RuntimeError> {
    value
        .as_number()
        .ok_or_else(|| Value::type_error(&format!("a number for {what}"), value))
}

fn numbers(value: &Value, what: &str) -> Result<Vec<f64>, RuntimeError> {
    list(value, what)?
        .iter()
        .map(|item| number(item, &format!("`{what}`")))
        .collect()
}

fn list<'v>(value: &'v Value, what: &str) -> Result<&'v Vec<Value>, RuntimeError> {
    match value {
        Value::List(items) => Ok(items),
        other => Err(Value::type_error(&format!("a list for `{what}`"), other)),
    }
}

fn record<'v>(value: &'v Value, what: &str) -> Result<&'v BTreeMap<String, Value>, RuntimeError> {
    match value {
        Value::Record(fields) => Ok(fields),
        other => Err(Value::type_error(&format!("a record for `{what}`"), other)),
    }
}

fn text<'v>(value: &'v Value, what: &str) -> Result<&'v String, RuntimeError> {
    match value {
        Value::Text(text) => Ok(text),
        other => Err(Value::type_error(&format!("a text for `{what}`"), other)),
    }
}

fn text_of(value: &Value, what: &str) -> Result<String, RuntimeError> {
    text(value, what).cloned()
}

/// `number(x)` (spec section 5.7): numbers pass through, bools become 0 or 1,
/// and text parses the way a number literal reads (`2k`, `80%`; spec 2.6).
fn to_number(value: &Value) -> Result<Value, RuntimeError> {
    match value {
        Value::Number(n) | Value::Prob(n) => Ok(Value::Number(*n)),
        Value::Bool(b) => Ok(Value::Number(if *b { 1.0 } else { 0.0 })),
        Value::Text(text) => {
            let trimmed = text.trim();
            let parsed = if let Some(percent) = trimmed.strip_suffix('%') {
                percent.trim().parse::<f64>().map(|n| n / 100.0)
            } else if let Some(thousands) = trimmed.strip_suffix('k') {
                thousands.trim().parse::<f64>().map(|n| n * 1000.0)
            } else {
                trimmed.parse::<f64>()
            };
            parsed.map(Value::Number).map_err(|_| {
                RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("`number` cannot read {text:?} as a number"),
                )
            })
        }
        other => Err(Value::type_error(
            "a number, bool or text for `number`",
            other,
        )),
    }
}

/// `hash(x)` (spec section 5.7): a stable short hex of the text form. The first
/// sixteen hex digits of the SHA-256, which is stable across builds and
/// platforms in a way `std`'s hasher is not.
pub fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// `chunk(text, tokens n)` (spec section 5.7): pieces of at most `n` tokens,
/// split on line boundaries. A single line over `n` tokens is split by
/// characters so that the cap always holds, since `focus` relies on it.
pub fn chunk(text: &str, n: u64, estimator: &dyn TokenEstimator) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if estimator.estimate(line) > n {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(split_by_chars(line, n, estimator));
            continue;
        }
        let mut candidate = current.clone();
        candidate.push_str(line);
        if estimator.estimate(&candidate) <= n {
            current = candidate;
        } else {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The longest prefixes of `text` that each fit in `n` tokens, found by
/// binary search over character counts since any estimator grows with length.
fn split_by_chars(text: &str, n: u64, estimator: &dyn TokenEstimator) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let (mut lo, mut hi) = (1, chars.len() - start);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            let candidate: String = chars[start..start + mid].iter().collect();
            if estimator.estimate(&candidate) <= n {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        pieces.push(chars[start..start + lo].iter().collect());
        start += lo;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{DEFAULT_MODEL, Profiles};
    use crate::value::{Choice, Level};
    use jevscript_ir::{CallForm, RecordField};

    /// A test double that logs every effect and answers with canned values.
    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        randoms: Vec<f64>,
    }

    impl Effects for Log {
        fn judge(&mut self, judge: &Judge, _env: &mut Env) -> Result<Value, RuntimeError> {
            self.calls
                .push(format!("judge {}", judge.subject.state_path));
            Ok(Value::Prob(0.9))
        }
        fn call_def(&mut self, name: &str, args: CallArgs) -> Result<Value, RuntimeError> {
            self.calls
                .push(format!("def {name} {}", args.positional.len()));
            Ok(Value::Text(name.to_string()))
        }
        fn call_capability(
            &mut self,
            target: CallTarget,
            verb: &str,
            args: CallArgs,
        ) -> Result<Value, RuntimeError> {
            self.calls.push(format!(
                "cap {} {verb} {:?}",
                target.capability(),
                args.named.keys().collect::<Vec<_>>()
            ));
            Ok(Value::Handle(Handle {
                capability: target.capability().to_string(),
                id: "h1".into(),
                fields: BTreeMap::new(),
            }))
        }
        fn focus(&mut self, text: Value, purpose: String, max: f64) -> Result<Value, RuntimeError> {
            self.calls.push(format!("focus {purpose} {max}"));
            Ok(text)
        }
        fn trail(&mut self, n: usize) -> Result<Value, RuntimeError> {
            self.calls.push(format!("trail {n}"));
            Ok(Value::List(vec![]))
        }
        fn draw_now(&mut self) -> Result<String, RuntimeError> {
            self.calls.push("now".into());
            Ok("2026-09-21T00:00:00Z".into())
        }
        fn draw_random(&mut self) -> Result<f64, RuntimeError> {
            self.calls.push("random".into());
            Ok(self.randoms.pop().unwrap_or(0.5))
        }
    }

    fn profile() -> Profile {
        Profiles::bundled()
            .resolve(DEFAULT_MODEL)
            .expect("bundled")
            .clone()
    }

    fn num(n: f64) -> Expr {
        Expr::Number {
            value: n,
            span: Span::default(),
        }
    }

    fn name(n: &str) -> Expr {
        Expr::Name {
            name: n.into(),
            span: Span::default(),
        }
    }

    fn text(t: &str) -> Expr {
        Expr::Text {
            parts: vec![TextPart::Literal { value: t.into() }],
            span: Span::default(),
        }
    }

    fn list(items: Vec<Expr>) -> Expr {
        Expr::List {
            items,
            span: Span::default(),
        }
    }

    fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        Expr::Binary {
            op,
            left: Box::new(l),
            right: Box::new(r),
            span: Span::default(),
        }
    }

    fn call(callee: Expr, positional: Vec<Expr>, named: Vec<(&str, Expr)>) -> Expr {
        let mut args: Vec<Arg> = positional
            .into_iter()
            .map(|value| Arg { name: None, value })
            .collect();
        args.extend(named.into_iter().map(|(n, value)| Arg {
            name: Some(n.into()),
            value,
        }));
        Expr::Call {
            callee: Box::new(callee),
            args,
            form: CallForm::Function,
            span: Span::default(),
        }
    }

    fn builtin(n: &str, positional: Vec<Expr>, named: Vec<(&str, Expr)>) -> Expr {
        call(name(n), positional, named)
    }

    fn field(target: Expr, n: &str) -> Expr {
        Expr::Field {
            target: Box::new(target),
            name: n.into(),
            span: Span::default(),
        }
    }

    fn eval_in(expr: &Expr, env: &mut Env) -> Result<Value, RuntimeError> {
        let mut log = Log::default();
        Evaluator::new(&mut log, &profile()).eval(expr, env)
    }

    fn eval(expr: &Expr) -> Result<Value, RuntimeError> {
        eval_in(expr, &mut Env::new())
    }

    fn ok(expr: &Expr) -> Value {
        eval(expr).expect("evaluates")
    }

    fn err(expr: &Expr) -> RuntimeError {
        eval(expr).expect_err("is a type error")
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn n(x: f64) -> Value {
        Value::Number(x)
    }

    #[test]
    fn literals_and_collections_evaluate_to_themselves() {
        assert_eq!(ok(&num(2.0)), n(2.0));
        assert_eq!(ok(&text("hi")), t("hi"));
        assert_eq!(
            ok(&Expr::Bool {
                value: true,
                span: Span::default()
            }),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&Expr::None {
                span: Span::default()
            }),
            Value::None
        );
        assert_eq!(
            ok(&list(vec![num(1.0), text("a")])),
            Value::List(vec![n(1.0), t("a")])
        );
        let record = Expr::Record {
            fields: vec![RecordField {
                name: "title".into(),
                value: text("x"),
            }],
            span: Span::default(),
        };
        assert_eq!(
            ok(&record),
            Value::Record(BTreeMap::from([("title".to_string(), t("x"))]))
        );
    }

    #[test]
    fn names_resolve_through_scopes_and_inputs_and_unknown_names_are_type_errors() {
        // Spec 5.3: block scoping; an unknown name is a runtime type_error
        // since the compiler already rejects `unassigned_read`.
        let mut env = Env::new().with_inputs(BTreeMap::from([("issue".to_string(), t("#1"))]));
        env.assign("x", n(1.0));
        env.push_scope();
        env.assign("x", n(2.0)); // updates the outer x
        env.define("y", n(3.0)); // block-local
        assert_eq!(eval_in(&name("x"), &mut env).expect("x"), n(2.0));
        assert_eq!(eval_in(&name("y"), &mut env).expect("y"), n(3.0));
        assert_eq!(eval_in(&name("issue"), &mut env).expect("input"), t("#1"));
        env.pop_scope();
        assert_eq!(eval_in(&name("x"), &mut env).expect("x"), n(2.0));
        let error = eval_in(&name("y"), &mut env).expect_err("y is gone");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
    }

    #[test]
    fn a_capability_name_in_value_position_is_a_handle_to_it() {
        // Spec 14.1: `claude.spawn in tree` hands the adapter the capability.
        let mut env = Env::new().with_capabilities(["claude"]);
        let value = eval_in(&name("claude"), &mut env).expect("a handle");
        assert_eq!(
            value,
            Value::Handle(Handle {
                capability: "claude".into(),
                id: "claude".into(),
                fields: BTreeMap::new(),
            })
        );
    }

    #[test]
    fn field_assignment_only_touches_records_in_variables() {
        // Spec 5.3: `x.field = expr` only on records held in variables.
        let mut env = Env::new().with_inputs(BTreeMap::from([(
            "cfg".to_string(),
            Value::Record(BTreeMap::new()),
        )]));
        env.assign("r", Value::Record(BTreeMap::new()));
        env.assign_field("r", &["a".into()], n(1.0))
            .expect("assigns");
        assert_eq!(
            eval_in(&field(name("r"), "a"), &mut env).expect("reads"),
            n(1.0)
        );
        let error = env
            .assign_field("cfg", &["a".into()], n(1.0))
            .expect_err("inputs are read-only");
        assert!(error.message.contains("read-only"));
        env.assign("s", t("x"));
        assert_eq!(
            env.assign_field("s", &["a".into()], n(1.0))
                .expect_err("not a record")
                .code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn arithmetic_follows_5_1() {
        assert_eq!(ok(&bin(BinaryOp::Add, num(1.0), num(2.0))), n(3.0));
        assert_eq!(ok(&bin(BinaryOp::Sub, num(1.0), num(2.0))), n(-1.0));
        assert_eq!(ok(&bin(BinaryOp::Mul, num(3.0), num(2.0))), n(6.0));
        assert_eq!(ok(&bin(BinaryOp::Div, num(3.0), num(2.0))), n(1.5));
        assert_eq!(ok(&bin(BinaryOp::Rem, num(7.0), num(3.0))), n(1.0));
        // `+` concatenates two texts or two lists.
        assert_eq!(ok(&bin(BinaryOp::Add, text("a"), text("b"))), t("ab"));
        assert_eq!(
            ok(&bin(
                BinaryOp::Add,
                list(vec![num(1.0)]),
                list(vec![num(2.0)])
            )),
            Value::List(vec![n(1.0), n(2.0)])
        );
        // Mixed text and number is a type error; interpolate instead.
        let error = err(&bin(BinaryOp::Add, text("a"), num(1.0)));
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        assert!(error.message.contains("`+`"));
        assert_eq!(
            err(&bin(BinaryOp::Sub, text("a"), text("b"))).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            err(&bin(BinaryOp::Mul, list(vec![]), num(2.0))).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            err(&bin(
                BinaryOp::Div,
                num(1.0),
                Expr::None {
                    span: Span::default()
                }
            ))
            .code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            err(&bin(BinaryOp::Rem, text("a"), num(1.0))).code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn a_prob_behaves_as_a_number() {
        // Spec section 4: prob behaves as a number in arithmetic and comparison.
        let mut env = Env::new();
        env.assign("p", Value::Prob(0.75));
        assert_eq!(
            eval_in(&bin(BinaryOp::Add, name("p"), num(0.25)), &mut env).expect("adds"),
            n(1.0)
        );
        assert_eq!(
            eval_in(&bin(BinaryOp::Gt, name("p"), num(0.7)), &mut env).expect("compares"),
            Value::Bool(true)
        );
        assert_eq!(
            eval_in(&bin(BinaryOp::Eq, name("p"), num(0.75)), &mut env).expect("equals"),
            Value::Bool(true)
        );
    }

    #[test]
    fn comparisons_work_on_numbers_and_texts_only() {
        assert_eq!(
            ok(&bin(BinaryOp::Lt, num(1.0), num(2.0))),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(BinaryOp::LtEq, num(2.0), num(2.0))),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(BinaryOp::Gt, num(1.0), num(2.0))),
            Value::Bool(false)
        );
        assert_eq!(
            ok(&bin(BinaryOp::GtEq, num(2.0), num(2.0))),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(BinaryOp::Lt, text("a"), text("b"))),
            Value::Bool(true)
        );
        assert_eq!(
            err(&bin(BinaryOp::Lt, text("a"), num(1.0))).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            err(&bin(BinaryOp::GtEq, list(vec![]), list(vec![]))).code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn equality_is_by_value_across_types_without_error() {
        // Spec 4.3: `==` compares by value; different types are unequal, not
        // an error.
        assert_eq!(
            ok(&bin(BinaryOp::Eq, text("a"), text("a"))),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(BinaryOp::Eq, text("1"), num(1.0))),
            Value::Bool(false)
        );
        assert_eq!(
            ok(&bin(BinaryOp::NotEq, num(1.0), num(2.0))),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(
                BinaryOp::Eq,
                list(vec![num(1.0)]),
                list(vec![num(1.0)])
            )),
            Value::Bool(true)
        );
    }

    #[test]
    fn and_or_short_circuit_and_return_bools() {
        // Spec 5.1: the deciding operand's truthiness, as a bool.
        let boom = builtin("len", vec![num(1.0)], vec![]); // would be a type error
        assert_eq!(
            ok(&bin(BinaryOp::And, num(0.0), boom.clone())),
            Value::Bool(false)
        );
        assert_eq!(
            ok(&bin(BinaryOp::Or, text("x"), boom.clone())),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(BinaryOp::And, num(1.0), text(""))),
            Value::Bool(false)
        );
        assert_eq!(
            ok(&bin(BinaryOp::And, num(1.0), text("y"))),
            Value::Bool(true)
        );
        assert_eq!(
            ok(&bin(BinaryOp::Or, num(0.0), list(vec![]))),
            Value::Bool(false)
        );
        assert_eq!(
            ok(&bin(BinaryOp::Or, num(0.0), num(2.0))),
            Value::Bool(true)
        );
        assert!(eval(&bin(BinaryOp::And, num(1.0), boom)).is_err());
    }

    #[test]
    fn not_and_unary_minus() {
        let not = |e: Expr| Expr::Unary {
            op: UnaryOp::Not,
            operand: Box::new(e),
            span: Span::default(),
        };
        let neg = |e: Expr| Expr::Unary {
            op: UnaryOp::Neg,
            operand: Box::new(e),
            span: Span::default(),
        };
        assert_eq!(ok(&not(text(""))), Value::Bool(true));
        assert_eq!(ok(&not(num(3.0))), Value::Bool(false));
        assert_eq!(ok(&neg(num(3.0))), n(-3.0));
        assert_eq!(err(&neg(text("3"))).code, RuntimeErrorCode::TypeError);
    }

    #[test]
    fn is_compares_a_choice_label_or_a_named_level() {
        // Spec 4.3.
        let mut env = Env::new();
        env.assign(
            "c",
            Value::Choice(Choice {
                label: "stuck".into(),
                confidence: 0.9,
                probabilities: BTreeMap::new(),
                index: None,
                item: None,
            }),
        );
        env.assign(
            "l",
            Value::Level(Level {
                level: 1,
                score: 1.1,
                normalized: 0.55,
                confidence: 0.5,
                probabilities: BTreeMap::new(),
                names: vec!["unchanged".into(), "drifting".into(), "advancing".into()],
            }),
        );
        env.assign(
            "bare",
            Value::Level(Level {
                level: 0,
                score: 0.0,
                normalized: 0.0,
                confidence: 1.0,
                probabilities: BTreeMap::new(),
                names: vec![],
            }),
        );
        let is = |target: &str, label: &str| Expr::Is {
            target: Box::new(name(target)),
            label: label.into(),
            span: Span::default(),
        };
        assert_eq!(
            eval_in(&is("c", "stuck"), &mut env).expect("is"),
            Value::Bool(true)
        );
        assert_eq!(
            eval_in(&is("c", "other"), &mut env).expect("is"),
            Value::Bool(false)
        );
        assert_eq!(
            eval_in(&is("l", "drifting"), &mut env).expect("is"),
            Value::Bool(true)
        );
        assert_eq!(
            eval_in(&is("l", "unchanged"), &mut env).expect("is"),
            Value::Bool(false)
        );
        assert_eq!(
            eval_in(&is("bare", "low"), &mut env)
                .expect_err("unnamed")
                .code,
            RuntimeErrorCode::TypeError
        );
        env.assign("s", t("stuck"));
        assert_eq!(
            eval_in(&is("s", "stuck"), &mut env).expect_err("text").code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn fields_of_records_and_answers_and_missing_fields_are_type_errors() {
        let mut env = Env::new();
        env.assign(
            "c",
            Value::Choice(Choice {
                label: "i1".into(),
                confidence: 0.6,
                probabilities: BTreeMap::from([("i1".to_string(), 0.6)]),
                index: Some(1),
                item: Some(Box::new(t("second"))),
            }),
        );
        env.assign(
            "l",
            Value::Level(Level {
                level: 2,
                score: 1.8,
                normalized: 0.9,
                confidence: 0.7,
                probabilities: BTreeMap::from([("2".to_string(), 0.9)]),
                names: vec![],
            }),
        );
        env.assign(
            "r",
            Value::Record(BTreeMap::from([("a".to_string(), n(1.0))])),
        );
        let get = |target: &str, f: &str, env: &mut Env| eval_in(&field(name(target), f), env);
        assert_eq!(get("c", "label", &mut env).expect("label"), t("i1"));
        assert_eq!(get("c", "confidence", &mut env).expect("conf"), n(0.6));
        assert_eq!(get("c", "index", &mut env).expect("index"), n(1.0));
        assert_eq!(get("c", "item", &mut env).expect("item"), t("second"));
        assert_eq!(
            get("c", "probabilities", &mut env).expect("probs"),
            Value::Record(BTreeMap::from([("i1".to_string(), Value::Prob(0.6))]))
        );
        assert_eq!(get("l", "level", &mut env).expect("level"), n(2.0));
        assert_eq!(get("l", "score", &mut env).expect("score"), n(1.8));
        assert_eq!(get("l", "normalized", &mut env).expect("norm"), n(0.9));
        assert_eq!(get("r", "a", &mut env).expect("a"), n(1.0));
        // A missing record field is a type error, not `none`.
        assert_eq!(
            get("r", "b", &mut env).expect_err("missing").code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            get("c", "score", &mut env).expect_err("missing").code,
            RuntimeErrorCode::TypeError
        );
        env.assign("s", t("x"));
        assert_eq!(
            get("s", "a", &mut env).expect_err("text").code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn indexing_lists_and_records() {
        let mut env = Env::new();
        env.assign("xs", Value::List(vec![t("a"), t("b")]));
        env.assign(
            "r",
            Value::Record(BTreeMap::from([("k".to_string(), n(1.0))])),
        );
        let index = |target: &str, i: Expr| Expr::Index {
            target: Box::new(name(target)),
            index: Box::new(i),
            span: Span::default(),
        };
        assert_eq!(
            eval_in(&index("xs", num(1.0)), &mut env).expect("b"),
            t("b")
        );
        assert_eq!(
            eval_in(&index("r", text("k")), &mut env).expect("k"),
            n(1.0)
        );
        assert_eq!(
            eval_in(&index("xs", num(2.0)), &mut env)
                .expect_err("range")
                .code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            eval_in(&index("xs", num(0.5)), &mut env)
                .expect_err("fraction")
                .code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            eval_in(&index("xs", text("0")), &mut env)
                .expect_err("text")
                .code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            eval_in(&index("r", num(0.0)), &mut env)
                .expect_err("number")
                .code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn interpolation_uses_the_text_form() {
        // Spec 2.7 and 4.2.
        let mut env = Env::new();
        env.assign("n", n(2.0));
        env.assign("xs", Value::List(vec![n(1.0), Value::None]));
        env.assign("p", Value::Prob(0.5));
        let expr = Expr::Text {
            parts: vec![
                TextPart::Literal {
                    value: "n={".into(),
                },
                TextPart::Interpolation { expr: name("n") },
                TextPart::Literal {
                    value: "} xs=".into(),
                },
                TextPart::Interpolation { expr: name("xs") },
                TextPart::Literal {
                    value: " p=".into(),
                },
                TextPart::Interpolation { expr: name("p") },
                TextPart::Literal {
                    value: " none=".into(),
                },
                TextPart::Interpolation {
                    expr: Expr::None {
                        span: Span::default(),
                    },
                },
                TextPart::Literal { value: "|".into() },
            ],
            span: Span::default(),
        };
        assert_eq!(
            eval_in(&expr, &mut env).expect("interpolates"),
            t("n={2} xs=[1,null] p=0.5 none=|")
        );
    }

    #[test]
    fn comprehensions_with_a_filter_and_with_zip() {
        // Spec 8.2.
        let mut env = Env::new();
        env.assign("xs", Value::List(vec![n(1.0), n(2.0), n(3.0)]));
        env.assign(
            "ps",
            Value::List(vec![Value::Prob(0.9), Value::Prob(0.2), Value::Prob(0.8)]),
        );
        let doubled = Expr::Comprehension {
            expr: Box::new(bin(BinaryOp::Mul, name("x"), num(2.0))),
            names: vec!["x".into()],
            iterable: Box::new(name("xs")),
            test: Some(Box::new(bin(BinaryOp::Gt, name("x"), num(1.0)))),
            span: Span::default(),
        };
        assert_eq!(
            eval_in(&doubled, &mut env).expect("comprehends"),
            Value::List(vec![n(4.0), n(6.0)])
        );
        let kept = Expr::Comprehension {
            expr: Box::new(name("c")),
            names: vec!["c".into(), "p".into()],
            iterable: Box::new(builtin("zip", vec![name("xs"), name("ps")], vec![])),
            test: Some(Box::new(bin(BinaryOp::Gt, name("p"), num(0.6)))),
            span: Span::default(),
        };
        assert_eq!(
            eval_in(&kept, &mut env).expect("zips"),
            Value::List(vec![n(1.0), n(3.0)])
        );
        // The loop variable does not leak.
        assert!(env.get("x").is_none());
        assert!(env.get("c").is_none());
        let bad = Expr::Comprehension {
            expr: Box::new(name("a")),
            names: vec!["a".into(), "b".into()],
            iterable: Box::new(name("xs")),
            test: None,
            span: Span::default(),
        };
        assert_eq!(
            eval_in(&bad, &mut env).expect_err("unpack").code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn len_count_max_min_sum() {
        assert_eq!(
            ok(&builtin(
                "len",
                vec![list(vec![num(1.0), num(2.0)])],
                vec![]
            )),
            n(2.0)
        );
        assert_eq!(ok(&builtin("len", vec![text("héllo")], vec![])), n(5.0));
        assert_eq!(
            err(&builtin("len", vec![num(1.0)], vec![])).code,
            RuntimeErrorCode::TypeError
        );
        let probs = list(vec![num(0.9), num(0.6), num(0.2)]);
        assert_eq!(
            ok(&builtin(
                "count",
                vec![probs.clone()],
                vec![("above", num(0.6))]
            )),
            n(1.0)
        );
        assert_eq!(
            ok(&builtin(
                "count",
                vec![probs.clone()],
                vec![("below", num(0.6))]
            )),
            n(1.0)
        );
        assert_eq!(
            err(&builtin("count", vec![probs.clone()], vec![])).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(ok(&builtin("max", vec![probs.clone()], vec![])), n(0.9));
        assert_eq!(ok(&builtin("min", vec![probs.clone()], vec![])), n(0.2));
        assert_eq!(ok(&builtin("max", vec![list(vec![])], vec![])), Value::None);
        assert_eq!(ok(&builtin("sum", vec![probs], vec![])), n(1.7));
        assert_eq!(ok(&builtin("sum", vec![list(vec![])], vec![])), n(0.0));
        assert_eq!(
            err(&builtin("sum", vec![list(vec![text("x")])], vec![])).code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn top_orders_by_paired_prob_descending() {
        let items = list(vec![text("a"), text("b"), text("c")]);
        let probs = list(vec![num(0.1), num(0.9), num(0.5)]);
        assert_eq!(
            ok(&builtin(
                "top",
                vec![items.clone()],
                vec![("by", probs.clone()), ("n", num(2.0))]
            )),
            Value::List(vec![t("b"), t("c")])
        );
        assert_eq!(
            err(&builtin(
                "top",
                vec![items],
                vec![("by", list(vec![num(1.0)])), ("n", num(2.0))]
            ))
            .code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn join_split_lines_head_tail() {
        let texts = list(vec![text("a"), text("b")]);
        assert_eq!(ok(&builtin("join", vec![texts.clone()], vec![])), t("a\nb"));
        assert_eq!(
            ok(&builtin("join", vec![texts.clone(), text(", ")], vec![])),
            t("a, b")
        );
        assert_eq!(
            ok(&builtin("join", vec![texts], vec![("sep", text("-"))])),
            t("a-b")
        );
        assert_eq!(
            ok(&builtin("split", vec![text("a,b,,c"), text(",")], vec![])),
            Value::List(vec![t("a"), t("b"), t(""), t("c")])
        );
        assert_eq!(
            err(&builtin("split", vec![text("a"), text("")], vec![])).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            ok(&builtin("lines", vec![text("a\nb\r\nc\n")], vec![])),
            Value::List(vec![t("a"), t("b"), t("c")])
        );
        assert_eq!(
            ok(&builtin("head", vec![text("héllo"), num(2.0)], vec![])),
            t("hé")
        );
        assert_eq!(
            ok(&builtin("tail", vec![text("héllo"), num(2.0)], vec![])),
            t("lo")
        );
        assert_eq!(
            ok(&builtin("tail", vec![text("hi"), num(9.0)], vec![])),
            t("hi")
        );
        assert_eq!(
            err(&builtin("head", vec![num(1.0), num(2.0)], vec![])).code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn tokens_and_chunk_use_the_profile_estimator() {
        // Spec 5.7 and 10.6: the estimate comes from the profile's tokenizer
        // (chars4 in the bundle), never from a number in this crate.
        assert_eq!(
            ok(&builtin("tokens", vec![text("abcdefgh")], vec![])),
            n(2.0)
        );
        assert_eq!(ok(&builtin("tokens", vec![num(12345.0)], vec![])), n(2.0));
        let text_value = text("line one\nline two\nline three\n");
        let chunks = ok(&builtin(
            "chunk",
            vec![text_value],
            vec![("tokens", num(5.0))],
        ));
        assert_eq!(
            chunks,
            Value::List(vec![t("line one\nline two\n"), t("line three\n")])
        );
        // One line over the cap is split by characters so the cap holds.
        let long = ok(&builtin(
            "chunk",
            vec![text(&"x".repeat(30))],
            vec![("tokens", num(2.0))],
        ));
        let Value::List(pieces) = long else { panic!() };
        assert_eq!(pieces.len(), 4);
        for piece in &pieces {
            assert!(crate::tokens::CharsPerToken.estimate(&piece.to_text()) <= 2);
        }
        assert_eq!(
            err(&builtin(
                "chunk",
                vec![text("x")],
                vec![("tokens", num(0.0))]
            ))
            .code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn hash_is_a_stable_short_hex_of_the_text_form() {
        let a = ok(&builtin("hash", vec![text("abc")], vec![]));
        let b = ok(&builtin("hash", vec![text("abc")], vec![]));
        assert_eq!(a, b);
        let Value::Text(hex) = &a else { panic!() };
        assert_eq!(hex.len(), 16);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        // The text form: hashing the number 1 equals hashing the text "1".
        assert_eq!(
            ok(&builtin("hash", vec![num(1.0)], vec![])),
            ok(&builtin("hash", vec![text("1")], vec![]))
        );
        assert_ne!(a, ok(&builtin("hash", vec![text("abd")], vec![])));
    }

    #[test]
    fn now_and_random_are_draws_through_the_effects() {
        // Spec 5.7: recorded and replayed, so they go through `Effects`.
        let mut log = Log {
            randoms: vec![0.25],
            ..Default::default()
        };
        let mut env = Env::new();
        let mut evaluator = Evaluator::new(&mut log, &profile());
        assert_eq!(
            evaluator
                .eval(&builtin("now", vec![], vec![]), &mut env)
                .expect("now"),
            t("2026-09-21T00:00:00Z")
        );
        assert_eq!(
            evaluator
                .eval(&builtin("random", vec![], vec![]), &mut env)
                .expect("random"),
            n(0.25)
        );
        assert_eq!(log.calls, vec!["now", "random"]);
    }

    #[test]
    fn zip_keys_values_items() {
        let xs = list(vec![num(1.0), num(2.0), num(3.0)]);
        let ys = list(vec![text("a"), text("b")]);
        assert_eq!(
            ok(&builtin("zip", vec![xs, ys], vec![])),
            Value::List(vec![
                Value::List(vec![n(1.0), t("a")]),
                Value::List(vec![n(2.0), t("b")]),
            ])
        );
        let record = Expr::Record {
            fields: vec![
                RecordField {
                    name: "b".into(),
                    value: num(2.0),
                },
                RecordField {
                    name: "a".into(),
                    value: num(1.0),
                },
            ],
            span: Span::default(),
        };
        assert_eq!(
            ok(&builtin("keys", vec![record.clone()], vec![])),
            Value::List(vec![t("a"), t("b")])
        );
        assert_eq!(
            ok(&builtin("values", vec![record.clone()], vec![])),
            Value::List(vec![n(1.0), n(2.0)])
        );
        assert_eq!(
            ok(&builtin("items", vec![record], vec![])),
            Value::List(vec![
                Value::List(vec![t("a"), n(1.0)]),
                Value::List(vec![t("b"), n(2.0)]),
            ])
        );
        assert_eq!(
            err(&builtin("keys", vec![num(1.0)], vec![])).code,
            RuntimeErrorCode::TypeError
        );
    }

    #[test]
    fn conversions() {
        assert_eq!(ok(&builtin("text", vec![num(2.5)], vec![])), t("2.5"));
        assert_eq!(ok(&builtin("number", vec![text(" 42 ")], vec![])), n(42.0));
        assert_eq!(ok(&builtin("number", vec![text("80%")], vec![])), n(0.8));
        assert_eq!(ok(&builtin("number", vec![text("2k")], vec![])), n(2000.0));
        assert_eq!(
            ok(&builtin(
                "number",
                vec![Expr::Bool {
                    value: true,
                    span: Span::default()
                }],
                vec![]
            )),
            n(1.0)
        );
        assert_eq!(
            err(&builtin("number", vec![text("x")], vec![])).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            err(&builtin("number", vec![list(vec![])], vec![])).code,
            RuntimeErrorCode::TypeError
        );
        assert_eq!(
            ok(&builtin("bool", vec![text("")], vec![])),
            Value::Bool(false)
        );
        assert_eq!(
            ok(&builtin("bool", vec![num(2.0)], vec![])),
            Value::Bool(true)
        );
    }

    #[test]
    fn calls_route_to_defs_capabilities_and_handles() {
        // Spec 5.2 and 3.9: a bare name is a def, `alias.name` a qualified def,
        // `cap.verb` a capability call, and `handle.verb` a call on the handle's
        // capability.
        let mut log = Log::default();
        let mut env = Env::new().with_capabilities(["claude"]);
        env.assign(
            "dev",
            Value::Handle(Handle {
                capability: "claude".into(),
                id: "h0".into(),
                fields: BTreeMap::new(),
            }),
        );
        let mut evaluator = Evaluator::new(&mut log, &profile());
        evaluator
            .eval(&call(name("stuck"), vec![num(1.0)], vec![]), &mut env)
            .expect("def");
        evaluator
            .eval(
                &call(field(name("std"), "repeats"), vec![], vec![]),
                &mut env,
            )
            .expect("qualified def");
        let spawned = evaluator
            .eval(
                &call(
                    field(name("claude"), "spawn"),
                    vec![],
                    vec![("prompt", text("go"))],
                ),
                &mut env,
            )
            .expect("capability");
        assert!(matches!(spawned, Value::Handle(_)));
        evaluator
            .eval(
                &call(field(name("dev"), "send"), vec![text("hi")], vec![]),
                &mut env,
            )
            .expect("handle");
        let error = evaluator
            .eval(&call(name("claude"), vec![], vec![]), &mut env)
            .expect_err("a capability needs a verb");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        env.assign("s", t("x"));
        let error = evaluator
            .eval(&call(field(name("s"), "send"), vec![], vec![]), &mut env)
            .expect_err("text has no verbs");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        assert_eq!(
            log.calls,
            vec![
                "def stuck 1",
                "def std.repeats 0",
                "cap claude spawn [\"prompt\"]",
                "cap claude send []",
            ]
        );
    }

    #[test]
    fn a_property_on_a_capability_or_a_handle_is_a_zero_argument_verb() {
        // Spec 5.2: `tree.tests_pass`, `tree.diff.files` and `dev.observe` are
        // calls even though they read as properties; a field a handle does
        // carry is still a field.
        let mut log = Log::default();
        let mut env = Env::new().with_capabilities(["tree", "claude"]);
        env.assign(
            "dev",
            Value::Handle(Handle {
                capability: "claude".into(),
                id: "h0".into(),
                fields: BTreeMap::from([("pane".to_string(), serde_json::json!(3))]),
            }),
        );
        let mut evaluator = Evaluator::new(&mut log, &profile());
        assert!(matches!(
            evaluator
                .eval(&field(name("tree"), "tests_pass"), &mut env)
                .expect("calls"),
            Value::Handle(_)
        ));
        assert_eq!(
            evaluator
                .eval(&field(field(name("dev"), "observe"), "id"), &mut env)
                .expect("calls then reads"),
            t("h1")
        );
        assert_eq!(
            evaluator
                .eval(&field(name("dev"), "pane"), &mut env)
                .expect("a field"),
            n(3.0)
        );
        assert_eq!(
            log.calls,
            vec!["cap tree tests_pass []", "cap claude observe []"]
        );
    }

    #[test]
    fn the_wait_mode_word_reaches_the_adapter_as_text() {
        // Spec 9.1: `<handle>.wait idle, minutes <n>`; `idle` is not a
        // variable. A bound name is still a variable.
        let mut log = Log::default();
        let mut env = Env::new().with_capabilities(["claude"]);
        env.assign(
            "dev",
            Value::Handle(Handle {
                capability: "claude".into(),
                id: "h0".into(),
                fields: BTreeMap::new(),
            }),
        );
        let mut evaluator = Evaluator::new(&mut log, &profile());
        evaluator
            .eval(
                &call(
                    field(name("dev"), "wait"),
                    vec![name("idle")],
                    vec![("minutes", num(5.0))],
                ),
                &mut env,
            )
            .expect("waits");
        let error = evaluator
            .eval(
                &call(field(name("dev"), "send"), vec![name("idle")], vec![]),
                &mut env,
            )
            .expect_err("only `wait` has a mode word");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        assert_eq!(log.calls, vec!["cap claude wait [\"minutes\"]"]);
    }

    #[test]
    fn focus_trail_and_judge_delegate() {
        let mut log = Log::default();
        let mut env = Env::new();
        env.assign("transcript", t("long"));
        let mut evaluator = Evaluator::new(&mut log, &profile());
        let focus = Expr::Focus {
            text: Box::new(name("transcript")),
            on: Box::new(text("errors")),
            max: 2000.0,
            span: Span::default(),
        };
        assert_eq!(evaluator.eval(&focus, &mut env).expect("focus"), t("long"));
        let trail = Expr::Trail {
            count: 3.0,
            span: Span::default(),
        };
        assert_eq!(
            evaluator.eval(&trail, &mut env).expect("trail"),
            Value::List(vec![])
        );
        let judge = Expr::Judge(Box::new(Judge {
            each: false,
            subject: jevscript_ir::Subject {
                root: "transcript".into(),
                path: vec![],
                state_path: "transcript".into(),
                span: Span::default(),
            },
            verb: jevscript_ir::JudgeVerb::Feels {
                condition: text("repeats"),
            },
            detail: None,
            request_group: 0,
            span: Span::default(),
        }));
        assert_eq!(
            evaluator.eval(&judge, &mut env).expect("judge"),
            Value::Prob(0.9)
        );
        assert_eq!(
            log.calls,
            vec!["focus errors 2000", "trail 3", "judge transcript"]
        );
    }

    #[test]
    fn pure_effects_refuse_everything() {
        let mut pure = PureEffects;
        let mut env = Env::new();
        let mut evaluator = Evaluator::new(&mut pure, &profile());
        let error = evaluator
            .eval(&builtin("random", vec![], vec![]), &mut env)
            .expect_err("refused");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        assert!(error.message.contains("inside a question"));
        assert!(
            evaluator
                .eval(&call(name("stuck"), vec![], vec![]), &mut env)
                .is_err()
        );
    }

    #[test]
    fn errors_carry_the_span_of_the_expression() {
        let span = Span::new(
            jevscript_syntax::Pos::new(3, 1, 20),
            jevscript_syntax::Pos::new(3, 9, 28),
        );
        let expr = Expr::Binary {
            op: BinaryOp::Add,
            left: Box::new(text("a")),
            right: Box::new(num(1.0)),
            span,
        };
        assert_eq!(err(&expr).span, span);
    }
}
