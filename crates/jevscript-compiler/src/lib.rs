//! The Jevscript compiler: AST in, IR out.
//!
//! The stages are [`jevscript_syntax::lex`], [`jevscript_syntax::parse`] and
//! [`compile`]. This crate owns the passes that need the whole program in view:
//! module linking (spec section 3.9), the scope and side-effect checks of
//! section 12, lowering to the IR of section 11.1, batching (6.6), judgment
//! shape hashes (11.4), and budget and threshold defaults (7.1, 7.6).
//!
//! A compile runs [`link::link`], then [`check::check_module`] and
//! [`lower::lower_module`] on every module, root first and the prelude last,
//! and assembles one flat [`Ir`] with every unit qualified by its module alias.
//!
//! Diagnostics are [`jevscript_syntax::Diagnostic`]: there is one diagnostic
//! type in the workspace and one closed list of codes, and both live next to the
//! lexer that first needs them. Every diagnostic names its file once the
//! compiler knows it: a library's own for a library, the root's path for the
//! root when it was compiled from one (spec section 12).

#![forbid(unsafe_code)]

pub mod batch;
pub mod check;
pub mod link;
pub mod lower;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jevscript_ir::{CapabilityKind, Expr, Ir, Module, NeedMapping, ShapeField, Stmt, TextPart};
use jevscript_syntax::ast::Decl;
use jevscript_syntax::{Diagnostic, Program, parse};

pub use jevscript_syntax::{ErrorCode, Severity, Span};
pub use link::Resolver;

/// What a compile produced: the IR when there were no errors, and every
/// diagnostic either way. `jevscript check` reports the diagnostics of a
/// program that compiled, which is why the two travel together.
#[derive(Debug, Clone, PartialEq)]
pub struct Compilation {
    /// The linked IR, present exactly when no diagnostic is an error.
    pub ir: Option<Ir>,
    /// Every error and warning, module by module, in source order.
    pub diagnostics: Vec<Diagnostic>,
}

impl Compilation {
    /// Whether the program compiled.
    pub fn is_ok(&self) -> bool {
        self.ir.is_some()
    }

    fn into_result(self) -> Result<Ir, Vec<Diagnostic>> {
        match self.ir {
            Some(ir) => Ok(ir),
            None => Err(self.diagnostics),
        }
    }
}

/// Lower a parsed program to IR, linking its `use` declarations against the
/// current directory and no search roots.
///
/// # Errors
///
/// Every diagnostic found, warnings included, when any is an error.
pub fn compile(program: &Program) -> Result<Ir, Vec<Diagnostic>> {
    analyze(program, None, &Resolver::default()).into_result()
}

/// Lex, parse and compile `source` in one step: a single file, whose `use`
/// paths — if it has any — resolve against the current directory. This is
/// what `program.load { source }` calls.
///
/// # Errors
///
/// The diagnostics from whichever stage failed first.
pub fn compile_source(source: &str) -> Result<Ir, Vec<Diagnostic>> {
    let program = parse(source)?;
    compile(&program)
}

/// Read, parse and compile the file at `path`, resolving relative `use` paths
/// against its directory and the rest against `resolver`. This is what
/// `jevscript compile` and `program.load { path }` call.
///
/// # Errors
///
/// [`ErrorCode::UseNotFound`] when the file cannot be read, otherwise the
/// diagnostics from whichever stage failed first.
pub fn compile_file(path: &Path, resolver: &Resolver) -> Result<Ir, Vec<Diagnostic>> {
    analyze_file(path, resolver).into_result()
}

/// [`compile_file`], keeping the warnings of a program that compiled.
pub fn analyze_file(path: &Path, resolver: &Resolver) -> Compilation {
    let file = path.display().to_string();
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return Compilation {
                ir: None,
                diagnostics: vec![
                    Diagnostic::error(
                        ErrorCode::UseNotFound,
                        format!("cannot read `{file}`: {error}"),
                        Span::default(),
                    )
                    .in_file(file),
                ],
            };
        }
    };
    let program = match parse(&source) {
        Ok(program) => program,
        Err(diagnostics) => {
            return Compilation {
                ir: None,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| d.in_file(file.clone()))
                    .collect(),
            };
        }
    };
    analyze(&program, Some(path), resolver)
}

/// Link, check and lower `program`, keeping every diagnostic.
///
/// `file` is where the program was read from, for relative `use` paths and
/// for [`Ir::file`].
pub fn analyze(program: &Program, file: Option<&Path>, resolver: &Resolver) -> Compilation {
    let root_file = file.map(|p| p.display().to_string());
    let attribute = |diagnostics: Vec<Diagnostic>| -> Vec<Diagnostic> {
        match &root_file {
            Some(root) => diagnostics
                .into_iter()
                .map(|d| d.in_file(root.clone()))
                .collect(),
            None => diagnostics,
        }
    };
    let linked = match link::link(program, file, resolver) {
        Ok(linked) => linked,
        Err(diagnostics) => {
            return Compilation {
                ir: None,
                diagnostics: attribute(diagnostics),
            };
        }
    };

    let mut diagnostics = Vec::new();
    let mut ir = Ir::empty(program.name.name.clone());
    ir.file = file.map(|p| p.display().to_string());
    ir.span = program.span;
    for decl in &program.decls {
        match decl {
            Decl::In { name, shape, span } => {
                ir.inputs.push(lower::lower_input(name, shape, *span));
            }
            Decl::Out { name, span } => ir.outputs.push(lower::lower_output(name, *span)),
            Decl::Needs {
                name,
                kind,
                signatures,
                span,
            } => ir
                .needs
                .push(lower::lower_need(name, *kind, signatures, *span)),
        }
    }

    for module in &linked.modules {
        diagnostics.extend(check::check_module(module, &linked));
        let (lowered, lowering_diagnostics) = lower::lower_module(module, &linked);
        diagnostics.extend(lowering_diagnostics);
        ir.judgments.extend(lowered.judgments);
        ir.tasks.extend(lowered.tasks);
        ir.defs.extend(lowered.defs);
        ir.machines.extend(lowered.machines);
        ir.modules.push(Module {
            alias: module.alias.clone(),
            program: module.program.name.name.clone(),
            path: module.use_path.clone(),
            mapping: module
                .mapping
                .iter()
                .map(|(inner, outer, kind)| NeedMapping {
                    inner: inner.clone(),
                    outer: outer.clone(),
                    kind: lower::lower_kind(*kind),
                })
                .collect(),
            span: module.span,
        });
    }

    let diagnostics = attribute(diagnostics);
    let failed = diagnostics.iter().any(Diagnostic::is_error);
    Compilation {
        ir: (!failed).then_some(ir),
        diagnostics,
    }
}

/// Every verb the linked IR calls on each `tool` capability, by the
/// capability's name (spec section 9.4).
///
/// This is the program's side of the manifest comparison: `task.start`
/// refuses to start when a verb here is missing from the adapter's manifest,
/// and `jevscript check --tools` makes the same comparison without a run. A
/// verb written as a property (`tree.tests_pass`, `tree.diff.files`) counts,
/// since it is a zero-argument call (spec section 5.2).
pub fn tool_verbs_referenced(ir: &Ir) -> BTreeMap<String, BTreeSet<String>> {
    let tools: BTreeSet<&str> = ir
        .needs
        .iter()
        .filter(|need| need.kind == CapabilityKind::Tool)
        .map(|need| need.name.as_str())
        .collect();
    let mut verbs: BTreeMap<String, BTreeSet<String>> = tools
        .iter()
        .map(|name| ((*name).to_string(), BTreeSet::new()))
        .collect();
    let mut visit = |expr: &Expr| {
        if let Expr::Field { target, name, .. } = expr
            && let Expr::Name { name: root, .. } = &**target
            && tools.contains(root.as_str())
        {
            verbs.entry(root.clone()).or_default().insert(name.clone());
        }
    };
    for judgment in &ir.judgments {
        for result in &judgment.results {
            walk_expr(&Expr::Judge(Box::new(result.question.clone())), &mut visit);
        }
        for log in &judgment.logs {
            walk_expr(&log.log, &mut visit);
        }
    }
    for task in &ir.tasks {
        walk_params(&task.params, &mut visit);
        walk_expr_in_stmts(&task.body, &mut visit);
    }
    for def in &ir.defs {
        walk_params(&def.params, &mut visit);
        walk_expr_in_stmts(&def.body, &mut visit);
    }
    for machine in &ir.machines {
        walk_params(&machine.params, &mut visit);
        if let Some(goal) = &machine.goal {
            walk_expr(goal, &mut visit);
        }
        walk_fields(&machine.observe, &mut visit);
        for state in &machine.states {
            for transition in &state.transitions {
                walk_expr(&transition.description, &mut visit);
                if let Some(when) = &transition.when {
                    walk_expr(when, &mut visit);
                }
                walk_expr_in_stmts(&transition.body, &mut visit);
            }
        }
    }
    verbs
}

fn walk_params(params: &[jevscript_ir::DefParam], visit: &mut impl FnMut(&Expr)) {
    for param in params {
        if let Some(default) = &param.default {
            walk_expr(default, visit);
        }
    }
}

fn walk_fields(fields: &[ShapeField], visit: &mut impl FnMut(&Expr)) {
    for field in fields {
        walk_expr(&field.value, visit);
        if let jevscript_ir::ShapePolicy::Focus { on } = &field.policy {
            walk_expr(on, visit);
        }
    }
}

/// Every expression node under a statement list, parents before children.
pub fn walk_expr_in_stmts(stmts: &[Stmt], visit: &mut impl FnMut(&Expr)) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { value, .. } => walk_expr(value, visit),
            Stmt::Expr { expr, .. } => walk_expr(expr, visit),
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in branches {
                    walk_expr(&branch.test, visit);
                    walk_expr_in_stmts(&branch.body, visit);
                }
                if let Some(body) = otherwise {
                    walk_expr_in_stmts(body, visit);
                }
            }
            Stmt::For { iterable, body, .. } => {
                walk_expr(iterable, visit);
                walk_expr_in_stmts(body, visit);
            }
            Stmt::Loop { body, .. } => walk_expr_in_stmts(body, visit),
            Stmt::Until { test, body, .. } => {
                walk_expr(test, visit);
                walk_expr_in_stmts(body, visit);
            }
            Stmt::Gate {
                risk,
                confidence,
                done,
                arms,
                ..
            } => {
                for expr in [risk, confidence, done].into_iter().flatten() {
                    walk_expr(expr, visit);
                }
                for arm in arms {
                    walk_expr_in_stmts(&arm.body, visit);
                }
            }
            Stmt::Shape { fields, .. } => walk_fields(fields, visit),
            Stmt::Return { value, .. } => {
                if let Some(value) = value {
                    walk_expr(value, visit);
                }
            }
            Stmt::Stop { reason, .. } | Stmt::Escalate { reason, .. } => walk_expr(reason, visit),
            Stmt::Continue { .. } | Stmt::Break { .. } => {}
        }
    }
}

/// Every expression node under `expr`, parents before children.
pub fn walk_expr(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    match expr {
        Expr::Number { .. }
        | Expr::Bool { .. }
        | Expr::None { .. }
        | Expr::Name { .. }
        | Expr::Trail { .. } => {}
        Expr::Text { parts, .. } => {
            for part in parts {
                if let TextPart::Interpolation { expr } = part {
                    walk_expr(expr, visit);
                }
            }
        }
        Expr::List { items, .. } => {
            for item in items {
                walk_expr(item, visit);
            }
        }
        Expr::Record { fields, .. } => {
            for field in fields {
                walk_expr(&field.value, visit);
            }
        }
        Expr::Field { target, .. } => walk_expr(target, visit),
        Expr::Index { target, index, .. } => {
            walk_expr(target, visit);
            walk_expr(index, visit);
        }
        Expr::Call { callee, args, .. } => {
            walk_expr(callee, visit);
            for arg in args {
                walk_expr(&arg.value, visit);
            }
        }
        Expr::Unary { operand, .. } => walk_expr(operand, visit),
        Expr::Binary { left, right, .. } => {
            walk_expr(left, visit);
            walk_expr(right, visit);
        }
        Expr::Is { target, .. } => walk_expr(target, visit),
        Expr::Comprehension {
            expr,
            iterable,
            test,
            ..
        } => {
            walk_expr(expr, visit);
            walk_expr(iterable, visit);
            if let Some(test) = test {
                walk_expr(test, visit);
            }
        }
        Expr::Focus { text, on, .. } => {
            walk_expr(text, visit);
            walk_expr(on, visit);
        }
        Expr::Log { value, fields, .. } => {
            walk_expr(value, visit);
            for field in fields {
                walk_expr(&field.value, visit);
            }
        }
        Expr::Judge(judge) => {
            for step in &judge.subject.path {
                if let jevscript_ir::SubjectStep::Index { index } = step {
                    walk_expr(index, visit);
                }
            }
            match &judge.verb {
                jevscript_ir::JudgeVerb::Feels { condition } => walk_expr(condition, visit),
                jevscript_ir::JudgeVerb::Pick { labels } => {
                    for label in labels {
                        for expr in [
                            &label.description,
                            &label.what,
                            &label.not_for,
                            &label.examples,
                        ]
                        .into_iter()
                        .flatten()
                        {
                            walk_expr(expr, visit);
                        }
                    }
                }
                jevscript_ir::JudgeVerb::Rate { levels } => {
                    for level in levels {
                        walk_expr(&level.situation, visit);
                    }
                }
                jevscript_ir::JudgeVerb::PickAmong { question, .. } => {
                    walk_expr(question, visit);
                }
            }
            if let Some(detail) = &judge.detail {
                for expr in [
                    &detail.focus,
                    &detail.note,
                    &detail.compare,
                    &detail.yes,
                    &detail.no,
                ]
                .into_iter()
                .flatten()
                {
                    walk_expr(expr, visit);
                }
            }
        }
    }
}
