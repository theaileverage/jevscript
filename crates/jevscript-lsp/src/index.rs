//! What every name in one file means.
//!
//! One walk over the parsed program records every declaration as a
//! [`Symbol`] and every name as an [`Occurrence`] with the role it plays and
//! what it refers to. Highlighting, hover, navigation, the outline and
//! completion all read this one table, so they cannot disagree about what a
//! name is.
//!
//! Resolution follows the compiler's order (spec sections 3.9, 5.3 and 7.8):
//! a unit's parameters and assigned variables, then the program's inputs and
//! outputs, its capabilities, its own units, its module aliases, the prelude
//! `std`, and the builtins of section 5.7. References that leave the file —
//! `harness.watch`, a result of an imported judgment — are recorded as
//! written, and [`crate::world`] resolves them against the imported file.

use std::collections::BTreeMap;

use jevscript_compiler::check::{AGENT_VERBS, BUILTINS, LLM_VERBS, PERSON_VERBS};
use jevscript_compiler::link::{PRELUDE_ALIAS, PRELUDE_SOURCE, UnitKind};
use jevscript_syntax::ast::{
    Arg, Block, CapabilityKind, Decl, Detail, Expr, FieldDecl, Ident, JudgeExpr, JudgeVerb,
    MachineUnit, Param, Program, Shape, ShapeField, Stmt, SubjectStep, Unit,
};
use jevscript_syntax::token::TextPart;
use jevscript_syntax::{Span, parse_expression};

/// An index into [`Index::symbols`].
pub type SymbolId = usize;

/// What a declaration declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// `program <name>` (3.1).
    Program,
    /// A `use ... as <alias>` (3.9).
    Module,
    /// `in <name>` (3.2).
    Input,
    /// A field of a record-shaped input.
    InputField,
    /// `out <name>` (3.3).
    Output,
    /// `needs <name>: <kind>` (3.4).
    Capability(CapabilityKind),
    /// A verb in a `tool` signature block (9.4).
    ToolVerb,
    /// A `judgment`, `task`, `def` or `machine`.
    Unit(UnitKind),
    /// A unit or tool-verb parameter.
    Parameter,
    /// A result line of a `judgment` block (6.7).
    Result,
    /// A variable assigned in a unit body, or a loop variable.
    Local,
    /// A field of a `shape` or of a machine's `observe` (7.2, 7.8).
    ShapeField,
    /// A machine state (7.8).
    State,
    /// A machine event (7.8).
    Event,
    /// A `pick` label or a named `rate` level (6.3, 6.4).
    Label,
}

/// One declaration.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The declared name.
    pub name: String,
    /// What it is.
    pub kind: SymbolKind,
    /// The name as written.
    pub selection: Span,
    /// The whole declaration.
    pub full: Span,
    /// The declaration it sits inside, for the outline.
    pub parent: Option<SymbolId>,
    /// A one-line signature: the header as written.
    pub detail: String,
}

/// What an occurrence refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A declaration in this file.
    Symbol(SymbolId),
    /// A unit, by the dotted path written: `["read_agent"]` or
    /// `["harness", "watch"]`. A one-element path that is not a unit of this
    /// file is a prelude def.
    Unit(Vec<String>),
    /// A module alias reached through another alias: `a.b` in `a.b.unit`.
    Module(Vec<String>),
    /// A named result of a judgment called through `unit`.
    Member {
        /// The judgment's path.
        unit: Vec<String>,
        /// The result's name.
        member: String,
    },
    /// A label of a result of a judgment called through `unit`.
    MemberLabel {
        /// The judgment's path.
        unit: Vec<String>,
        /// The result's name.
        member: String,
        /// The label.
        label: String,
    },
    /// A parameter of a unit, named in a call to it.
    UnitParam {
        /// The unit's path.
        unit: Vec<String>,
        /// The parameter's name.
        param: String,
    },
    /// A capability of the library imported as `alias`, in a `with` clause.
    LibraryCapability {
        /// The alias of the `use`.
        alias: String,
        /// The library's own name for the capability.
        name: String,
    },
    /// The file a `use` path names.
    UsePath(String),
    /// A verb of a declared capability of a fixed-verb kind, or an open tool.
    Verb {
        /// The capability's kind.
        kind: CapabilityKind,
        /// The verb.
        verb: String,
    },
    /// An `agent` verb called on a handle (9.1).
    HandleVerb(String),
    /// A builtin function (5.7).
    Builtin(String),
    /// A label that could not be tied to one judgment; any label of that
    /// name in the file.
    Label(String),
    /// Nothing the server can say more about: an unknown name, a record field.
    None,
}

/// How a name is used, which is what highlighting shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A unit, by kind.
    Unit(UnitKind),
    /// A prelude def (8.1).
    Prelude,
    /// A builtin function (5.7).
    Builtin,
    /// A capability name.
    Capability,
    /// A module alias.
    Module,
    /// A parameter.
    Parameter,
    /// A variable.
    Variable,
    /// An input, which is immutable (3.2).
    Input,
    /// An output (3.3).
    Output,
    /// A field of a record.
    Property,
    /// A capability verb.
    Verb,
    /// A label or named level.
    Label,
    /// A machine state.
    State,
    /// A machine event.
    Event,
    /// A type name in a shape or signature.
    Type,
}

/// One name in the source.
#[derive(Debug, Clone)]
pub struct Occurrence {
    /// Where the name is written.
    pub span: Span,
    /// How it is used.
    pub role: Role,
    /// What it refers to.
    pub target: Target,
    /// Whether this is the declaration itself.
    pub declaration: bool,
}

/// What a variable holds, as far as the source says, for member completion
/// and for resolving `j.next` to the judgment result it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The record a unit call returns.
    Unit(Vec<String>),
    /// A `shape` record, or a machine's `obs`, with these field symbols.
    Shape(Vec<SymbolId>),
    /// A record-shaped input with these field symbols.
    Record(Vec<SymbolId>),
    /// An agent handle from `spawn` (9.1).
    Handle,
    /// An inline `pick` or `rate`, with its label symbols.
    Labels(Vec<SymbolId>),
}

/// A name bound inside a unit.
#[derive(Debug, Clone)]
pub struct Binding {
    /// Its declaration, when it has one (`obs` does not).
    pub symbol: Option<SymbolId>,
    /// What it holds, when known.
    pub source: Option<Source>,
}

/// One unit's names, for completion at a cursor inside it.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The unit.
    pub unit: SymbolId,
    /// The unit's whole span.
    pub span: Span,
    /// Every name bound anywhere in it.
    pub names: BTreeMap<String, Binding>,
}

/// A `use` declaration.
#[derive(Debug, Clone)]
pub struct UseInfo {
    /// The alias symbol.
    pub symbol: SymbolId,
    /// The alias.
    pub alias: String,
    /// The path as written, when it is plain text.
    pub path: Option<String>,
    /// Where the path literal is written.
    pub path_span: Span,
    /// The whole declaration.
    pub span: Span,
}

/// Everything known about the names of one file.
#[derive(Debug, Clone, Default)]
pub struct Index {
    /// Every declaration, in the order the walk met them.
    pub symbols: Vec<Symbol>,
    /// Every name, sorted by position.
    pub occurrences: Vec<Occurrence>,
    /// Each unit's bindings.
    pub scopes: Vec<Scope>,
    /// The `use` declarations, in order.
    pub uses: Vec<UseInfo>,
    /// The program's own units, by name.
    pub units: BTreeMap<String, SymbolId>,
    /// The program's capabilities, by name.
    pub capabilities: BTreeMap<String, SymbolId>,
    /// The program's inputs and outputs, by name.
    pub globals: BTreeMap<String, Binding>,
    /// Each `log` expression's contextual `log` word and its level word
    /// (spec section 5.8), which the AST keeps no spans for.
    pub logs: Vec<(Span, Span)>,
}

impl Index {
    /// Index `program`, whose source is `source`.
    pub fn build(program: &Program, source: &str) -> Self {
        let mut builder = Builder {
            source,
            index: Index::default(),
            scope: BTreeMap::new(),
            unit: None,
            prelude: prelude_names(),
        };
        builder.program(program);
        let mut index = builder.index;
        index
            .occurrences
            .sort_by_key(|o| (o.span.start.offset, o.span.end.offset));
        index
    }

    /// The occurrence under byte `offset`, if any. The end of a name counts,
    /// so a cursor just after a word still finds it.
    pub fn occurrence_at(&self, offset: usize) -> Option<&Occurrence> {
        self.occurrences
            .iter()
            .filter(|o| {
                o.span.start.offset as usize <= offset && offset <= o.span.end.offset as usize
            })
            .min_by_key(|o| o.span.end.offset - o.span.start.offset)
    }

    /// The innermost unit scope holding byte `offset`.
    pub fn scope_at(&self, offset: usize) -> Option<&Scope> {
        self.scopes.iter().find(|scope| {
            scope.span.start.offset as usize <= offset && offset <= scope.span.end.offset as usize
        })
    }

    /// The symbol declared at `span`, if one is.
    pub fn symbol_at(&self, span: Span) -> Option<SymbolId> {
        self.symbols.iter().position(|s| s.selection == span)
    }

    /// The children of `parent`, in source order.
    pub fn children(&self, parent: Option<SymbolId>) -> Vec<SymbolId> {
        let mut children: Vec<SymbolId> = (0..self.symbols.len())
            .filter(|id| self.symbols[*id].parent == parent)
            .collect();
        children.sort_by_key(|id| self.symbols[*id].full.start.offset);
        children
    }

    /// A unit of this file by name, with its kind.
    pub fn unit(&self, name: &str) -> Option<(SymbolId, UnitKind)> {
        let id = *self.units.get(name)?;
        match self.symbols[id].kind {
            SymbolKind::Unit(kind) => Some((id, kind)),
            _ => None,
        }
    }

    /// The named child of a symbol.
    pub fn child(&self, parent: SymbolId, name: &str) -> Option<SymbolId> {
        (0..self.symbols.len())
            .find(|id| self.symbols[*id].parent == Some(parent) && self.symbols[*id].name == name)
    }

    /// The labels under `parent` (a result, a variable or a label's result).
    pub fn labels_of(&self, parent: SymbolId) -> Vec<SymbolId> {
        (0..self.symbols.len())
            .filter(|id| {
                self.symbols[*id].parent == Some(parent)
                    && self.symbols[*id].kind == SymbolKind::Label
            })
            .collect()
    }

    /// Every label declared in the file.
    pub fn all_labels(&self) -> Vec<SymbolId> {
        (0..self.symbols.len())
            .filter(|id| self.symbols[*id].kind == SymbolKind::Label)
            .collect()
    }
}

/// The prelude's exported defs, which are in scope unqualified (3.9).
pub fn prelude_names() -> Vec<String> {
    let Ok(program) = jevscript_syntax::parse(PRELUDE_SOURCE) else {
        return Vec::new();
    };
    program
        .units
        .iter()
        .filter_map(|unit| match unit {
            Unit::Def(def) if !def.name.name.starts_with('_') => Some(def.name.name.clone()),
            _ => None,
        })
        .collect()
}

/// The verbs a fixed-verb capability kind accepts (spec section 9).
pub fn kind_verbs(kind: CapabilityKind) -> &'static [&'static str] {
    match kind {
        CapabilityKind::Agent => &AGENT_VERBS,
        CapabilityKind::Person => &PERSON_VERBS,
        CapabilityKind::Llm => &LLM_VERBS,
        CapabilityKind::Tool => &[],
    }
}

/// Whether `name` is a builtin function (5.7).
pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// A dotted name path, `a.b.c`, if `expr` is one.
fn dotted(expr: &Expr) -> Option<Vec<&Ident>> {
    match expr {
        Expr::Name(ident) => Some(vec![ident]),
        Expr::Field { target, name, .. } => {
            let mut path = dotted(target)?;
            path.push(name);
            Some(path)
        }
        _ => None,
    }
}

struct Builder<'a> {
    source: &'a str,
    index: Index,
    /// The current unit's bindings.
    scope: BTreeMap<String, Binding>,
    /// The unit being walked.
    unit: Option<SymbolId>,
    prelude: Vec<String>,
}

impl Builder<'_> {
    fn symbol(
        &mut self,
        name: &Ident,
        kind: SymbolKind,
        full: Span,
        parent: Option<SymbolId>,
        detail: String,
    ) -> SymbolId {
        let id = self.index.symbols.len();
        self.index.symbols.push(Symbol {
            name: name.name.clone(),
            kind,
            selection: name.span,
            full,
            parent,
            detail,
        });
        id
    }

    fn occur(&mut self, span: Span, role: Role, target: Target, declaration: bool) {
        self.index.occurrences.push(Occurrence {
            span,
            role,
            target,
            declaration,
        });
    }

    fn declare(
        &mut self,
        name: &Ident,
        kind: SymbolKind,
        role: Role,
        full: Span,
        parent: Option<SymbolId>,
        detail: String,
    ) -> SymbolId {
        let id = self.symbol(name, kind, full, parent, detail);
        self.occur(name.span, role, Target::Symbol(id), true);
        id
    }

    /// The first line of a declaration as written, without its trailing `:`.
    fn header(&self, span: Span) -> String {
        let start = span.start.offset as usize;
        let end = (span.end.offset as usize).min(self.source.len());
        let text = self.source.get(start..end).unwrap_or_default();
        let line = text.lines().next().unwrap_or_default().trim_end();
        line.strip_suffix(':')
            .unwrap_or(line)
            .trim_end()
            .to_string()
    }

    /* ---------------------------------------------------------------- */
    /* declarations                                                      */
    /* ---------------------------------------------------------------- */

    fn program(&mut self, program: &Program) {
        let root = self.symbol(
            &program.name,
            SymbolKind::Program,
            program.span,
            None,
            format!("program {}", program.name.name),
        );
        self.occur(program.name.span, Role::Module, Target::Symbol(root), true);

        // Declarations and unit names first, so every reference resolves
        // regardless of the order things are written in.
        for decl in &program.decls {
            self.decl(decl);
        }
        for unit in &program.units {
            let (name, kind, span) = match unit {
                Unit::Judgment(u) => (&u.name, UnitKind::Judgment, u.span),
                Unit::Task(u) => (&u.name, UnitKind::Task, u.span),
                Unit::Def(u) => (&u.name, UnitKind::Def, u.span),
                Unit::Machine(u) => (&u.name, UnitKind::Machine, u.span),
            };
            let detail = self.header(span);
            let id = self.declare(
                name,
                SymbolKind::Unit(kind),
                Role::Unit(kind),
                span,
                None,
                detail,
            );
            self.index.units.entry(name.name.clone()).or_insert(id);
        }
        for use_decl in &program.uses {
            let detail = self.header(use_decl.span);
            let id = self.declare(
                &use_decl.alias,
                SymbolKind::Module,
                Role::Module,
                use_decl.span,
                None,
                detail,
            );
            let path = use_decl.path.as_plain().map(str::to_string);
            let path_span = self.use_path_span(use_decl.span);
            if let Some(path) = &path {
                self.occur(
                    path_span,
                    Role::Module,
                    Target::UsePath(path.clone()),
                    false,
                );
            }
            for mapping in &use_decl.mapping {
                self.occur(
                    mapping.inner.span,
                    Role::Capability,
                    Target::LibraryCapability {
                        alias: use_decl.alias.name.clone(),
                        name: mapping.inner.name.clone(),
                    },
                    false,
                );
                // `with dev: claude`: `claude` is this program's capability.
                if let Some(outer) = &mapping.outer {
                    let target = self
                        .index
                        .capabilities
                        .get(&outer.name)
                        .map_or(Target::None, |id| Target::Symbol(*id));
                    self.occur(outer.span, Role::Capability, target, false);
                }
            }
            self.index.uses.push(UseInfo {
                symbol: id,
                alias: use_decl.alias.name.clone(),
                path,
                path_span,
                span: use_decl.span,
            });
        }
        for unit in &program.units {
            self.unit(unit);
        }
    }

    /// The span of the path literal of a `use`, found in the source: the AST
    /// keeps the literal but not where it was written.
    fn use_path_span(&self, span: Span) -> Span {
        let start = span.start.offset as usize;
        let end = (span.end.offset as usize).min(self.source.len());
        let text = self.source.get(start..end).unwrap_or_default();
        let Some(open) = text.find('"') else {
            return span;
        };
        let close = text[open + 1..]
            .find('"')
            .map_or(text.len(), |i| open + 1 + i + 1);
        offset_span(span, self.source, start + open, start + close)
    }

    fn decl(&mut self, decl: &Decl) {
        match decl {
            Decl::In { name, shape, span } => {
                let detail = self.header(*span);
                let id = self.declare(name, SymbolKind::Input, Role::Input, *span, None, detail);
                let fields = self.shape(shape, id);
                self.index.globals.insert(
                    name.name.clone(),
                    Binding {
                        symbol: Some(id),
                        source: (!fields.is_empty()).then_some(Source::Record(fields)),
                    },
                );
            }
            Decl::Out { name, span } => {
                let detail = self.header(*span);
                let id = self.declare(name, SymbolKind::Output, Role::Output, *span, None, detail);
                self.index.globals.insert(
                    name.name.clone(),
                    Binding {
                        symbol: Some(id),
                        source: None,
                    },
                );
            }
            Decl::Needs {
                name,
                kind,
                signatures,
                span,
            } => {
                let detail = self.header(*span);
                let id = self.declare(
                    name,
                    SymbolKind::Capability(*kind),
                    Role::Capability,
                    *span,
                    None,
                    detail,
                );
                self.index.capabilities.insert(name.name.clone(), id);
                for signature in signatures {
                    let detail = self.header(signature.span);
                    let verb = self.declare(
                        &signature.name,
                        SymbolKind::ToolVerb,
                        Role::Verb,
                        signature.span,
                        Some(id),
                        detail,
                    );
                    for param in &signature.params {
                        self.declare(
                            param,
                            SymbolKind::Parameter,
                            Role::Parameter,
                            param.span,
                            Some(verb),
                            param.name.clone(),
                        );
                    }
                }
            }
        }
    }

    /// The fields of a record shape, as symbols under `parent`.
    fn shape(&mut self, shape: &Shape, parent: SymbolId) -> Vec<SymbolId> {
        match shape {
            Shape::Type { span, .. } => {
                self.occur(*span, Role::Type, Target::None, false);
                Vec::new()
            }
            Shape::Record { fields, .. } => fields
                .iter()
                .map(|field: &FieldDecl| {
                    let id = self.declare(
                        &field.name,
                        SymbolKind::InputField,
                        Role::Property,
                        field.span,
                        Some(parent),
                        field.name.name.clone(),
                    );
                    if let Some(shape) = &field.shape {
                        self.shape(shape, id);
                    }
                    id
                })
                .collect(),
        }
    }

    /* ---------------------------------------------------------------- */
    /* units                                                             */
    /* ---------------------------------------------------------------- */

    fn unit(&mut self, unit: &Unit) {
        let (name, span) = match unit {
            Unit::Judgment(u) => (&u.name, u.span),
            Unit::Task(u) => (&u.name, u.span),
            Unit::Def(u) => (&u.name, u.span),
            Unit::Machine(u) => (&u.name, u.span),
        };
        let Some(id) = self
            .index
            .symbols
            .iter()
            .position(|s| s.selection == name.span)
        else {
            return;
        };
        self.scope = BTreeMap::new();
        self.unit = Some(id);
        match unit {
            Unit::Judgment(judgment) => {
                for param in &judgment.params {
                    self.bind_param(param, id);
                }
                for result in &judgment.results {
                    let detail = self.header(result.span);
                    let result_id = self.declare(
                        &result.name,
                        SymbolKind::Result,
                        Role::Property,
                        result.span,
                        Some(id),
                        detail,
                    );
                    self.judge(&result.question, result_id);
                    // A `log` line reads the results written above it by
                    // name (spec section 5.8).
                    self.scope.insert(
                        result.name.name.clone(),
                        Binding {
                            symbol: Some(result_id),
                            source: None,
                        },
                    );
                }
                for log in &judgment.logs {
                    self.expr(&log.log, false);
                }
            }
            Unit::Task(task) => {
                self.params(&task.params, id);
                self.budgets_and_thresholds(&task.budgets, &task.thresholds);
                self.collect_locals(&task.body.statements, id);
                self.block(&task.body);
            }
            Unit::Def(def) => {
                self.params(&def.params, id);
                self.collect_locals(&def.body.statements, id);
                self.block(&def.body);
            }
            Unit::Machine(machine) => self.machine(machine, id),
        }
        let names = std::mem::take(&mut self.scope);
        self.index.scopes.push(Scope {
            unit: id,
            span,
            names,
        });
    }

    fn bind_param(&mut self, name: &Ident, unit: SymbolId) -> SymbolId {
        let id = self.declare(
            name,
            SymbolKind::Parameter,
            Role::Parameter,
            name.span,
            Some(unit),
            name.name.clone(),
        );
        self.scope.insert(
            name.name.clone(),
            Binding {
                symbol: Some(id),
                source: None,
            },
        );
        id
    }

    fn params(&mut self, params: &[Param], unit: SymbolId) {
        for param in params {
            self.bind_param(&param.name, unit);
            if let Some(default) = &param.default {
                self.expr(default, false);
            }
        }
    }

    fn budgets_and_thresholds(
        &mut self,
        budgets: &[jevscript_syntax::ast::BudgetItem],
        thresholds: &[jevscript_syntax::ast::ThresholdItem],
    ) {
        for budget in budgets {
            let start = budget.span.start.offset as usize;
            let word = self.source[start..]
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .map_or(0, |len| len);
            let span = offset_span(budget.span, self.source, start, start + word);
            self.occur(span, Role::Property, Target::None, false);
        }
        for threshold in thresholds {
            self.occur(threshold.name.span, Role::Property, Target::None, false);
        }
    }

    fn machine(&mut self, machine: &MachineUnit, id: SymbolId) {
        self.params(&machine.params, id);
        self.budgets_and_thresholds(&machine.budgets, &machine.thresholds);
        if let Some(goal) = &machine.goal {
            self.expr(goal, false);
        }
        let mut fields = Vec::new();
        for field in &machine.observe {
            fields.push(self.shape_field(field, id));
        }
        self.scope.insert(
            "obs".to_string(),
            Binding {
                symbol: None,
                source: Some(Source::Shape(fields)),
            },
        );
        let mut states = BTreeMap::new();
        for state in &machine.states {
            let detail = self.header(state.span);
            let state_id = self.declare(
                &state.name,
                SymbolKind::State,
                Role::State,
                state.span,
                Some(id),
                detail,
            );
            states.entry(state.name.name.clone()).or_insert(state_id);
        }
        if let Some(initial) = &machine.initial {
            let target = states
                .get(&initial.name)
                .map_or(Target::None, |s| Target::Symbol(*s));
            self.occur(initial.span, Role::State, target, false);
        }
        let bodies: Vec<&Block> = machine
            .states
            .iter()
            .flat_map(|s| s.transitions.iter().filter_map(|t| t.body.as_ref()))
            .collect();
        for body in &bodies {
            self.collect_locals(&body.statements, id);
        }
        for state in &machine.states {
            let Some(state_id) = self
                .index
                .symbols
                .iter()
                .position(|s| s.selection == state.name.span)
            else {
                continue;
            };
            for transition in &state.transitions {
                let detail = self.header(transition.span);
                self.declare(
                    &transition.event,
                    SymbolKind::Event,
                    Role::Event,
                    transition.span,
                    Some(state_id),
                    detail,
                );
                self.expr(&transition.description, false);
                let target = states
                    .get(&transition.target.name)
                    .map_or(Target::None, |s| Target::Symbol(*s));
                self.occur(transition.target.span, Role::State, target, false);
                if let Some(when) = &transition.when {
                    self.expr(when, false);
                }
                if let Some(body) = &transition.body {
                    self.block(body);
                }
            }
        }
    }

    /// Declare every variable a body assigns, at its first assignment, so a
    /// read resolves whichever branch it sits in (spec section 5.3).
    fn collect_locals(&mut self, statements: &[Stmt], unit: SymbolId) {
        for stmt in statements {
            match stmt {
                Stmt::Assign {
                    target,
                    value,
                    span,
                } if target.path.is_empty() => {
                    let source = match source_of(value) {
                        Some(Source::Unit(path)) => {
                            let idents: Vec<Ident> = path
                                .iter()
                                .map(|name| Ident {
                                    name: name.clone(),
                                    span: Span::default(),
                                })
                                .collect();
                            let refs: Vec<&Ident> = idents.iter().collect();
                            self.unit_path(&refs).map(Source::Unit)
                        }
                        other => other,
                    };
                    self.local(&target.root, *span, unit, source);
                }
                Stmt::Shape(shape) => {
                    self.local(&shape.target, shape.span, unit, None);
                }
                Stmt::For { names, body, .. } => {
                    for name in names {
                        self.local(name, name.span, unit, None);
                    }
                    self.collect_locals(&body.statements, unit);
                }
                Stmt::If {
                    branches,
                    otherwise,
                    ..
                } => {
                    for branch in branches {
                        self.collect_locals(&branch.body.statements, unit);
                    }
                    if let Some(body) = otherwise {
                        self.collect_locals(&body.statements, unit);
                    }
                }
                Stmt::Loop { body, .. } | Stmt::Until { body, .. } => {
                    self.collect_locals(&body.statements, unit);
                }
                Stmt::Gate(gate) => {
                    for arm in &gate.arms {
                        self.collect_locals(&arm.body.statements, unit);
                    }
                }
                _ => {}
            }
        }
    }

    fn local(&mut self, name: &Ident, full: Span, unit: SymbolId, source: Option<Source>) {
        if self.scope.contains_key(&name.name) || self.index.globals.contains_key(&name.name) {
            return;
        }
        let detail = self.header(full);
        let id = self.symbol(name, SymbolKind::Local, full, Some(unit), detail);
        self.scope.insert(
            name.name.clone(),
            Binding {
                symbol: Some(id),
                source,
            },
        );
    }

    /* ---------------------------------------------------------------- */
    /* statements                                                        */
    /* ---------------------------------------------------------------- */

    fn block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign { target, value, .. } => {
                let root = self.name(&target.root, false);
                let mut owner = match root {
                    Target::Symbol(id) => Some(id),
                    _ => None,
                };
                for field in &target.path {
                    self.occur(field.span, Role::Property, Target::None, false);
                    owner = None;
                }
                match value {
                    Expr::Judge(judge) => {
                        let parent = owner.or(self.unit).unwrap_or(0);
                        self.judge(judge, parent);
                        // The labels are what this variable can be compared
                        // against with `is`.
                        let labels = self.index.labels_of(parent);
                        if let Some(binding) = self.scope.get_mut(&target.root.name)
                            && !labels.is_empty()
                        {
                            binding.source = Some(Source::Labels(labels));
                        }
                    }
                    _ => self.expr(value, false),
                }
            }
            Stmt::Expr { expr, .. } => self.expr(expr, false),
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in branches {
                    self.expr(&branch.test, false);
                    self.block(&branch.body);
                }
                if let Some(body) = otherwise {
                    self.block(body);
                }
            }
            Stmt::For {
                names,
                iterable,
                body,
                ..
            } => {
                for name in names {
                    self.name(name, false);
                }
                self.expr(iterable, false);
                self.block(body);
            }
            Stmt::Loop { body, .. } => self.block(body),
            Stmt::Until { test, body, .. } => {
                self.expr(test, false);
                self.block(body);
            }
            Stmt::Gate(gate) => {
                for expr in [&gate.risk, &gate.confidence, &gate.done]
                    .into_iter()
                    .flatten()
                {
                    self.expr(expr, false);
                }
                for arm in &gate.arms {
                    self.block(&arm.body);
                }
            }
            Stmt::Shape(shape) => {
                self.name(&shape.target, false);
                let owner = self.scope.get(&shape.target.name).and_then(|b| b.symbol);
                let mut fields = Vec::new();
                for field in &shape.fields {
                    fields.push(self.shape_field(field, owner.unwrap_or(0)));
                }
                if let Some(binding) = self.scope.get_mut(&shape.target.name) {
                    binding.source = Some(Source::Shape(fields));
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.expr(value, false);
                }
            }
            Stmt::Stop { reason, .. } | Stmt::Escalate { reason, .. } => self.expr(reason, false),
            Stmt::Continue { .. } | Stmt::Break { .. } => {}
        }
    }

    fn shape_field(&mut self, field: &ShapeField, parent: SymbolId) -> SymbolId {
        let detail = self.header(field.span);
        let id = self.declare(
            &field.name,
            SymbolKind::ShapeField,
            Role::Property,
            field.span,
            Some(parent),
            detail,
        );
        self.expr(&field.value, false);
        id
    }

    /* ---------------------------------------------------------------- */
    /* names                                                             */
    /* ---------------------------------------------------------------- */

    /// Resolve a bare name and record it. Returns what it refers to.
    fn name(&mut self, ident: &Ident, callee: bool) -> Target {
        let (role, target) = self.resolve(&ident.name, callee);
        let declaration = matches!(target, Target::Symbol(id)
            if self.index.symbols[id].selection == ident.span);
        self.occur(ident.span, role, target.clone(), declaration);
        target
    }

    fn resolve(&self, name: &str, callee: bool) -> (Role, Target) {
        if let Some(binding) = self.scope.get(name) {
            let role = match binding.symbol.map(|id| self.index.symbols[id].kind) {
                Some(SymbolKind::Parameter) => Role::Parameter,
                _ => Role::Variable,
            };
            return (role, binding.symbol.map_or(Target::None, Target::Symbol));
        }
        if let Some(binding) = self.index.globals.get(name) {
            let role = match binding.symbol.map(|id| self.index.symbols[id].kind) {
                Some(SymbolKind::Output) => Role::Output,
                _ => Role::Input,
            };
            return (role, binding.symbol.map_or(Target::None, Target::Symbol));
        }
        if let Some(id) = self.index.capabilities.get(name) {
            return (Role::Capability, Target::Symbol(*id));
        }
        if let Some((id, kind)) = self.index.unit(name) {
            return (Role::Unit(kind), Target::Symbol(id));
        }
        if let Some(use_info) = self.index.uses.iter().find(|u| u.alias == name) {
            return (Role::Module, Target::Symbol(use_info.symbol));
        }
        if name == PRELUDE_ALIAS {
            return (Role::Module, Target::None);
        }
        if self.prelude.iter().any(|p| p == name) {
            return (Role::Prelude, Target::Unit(vec![name.to_string()]));
        }
        if callee && is_builtin(name) {
            return (Role::Builtin, Target::Builtin(name.to_string()));
        }
        (Role::Variable, Target::None)
    }

    /// The source a variable's binding records, if the name is bound.
    fn binding_source(&self, name: &str) -> Option<Source> {
        self.scope
            .get(name)
            .or_else(|| self.index.globals.get(name))
            .and_then(|b| b.source.clone())
    }

    /// A dotted path: `alias.unit`, `cap.verb`, `j.result`, `obs.field`.
    fn path(&mut self, path: &[&Ident], callee: bool) {
        let root = path[0];
        let target = self.name(root, callee && path.len() == 1);
        let rest = &path[1..];
        if rest.is_empty() {
            return;
        }
        let root_kind = match &target {
            Target::Symbol(id) => Some(self.index.symbols[*id].kind),
            _ => None,
        };
        let module = root_kind == Some(SymbolKind::Module)
            || (root.name == PRELUDE_ALIAS && target == Target::None);
        match (root_kind, &target) {
            _ if module => {
                // `alias.unit`, or `a.b.unit` through a library's own alias.
                let mut written = vec![root.name.clone()];
                for (i, segment) in rest.iter().enumerate() {
                    written.push(segment.name.clone());
                    if i + 1 == rest.len() {
                        self.occur(
                            segment.span,
                            Role::Unit(UnitKind::Def),
                            Target::Unit(written.clone()),
                            false,
                        );
                    } else {
                        self.occur(
                            segment.span,
                            Role::Module,
                            Target::Module(written.clone()),
                            false,
                        );
                    }
                }
            }
            (Some(SymbolKind::Capability(kind)), Target::Symbol(cap)) => {
                let verb = rest[0];
                let declared = self.index.child(*cap, &verb.name);
                let target = match declared {
                    Some(id) => Target::Symbol(id),
                    None => Target::Verb {
                        kind,
                        verb: verb.name.clone(),
                    },
                };
                self.occur(verb.span, Role::Verb, target, false);
                self.properties(&rest[1..]);
            }
            _ => {
                let first = rest[0];
                let source = self.binding_source(&root.name);
                let (role, target) = match &source {
                    Some(Source::Unit(unit)) => (
                        Role::Property,
                        Target::Member {
                            unit: unit.clone(),
                            member: first.name.clone(),
                        },
                    ),
                    Some(Source::Shape(fields) | Source::Record(fields)) => (
                        Role::Property,
                        fields
                            .iter()
                            .find(|id| self.index.symbols[**id].name == first.name)
                            .map_or(Target::None, |id| Target::Symbol(*id)),
                    ),
                    _ if AGENT_VERBS.contains(&first.name.as_str())
                        && (matches!(source, Some(Source::Handle))
                            || (callee && rest.len() == 1)
                            || matches!(first.name.as_str(), "observe" | "stop")) =>
                    {
                        (Role::Verb, Target::HandleVerb(first.name.clone()))
                    }
                    _ if callee && rest.len() == 1 => (Role::Verb, Target::None),
                    _ => (Role::Property, Target::None),
                };
                self.occur(first.span, role, target, false);
                // `dev.observe.last_message`: what a verb returns has fields.
                self.properties(&rest[1..]);
            }
        }
    }

    /// Record where a `log` expression starting at `span` writes its `log`
    /// word and its level: the first two words of the expression.
    fn log_word(&mut self, span: Span) {
        let start = span.start.offset as usize;
        let text = self.source.get(start..).unwrap_or_default();
        let word_end = |from: usize| {
            text[from..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(text.len(), |len| from + len)
        };
        let log_end = word_end(0);
        let Some(gap) = text[log_end..].find(|c: char| c != ' ') else {
            return;
        };
        let level_start = log_end + gap;
        let level_end = word_end(level_start);
        let log = offset_span(span, self.source, start, start + log_end);
        let level = offset_span(span, self.source, start + level_start, start + level_end);
        self.index.logs.push((log, level));
    }

    fn properties(&mut self, rest: &[&Ident]) {
        for segment in rest {
            self.occur(segment.span, Role::Property, Target::None, false);
        }
    }

    /* ---------------------------------------------------------------- */
    /* expressions                                                       */
    /* ---------------------------------------------------------------- */

    fn expr(&mut self, expr: &Expr, callee: bool) {
        if let Some(path) = dotted(expr) {
            self.path(&path, callee);
            return;
        }
        match expr {
            Expr::Number { .. } | Expr::Bool { .. } | Expr::None { .. } | Expr::Trail { .. } => {}
            Expr::Name(_) => unreachable!("a name is a dotted path"),
            Expr::Text { value, .. } => {
                for part in &value.parts {
                    if let TextPart::Interpolation { source, span } = part
                        && let Ok(hole) = parse_expression(source, *span)
                    {
                        self.expr(&hole, false);
                    }
                }
            }
            Expr::List { items, .. } => {
                for item in items {
                    self.expr(item, false);
                }
            }
            Expr::Record { fields, .. } => {
                for field in fields {
                    match &field.value {
                        Some(value) => {
                            self.occur(field.name.span, Role::Property, Target::None, false);
                            self.expr(value, false);
                        }
                        // `{ title }` reads the variable `title`.
                        None => {
                            self.name(&field.name, false);
                        }
                    }
                }
            }
            Expr::Field { target, name, .. } => {
                self.expr(target, false);
                let role = if callee || matches!(name.name.as_str(), "observe" | "stop") {
                    Role::Verb
                } else {
                    Role::Property
                };
                self.occur(name.span, role, Target::None, false);
            }
            Expr::Index { target, index, .. } => {
                self.expr(target, false);
                self.expr(index, false);
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee, true);
                let unit = dotted(callee).and_then(|path| self.unit_path(&path));
                self.args(args, unit);
            }
            Expr::Unary { operand, .. } => self.expr(operand, false),
            Expr::Binary { left, right, .. } => {
                self.expr(left, false);
                self.expr(right, false);
            }
            Expr::Is { target, label, .. } => {
                self.expr(target, false);
                let resolved = self.label_target(target, &label.name);
                self.occur(label.span, Role::Label, resolved, false);
            }
            Expr::Comprehension {
                expr,
                names,
                iterable,
                test,
                ..
            } => {
                self.expr(iterable, false);
                let saved: Vec<(String, Option<Binding>)> = names
                    .iter()
                    .map(|n| (n.name.clone(), self.scope.get(&n.name).cloned()))
                    .collect();
                for name in names {
                    let unit = self.unit;
                    let id = self.declare(
                        name,
                        SymbolKind::Local,
                        Role::Variable,
                        name.span,
                        unit,
                        name.name.clone(),
                    );
                    self.scope.insert(
                        name.name.clone(),
                        Binding {
                            symbol: Some(id),
                            source: None,
                        },
                    );
                }
                self.expr(expr, false);
                if let Some(test) = test {
                    self.expr(test, false);
                }
                for (name, binding) in saved {
                    match binding {
                        Some(binding) => self.scope.insert(name, binding),
                        None => self.scope.remove(&name),
                    };
                }
            }
            Expr::Focus { text, on, .. } => {
                self.expr(text, false);
                self.expr(on, false);
            }
            Expr::Judge(judge) => self.judge(judge, self.unit.unwrap_or(0)),
            Expr::Log {
                value,
                fields,
                span,
                ..
            } => {
                self.log_word(*span);
                self.expr(value, false);
                for field in fields {
                    match &field.value {
                        Some(value) => {
                            self.occur(field.name.span, Role::Property, Target::None, false);
                            self.expr(value, false);
                        }
                        // `{ owner }` logs the variable `owner`.
                        None => {
                            self.name(&field.name, false);
                        }
                    }
                }
            }
        }
    }

    /// The unit a dotted callee names, as written, if it names one.
    fn unit_path(&self, path: &[&Ident]) -> Option<Vec<String>> {
        let written: Vec<String> = path.iter().map(|i| i.name.clone()).collect();
        match path {
            [name] => {
                if self.scope.contains_key(&name.name) {
                    return None;
                }
                (self.index.units.contains_key(&name.name) || self.prelude.contains(&name.name))
                    .then_some(written)
            }
            [alias, ..] => (self.index.uses.iter().any(|u| u.alias == alias.name)
                || alias.name == PRELUDE_ALIAS)
                .then_some(written),
            [] => None,
        }
    }

    fn args(&mut self, args: &[Arg], unit: Option<Vec<String>>) {
        for arg in args {
            if let Some(name) = &arg.name {
                let target = match &unit {
                    Some(unit) => Target::UnitParam {
                        unit: unit.clone(),
                        param: name.name.clone(),
                    },
                    None => Target::None,
                };
                self.occur(name.span, Role::Parameter, target, false);
            }
            self.expr(&arg.value, false);
        }
    }

    /// What the label in `<target> is <label>` names.
    fn label_target(&self, target: &Expr, label: &str) -> Target {
        let fallback = Target::Label(label.to_string());
        let Some(path) = dotted(target) else {
            return fallback;
        };
        let source = self.binding_source(&path[0].name);
        match (path.len(), source) {
            (1, Some(Source::Labels(labels))) => labels
                .iter()
                .find(|id| self.index.symbols[**id].name == label)
                .map_or(fallback, |id| Target::Symbol(*id)),
            (2, Some(Source::Unit(unit))) => Target::MemberLabel {
                unit,
                member: path[1].name.clone(),
                label: label.to_string(),
            },
            _ => fallback,
        }
    }

    /* ---------------------------------------------------------------- */
    /* judgments                                                         */
    /* ---------------------------------------------------------------- */

    fn judge(&mut self, judge: &JudgeExpr, parent: SymbolId) {
        let subject: Vec<&Ident> = std::iter::once(&judge.subject.root)
            .chain(judge.subject.path.iter().filter_map(|step| match step {
                SubjectStep::Field(ident) => Some(ident),
                SubjectStep::Index(_) => None,
            }))
            .collect();
        let pure = judge
            .subject
            .path
            .iter()
            .all(|step| matches!(step, SubjectStep::Field(_)));
        if pure {
            self.path(&subject, false);
        } else {
            self.name(&judge.subject.root, false);
            for step in &judge.subject.path {
                match step {
                    SubjectStep::Field(ident) => {
                        self.occur(ident.span, Role::Property, Target::None, false)
                    }
                    SubjectStep::Index(index) => self.expr(index, false),
                }
            }
        }
        match &judge.verb {
            JudgeVerb::Feels { condition } => self.expr(condition, false),
            JudgeVerb::Pick { labels } => {
                for label in labels {
                    if !label.escape {
                        let detail = self.header(label.span);
                        self.declare(
                            &label.name,
                            SymbolKind::Label,
                            Role::Label,
                            label.span,
                            Some(parent),
                            detail,
                        );
                    }
                    if let Some(description) = &label.description {
                        self.expr(description, false);
                    }
                    if let Some(detail) = &label.detail {
                        self.detail(detail);
                    }
                }
            }
            JudgeVerb::Rate { levels } => {
                for level in levels {
                    if let Some(name) = &level.name {
                        let detail = self.header(level.span);
                        self.declare(
                            name,
                            SymbolKind::Label,
                            Role::Label,
                            level.span,
                            Some(parent),
                            detail,
                        );
                    }
                    self.expr(&level.situation, false);
                }
            }
            JudgeVerb::PickAmong { question, by, .. } => {
                self.expr(question, false);
                if let Some(by) = by {
                    self.occur(by.span, Role::Property, Target::None, false);
                }
            }
        }
        if let Some(detail) = &judge.detail {
            self.detail(detail);
        }
    }

    fn detail(&mut self, detail: &Detail) {
        for expr in [
            &detail.focus,
            &detail.note,
            &detail.compare,
            &detail.yes,
            &detail.no,
            &detail.examples,
            &detail.what,
            &detail.not_for,
        ]
        .into_iter()
        .flatten()
        {
            self.expr(expr, false);
        }
    }
}

/// What an assignment's right-hand side makes the variable hold.
fn source_of(value: &Expr) -> Option<Source> {
    let callee = match value {
        Expr::Call { callee, .. } => callee,
        other => {
            // A zero-argument unit call written as a bare name is not a
            // call; only a path with a verb is.
            return match dotted(other) {
                Some(path) if path.len() == 2 && path[1].name == "spawn" => Some(Source::Handle),
                _ => None,
            };
        }
    };
    let path = dotted(callee)?;
    if path.last().is_some_and(|verb| verb.name == "spawn") {
        return Some(Source::Handle);
    }
    Some(Source::Unit(path.iter().map(|i| i.name.clone()).collect()))
}

/// A span for the bytes `start..end` of `source`, lines and columns counted
/// from `base`, which must start on the same line.
fn offset_span(base: Span, source: &str, start: usize, end: usize) -> Span {
    use jevscript_syntax::Pos;
    let from = base.start.offset as usize;
    let column = |offset: usize| {
        base.start.column + source.get(from..offset).map_or(0, |s| s.chars().count()) as u32
    };
    Span::new(
        Pos::new(base.start.line, column(start), start as u32),
        Pos::new(base.start.line, column(end), end as u32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(source: &str) -> Index {
        let program = jevscript_syntax::parse(source).expect("parses");
        Index::build(&program, source)
    }

    fn at<'a>(index: &'a Index, source: &str, needle: &str, nth: usize) -> &'a Occurrence {
        let offset = source
            .match_indices(needle)
            .nth(nth)
            .unwrap_or_else(|| panic!("`{needle}` #{nth} is in the source"))
            .0;
        index
            .occurrence_at(offset)
            .unwrap_or_else(|| panic!("an occurrence at `{needle}` #{nth}"))
    }

    const INLINE: &str = include_str!("../../../examples/fix_issue_inline.jev");
    const LIBRARY_USER: &str = include_str!("../../../examples/fix_issue.jev");
    const REVIEW: &str = include_str!("../../../examples/review_loop.jev");

    #[test]
    fn a_unit_call_resolves_to_the_unit() {
        let index = index(INLINE);
        let call = at(&index, INLINE, "read_agent obs", 0);
        let Target::Symbol(id) = call.target else {
            panic!("a local unit: {:?}", call.target);
        };
        assert_eq!(index.symbols[id].kind, SymbolKind::Unit(UnitKind::Judgment));
        assert_eq!(
            index.symbols[id].detail,
            "judgment read_agent(summary, files, tests, recent)"
        );
    }

    #[test]
    fn a_result_of_a_called_judgment_is_a_member() {
        // Spec 6.7: calling a judgment returns a record of its results.
        let index = index(INLINE);
        let next = at(&index, INLINE, "next is stuck", 0);
        assert_eq!(
            next.target,
            Target::Member {
                unit: vec!["read_agent".into()],
                member: "next".into()
            }
        );
        let label = at(&index, INLINE, "stuck:", 0);
        assert_eq!(label.role, Role::Label);
    }

    #[test]
    fn capability_verbs_and_handle_verbs() {
        // Spec 9.1: `spawn` returns a handle, whose verbs are agent verbs.
        let index = index(INLINE);
        let spawn = at(&index, INLINE, "spawn", 0);
        assert_eq!(
            spawn.target,
            Target::Verb {
                kind: CapabilityKind::Agent,
                verb: "spawn".into()
            }
        );
        let send = at(&index, INLINE, "send", 0);
        assert_eq!(send.target, Target::HandleVerb("send".into()));
        let claude = at(&index, INLINE, "claude.spawn", 0);
        assert_eq!(claude.role, Role::Capability);
    }

    #[test]
    fn a_qualified_name_through_an_alias_is_a_unit_path() {
        // Spec 3.9: exported units are referred to as `<alias>.<name>`.
        let index = index(LIBRARY_USER);
        assert_eq!(
            at(&index, LIBRARY_USER, "watch", 0).target,
            Target::Unit(vec!["harness".into(), "watch".into()])
        );
        assert_eq!(
            at(&index, LIBRARY_USER, "harness.watch", 0).role,
            Role::Module
        );
        assert_eq!(index.uses[0].path.as_deref(), Some("./lib/agent_loop.jev"));
        let path = index.uses[0].path_span;
        assert_eq!(
            &LIBRARY_USER[path.start.offset as usize..path.end.offset as usize],
            "\"./lib/agent_loop.jev\""
        );
    }

    #[test]
    fn a_prelude_def_and_a_builtin() {
        let index = index(INLINE);
        let stuck = at(&index, INLINE, "stuck(obs", 0);
        assert_eq!(stuck.role, Role::Prelude);
        assert_eq!(stuck.target, Target::Unit(vec!["stuck".into()]));
        let max = at(&index, INLINE, "max(j", 0);
        assert_eq!(max.target, Target::Builtin("max".into()));
    }

    #[test]
    fn shape_fields_and_record_inputs_resolve_to_their_fields() {
        let index = index(INLINE);
        let Target::Symbol(summary) = at(&index, INLINE, "summary\n", 0).target.clone() else {
            panic!("`me.ask obs.summary` resolves to the shape field");
        };
        assert_eq!(index.symbols[summary].kind, SymbolKind::ShapeField);
        let Target::Symbol(title) = at(&index, INLINE, "title}", 0).target.clone() else {
            panic!("`issue.title` inside an interpolation resolves");
        };
        assert_eq!(index.symbols[title].kind, SymbolKind::InputField);
    }

    #[test]
    fn machine_states_events_and_obs() {
        // Spec 7.8.
        let index = index(REVIEW);
        let Target::Symbol(state) = at(&index, REVIEW, "reviewing when", 0).target.clone() else {
            panic!("a state");
        };
        assert_eq!(index.symbols[state].kind, SymbolKind::State);
        let Target::Symbol(tests) = at(&index, REVIEW, "tests}", 0).target.clone() else {
            panic!("`obs.tests` resolves to the observe field");
        };
        assert_eq!(index.symbols[tests].kind, SymbolKind::ShapeField);
        let events = index
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Event)
            .count();
        assert_eq!(events, 8);
    }

    #[test]
    fn named_arguments_to_a_unit_point_at_its_parameters() {
        let source = include_str!("../../../examples/lib/agent_loop.jev");
        let index = index(source);
        assert_eq!(
            at(&index, source, "summary: obs", 0).target,
            Target::UnitParam {
                unit: vec!["read_agent".into()],
                param: "summary".into()
            }
        );
    }

    #[test]
    fn locals_are_declared_at_their_first_assignment() {
        let index = index(INLINE);
        let first = at(&index, INLINE, "dev = ", 0);
        assert!(first.declaration);
        let Target::Symbol(dev) = first.target else {
            panic!("a symbol");
        };
        let reads = index
            .occurrences
            .iter()
            .filter(|o| o.target == Target::Symbol(dev) && !o.declaration)
            .count();
        assert!(reads >= 5, "{reads}");
    }
}
