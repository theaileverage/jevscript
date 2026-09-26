//! Module resolution and linking (spec section 3.9).
//!
//! One `.jev` file is one module, and `use "<path>" as <alias> with <mapping>`
//! brings another file's units into scope. There is no registry and no version
//! syntax in 0.1: composition is files.
//!
//! What this pass does:
//!
//! 1. **Resolve.** A path starting with `./` or `../` is relative to the
//!    importing file. Anything else is resolved against the host's search roots,
//!    in order — the SDK's `paths` option, then `JEVSCRIPT_PATH`, colon
//!    separated. A path that resolves to nothing is
//!    [`ErrorCode::UseNotFound`].
//! 2. **Check the shape of the import.** A file that is imported may declare
//!    `needs` but not `in` or `out`; one that does is a program, and importing it
//!    is [`ErrorCode::UseHasInputs`]. Modules that import each other are
//!    [`ErrorCode::UseCycle`].
//! 3. **Map capabilities.** Every `needs` in the library must have an entry in
//!    the `with` clause and the kinds must match, or
//!    [`ErrorCode::UseNeedsUnmapped`]. Nothing is bound implicitly: a library
//!    cannot reach a capability it was not handed. Because the mapping is
//!    static, verbs are checked against kinds across the module boundary exactly
//!    as they are within a file.
//! 4. **Qualify and inline.** Every exported unit is renamed to
//!    `<alias>.<name>`, and the root program's IR carries every transitively
//!    used module inlined, with a [`jevscript_ir::Module`] per module recording
//!    where its units came from. The runtime and the SDKs see one flat program.
//!    A name starting with `_` is private to its file, and a reference to one
//!    from another module is [`ErrorCode::UsePrivate`]. That reference check
//!    runs in [`crate::check`], which is the pass that walks every reference;
//!    this pass only decides what each module can see.
//!
//! The prelude is the implicit module `std` (spec sections 7.3 and 8.1), linked
//! into every program last: its exported names are in scope unqualified and
//! also as `std.stuck`. A user definition of the same name shadows the
//! unqualified form, with a warning.
//!
//! Judgment shape hashes (spec section 11.4) are computed on the qualified
//! unit, so moving a judgment between files without changing it keeps its hash.
//!
//! Every diagnostic raised inside a library names that file in its `file`
//! field, so a library's errors are reported against the library (spec
//! sections 3.9 and 12).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jevscript_syntax::ast::{CapabilityKind, Decl, ToolSignature, Unit, UseDecl};
use jevscript_syntax::{Diagnostic, ErrorCode, Program, Span, parse};

/// The implicit module the standard prelude is exported as.
pub const PRELUDE_ALIAS: &str = "std";

/// The prelude's source: the defs of spec sections 7.3 and 8.1, verbatim, as
/// `program std`.
pub const PRELUDE_SOURCE: &str = include_str!("prelude.jev");

/// The environment variable that adds search roots, colon separated.
pub const SEARCH_PATH_ENV: &str = "JEVSCRIPT_PATH";

/// Where to look for a `use` path that is not relative, and where to read
/// the files it finds.
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    /// Search roots, in the order they are tried.
    pub roots: Vec<PathBuf>,
    /// Source to read instead of a file's contents on disk, by the file's
    /// canonical path. An editor puts its unsaved buffers here, so an
    /// importer is linked against the library as it is being edited. Empty
    /// for every other host.
    pub overlay: BTreeMap<PathBuf, String>,
}

impl Resolver {
    /// A resolver with the roots from [`SEARCH_PATH_ENV`].
    pub fn from_env() -> Self {
        let roots = std::env::var(SEARCH_PATH_ENV)
            .ok()
            .map(|value| {
                value
                    .split(':')
                    .filter(|part| !part.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            roots,
            overlay: BTreeMap::new(),
        }
    }

    /// Whether `path` names a module: a file on disk or an overlaid source.
    fn exists(&self, path: &Path) -> bool {
        path.exists() || self.overlay.contains_key(&canonical(path))
    }

    /// The source of the module at `path`: the overlay's, if it has one.
    ///
    /// # Errors
    ///
    /// The error reading the file, when it is not overlaid.
    pub fn read(&self, path: &Path) -> std::io::Result<String> {
        match self.overlay.get(&canonical(path)) {
            Some(source) => Ok(source.clone()),
            None => std::fs::read_to_string(path),
        }
    }

    /// Resolve one `use` path against the importing file.
    ///
    /// A path starting with `./` or `../` is relative to `importer`'s directory
    /// and is never looked up in the roots.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UseNotFound`] if nothing exists at any candidate.
    pub fn resolve(&self, importer: &Path, path: &str) -> Result<PathBuf, ErrorCode> {
        if path.starts_with("./") || path.starts_with("../") {
            let candidate = importer.parent().unwrap_or(Path::new(".")).join(path);
            return if self.exists(&candidate) {
                Ok(candidate)
            } else {
                Err(ErrorCode::UseNotFound)
            };
        }
        for root in &self.roots {
            let candidate = root.join(path);
            if self.exists(&candidate) {
                return Ok(candidate);
            }
        }
        Err(ErrorCode::UseNotFound)
    }
}

/// Whether a unit name is exported from its module.
///
/// Every `judgment`, `task`, `def` and `machine` whose name does not start
/// with `_` is exported (spec section 3.9).
pub fn is_exported(name: &str) -> bool {
    !name.starts_with('_')
}

/// The qualified name of a unit in a module: `harness.read_agent`. The root
/// program's units keep their bare names.
pub fn qualify(alias: &str, name: &str) -> String {
    if alias.is_empty() {
        name.to_string()
    } else {
        format!("{alias}.{name}")
    }
}

/// Which kind of unit a name refers to (spec section 3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    /// A `judgment` block.
    Judgment,
    /// A `task`.
    Task,
    /// A `def`.
    Def,
    /// A `machine`.
    Machine,
}

impl UnitKind {
    /// The keyword the unit was declared with.
    pub const fn keyword(self) -> &'static str {
        match self {
            UnitKind::Judgment => "judgment",
            UnitKind::Task => "task",
            UnitKind::Def => "def",
            UnitKind::Machine => "machine",
        }
    }
}

/// The name and kind of a unit as declared.
pub fn unit_name_and_kind(unit: &Unit) -> (&str, UnitKind) {
    match unit {
        Unit::Judgment(j) => (&j.name.name, UnitKind::Judgment),
        Unit::Task(t) => (&t.name.name, UnitKind::Task),
        Unit::Def(d) => (&d.name.name, UnitKind::Def),
        Unit::Machine(m) => (&m.name.name, UnitKind::Machine),
    }
}

/// A capability as one module sees it: its local name's kind, the root
/// program's name it is bound to, and the signatures that apply to calls on it.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedCapability {
    /// The kind, which both sides of every `with` mapping agreed on.
    pub kind: CapabilityKind,
    /// The root program's name for it. The linked IR only mentions these
    /// (spec section 3.9).
    pub root_name: String,
    /// The `tool` signatures calls are checked against (spec section 9.4): the
    /// module's own block if it wrote one, otherwise the importer's.
    pub signatures: Vec<ToolSignature>,
}

/// One module after linking: its parsed program plus everything it can see.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedModule {
    /// The qualified alias its units are named with: `""` for the root,
    /// `harness` for a `use`, `harness.inner` for a `use` inside that library,
    /// `std` for the prelude.
    pub alias: String,
    /// The parsed file.
    pub program: Program,
    /// Where the file was read from, when it was.
    pub path: Option<PathBuf>,
    /// The `use` path as written, for [`jevscript_ir::Module::path`].
    pub use_path: Option<String>,
    /// How the importer's capabilities satisfied this module's `needs`
    /// (`inner`, importer's local name, kind), in `needs` order.
    pub mapping: Vec<(String, String, CapabilityKind)>,
    /// The capabilities in scope, by the module's own names.
    pub capabilities: BTreeMap<String, LinkedCapability>,
    /// The module aliases this module can reach, by the local alias, mapped to
    /// the qualified alias. Always contains `std`.
    pub visible: BTreeMap<String, String>,
    /// Where the `use` was written in the importer, or the whole file for the
    /// root and the prelude.
    pub span: Span,
}

impl LinkedModule {
    /// The units this module declares, by name.
    pub fn units(&self) -> BTreeMap<&str, UnitKind> {
        self.program.units.iter().map(unit_name_and_kind).collect()
    }

    /// The file diagnostics inside this module name: the root's path when it
    /// was compiled from one, `<prelude>` for the prelude.
    pub fn display_path(&self) -> Option<String> {
        if self.alias == PRELUDE_ALIAS {
            Some("<prelude>".to_string())
        } else {
            self.path.as_ref().map(|p| p.display().to_string())
        }
    }

    /// Attach this module's file to a diagnostic raised inside it.
    pub fn locate(&self, diagnostic: Diagnostic) -> Diagnostic {
        match self.display_path() {
            Some(path) => diagnostic.in_file(path),
            None => diagnostic,
        }
    }
}

/// The result of linking: every module the root reaches, root first, the
/// prelude last.
#[derive(Debug, Clone, PartialEq)]
pub struct Linked {
    /// The modules, root first, then in the order their `use` lines were
    /// reached, then `std`.
    pub modules: Vec<LinkedModule>,
}

/// What a reference to a unit resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A unit, by its qualified name.
    Unit {
        /// The name the linked IR uses.
        qualified: String,
        /// What kind of unit it is.
        kind: UnitKind,
    },
    /// A `_name` reached through an alias (spec section 3.9, `use_private`).
    Private {
        /// The name the reference used.
        qualified: String,
    },
    /// An alias that exports no unit by that name.
    Missing {
        /// The alias, as written.
        alias: String,
        /// The name that was asked for.
        name: String,
    },
    /// An alias this module cannot inspect: a `use` of a program checked
    /// without linking. Nothing can be said about the reference.
    Opaque,
}

impl Linked {
    /// The module with that qualified alias.
    pub fn module(&self, alias: &str) -> Option<&LinkedModule> {
        self.modules.iter().find(|m| m.alias == alias)
    }

    /// The root program.
    pub fn root(&self) -> &LinkedModule {
        &self.modules[0]
    }

    /// The prelude's exported names.
    pub fn prelude_names(&self) -> BTreeSet<&str> {
        self.module(PRELUDE_ALIAS)
            .map(|std| {
                std.units()
                    .into_keys()
                    .filter(|name| is_exported(name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The unit `qualified` names, in whichever module declares it.
    pub fn unit(&self, qualified: &str) -> Option<&Unit> {
        let (alias, name) = match qualified.rsplit_once('.') {
            Some((alias, name)) => (alias, name),
            None => ("", qualified),
        };
        self.module(alias)?
            .program
            .units
            .iter()
            .find(|unit| unit_name_and_kind(unit).0 == name)
    }

    /// Resolve a dotted path written inside `module` — `[name]` or
    /// `[alias, name, ...]` — to the unit it refers to, if it refers to one.
    ///
    /// A bare name is the module's own unit first, then a prelude export that
    /// no own unit shadows. `alias.name` is a unit of the module `alias` names.
    /// Anything else — a variable, a capability, a builtin — is `None`.
    pub fn resolve(&self, module: &LinkedModule, path: &[&str]) -> Option<Resolution> {
        match path {
            [] => None,
            [name] => {
                let units = module.units();
                if let Some(kind) = units.get(name) {
                    return Some(Resolution::Unit {
                        qualified: qualify(&module.alias, name),
                        kind: *kind,
                    });
                }
                if module.alias == PRELUDE_ALIAS {
                    return None;
                }
                let std = self.module(PRELUDE_ALIAS)?;
                let kind = *std.units().get(name)?;
                is_exported(name).then(|| Resolution::Unit {
                    qualified: qualify(PRELUDE_ALIAS, name),
                    kind,
                })
            }
            [alias, name, ..] => {
                let qualified_alias = module.visible.get(*alias)?;
                let Some(target) = self.module(qualified_alias) else {
                    return Some(Resolution::Opaque);
                };
                let qualified = qualify(qualified_alias, name);
                match target.units().get(name) {
                    None => Some(Resolution::Missing {
                        alias: (*alias).to_string(),
                        name: (*name).to_string(),
                    }),
                    Some(_) if !is_exported(name) => Some(Resolution::Private { qualified }),
                    Some(kind) => Some(Resolution::Unit {
                        qualified,
                        kind: *kind,
                    }),
                }
            }
        }
    }

    /// A program checked on its own, without reading any file: the root and
    /// the prelude, with every `use` alias visible but opaque. This is what
    /// [`crate::check::check`] works from.
    pub fn standalone(program: &Program) -> Self {
        let mut root = root_module(program, None);
        for use_decl in &program.uses {
            root.visible
                .insert(use_decl.alias.name.clone(), use_decl.alias.name.clone());
        }
        Self {
            modules: vec![root, prelude_module()],
        }
    }
}

/// The root program as a module: its own capabilities under their own names.
fn root_module(program: &Program, path: Option<&Path>) -> LinkedModule {
    let capabilities = program
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Needs {
                name,
                kind,
                signatures,
                ..
            } => Some((
                name.name.clone(),
                LinkedCapability {
                    kind: *kind,
                    root_name: name.name.clone(),
                    signatures: signatures.clone(),
                },
            )),
            _ => None,
        })
        .collect();
    LinkedModule {
        alias: String::new(),
        program: program.clone(),
        path: path.map(Path::to_path_buf),
        use_path: None,
        mapping: Vec::new(),
        capabilities,
        visible: BTreeMap::from([(PRELUDE_ALIAS.to_string(), PRELUDE_ALIAS.to_string())]),
        span: program.span,
    }
}

/// The prelude as a module. It parses by construction: the source is the
/// spec's own text and the parser's fixture suite pins it.
fn prelude_module() -> LinkedModule {
    let program = parse(PRELUDE_SOURCE).unwrap_or_else(|diagnostics| {
        panic!("the bundled prelude does not parse: {diagnostics:?}")
    });
    LinkedModule {
        alias: PRELUDE_ALIAS.to_string(),
        span: program.span,
        program,
        path: None,
        use_path: None,
        mapping: Vec::new(),
        capabilities: BTreeMap::new(),
        visible: BTreeMap::new(),
    }
}

/// Resolve every `use` in `program`, transitively, and return the modules with
/// the root first and the prelude last.
///
/// `file` is where the root was read from; relative `use` paths resolve
/// against its directory, or against the current directory when the program
/// came from a string.
///
/// # Errors
///
/// The diagnostics listed in this module's documentation, plus the parser's
/// own for a library that does not parse.
pub fn link(
    program: &Program,
    file: Option<&Path>,
    resolver: &Resolver,
) -> Result<Linked, Vec<Diagnostic>> {
    let mut linker = Linker {
        resolver,
        modules: Vec::new(),
        stack: Vec::new(),
        diagnostics: Vec::new(),
    };
    let root = root_module(program, file);
    if let Some(file) = file {
        linker.stack.push(canonical(file));
    }
    linker.modules.push(root);
    linker.link_uses(0);
    linker.modules.push(prelude_module());
    if linker.diagnostics.is_empty() {
        Ok(Linked {
            modules: linker.modules,
        })
    } else {
        Err(linker.diagnostics)
    }
}

struct Linker<'a> {
    resolver: &'a Resolver,
    modules: Vec<LinkedModule>,
    /// The canonical paths of the files being linked, root first, for
    /// `use_cycle`.
    stack: Vec<PathBuf>,
    diagnostics: Vec<Diagnostic>,
}

/// The one spelling of a file's path, used to detect cycles and to key
/// [`Resolver::overlay`]: symlinks, `.` and `..` resolved. A file that does not
/// exist yet (an editor's unsaved new buffer) is spelled as its canonical
/// directory joined with its name, so it still matches however it is reached.
pub fn canonical(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            std::fs::canonicalize(parent)
                .map(|dir| dir.join(name))
                .unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

impl Linker<'_> {
    /// Link every `use` of `self.modules[index]`, depth first.
    fn link_uses(&mut self, index: usize) {
        let uses = self.modules[index].program.uses.clone();
        for use_decl in &uses {
            if let Some(child) = self.link_one(index, use_decl) {
                let child_index = self.modules.len();
                let child_alias = child.alias.clone();
                let stack_entry = child.path.as_deref().map(canonical);
                self.modules.push(child);
                self.modules[index]
                    .visible
                    .insert(use_decl.alias.name.clone(), child_alias);
                if let Some(entry) = stack_entry {
                    self.stack.push(entry);
                    self.link_uses(child_index);
                    self.stack.pop();
                }
            }
        }
    }

    /// Resolve, read, parse and map one `use` of the importer at `index`.
    fn link_one(&mut self, index: usize, use_decl: &UseDecl) -> Option<LinkedModule> {
        let importer = &self.modules[index];
        let importer_path = importer.path.clone();
        let written = use_decl
            .path
            .as_plain()
            .map(str::to_string)
            .unwrap_or_default();
        let alias = &use_decl.alias.name;

        if use_decl.path.as_plain().is_none() {
            self.report(
                index,
                Diagnostic::error(
                    ErrorCode::UseNotFound,
                    "a `use` path is a plain text literal with no interpolation (spec section 3.9)",
                    use_decl.span,
                ),
            );
            return None;
        }
        if importer.visible.contains_key(alias) {
            self.report(
                index,
                Diagnostic::error(
                    ErrorCode::DuplicateName,
                    format!("module alias `{alias}` is already in use"),
                    use_decl.alias.span,
                ),
            );
            return None;
        }

        let base = importer_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("./<source>"));
        let path = match self.resolver.resolve(&base, &written) {
            Ok(path) => path,
            Err(code) => {
                self.report(
                    index,
                    Diagnostic::error(
                        code,
                        format!("cannot find module `{written}` (spec section 3.9)"),
                        use_decl.span,
                    ),
                );
                return None;
            }
        };
        if self.stack.contains(&canonical(&path)) {
            self.report(
                index,
                Diagnostic::error(
                    ErrorCode::UseCycle,
                    format!("`{written}` is already being linked: modules cannot import each other (spec section 3.9)"),
                    use_decl.span,
                ),
            );
            return None;
        }
        let source = match self.resolver.read(&path) {
            Ok(source) => source,
            Err(error) => {
                self.report(
                    index,
                    Diagnostic::error(
                        ErrorCode::UseNotFound,
                        format!("cannot read module `{written}`: {error}"),
                        use_decl.span,
                    ),
                );
                return None;
            }
        };
        let display = path.display().to_string();
        let program = match parse(&source) {
            Ok(program) => program,
            Err(diagnostics) => {
                for diagnostic in diagnostics {
                    self.diagnostics.push(diagnostic.in_file(display.clone()));
                }
                return None;
            }
        };

        let mut ok = true;
        if let Some(decl) = program
            .decls
            .iter()
            .find(|decl| matches!(decl, Decl::In { .. } | Decl::Out { .. }))
        {
            let what = match decl {
                Decl::In { name, .. } => format!("`in {}`", name.name),
                Decl::Out { name, .. } => format!("`out {}`", name.name),
                Decl::Needs { .. } => unreachable!("filtered above"),
            };
            self.report(
                index,
                Diagnostic::error(
                    ErrorCode::UseHasInputs,
                    format!("`{written}` declares {what}, so it is a program, not a library (spec section 3.9)"),
                    use_decl.span,
                ),
            );
            ok = false;
        }

        // Map the library's `needs` through the `with` clause.
        let importer = &self.modules[index];
        // Raised in the importer, so reported against the importer's file.
        let mut mapping_errors = Vec::new();
        let mut capabilities = BTreeMap::new();
        let mut mapping = Vec::new();
        let mut mapped_inner = BTreeSet::new();
        for entry in &use_decl.mapping {
            mapped_inner.insert(entry.inner.name.as_str());
        }
        for decl in &program.decls {
            let Decl::Needs {
                name,
                kind,
                signatures,
                ..
            } = decl
            else {
                continue;
            };
            let Some(entry) = use_decl
                .mapping
                .iter()
                .find(|entry| entry.inner.name == name.name)
            else {
                mapping_errors.push(Diagnostic::error(
                    ErrorCode::UseNeedsUnmapped,
                    format!(
                        "`{written}` needs `{}` and the `with` clause does not map it (spec section 3.9)",
                        name.name
                    ),
                    use_decl.span,
                ));
                ok = false;
                continue;
            };
            let outer = entry
                .outer
                .as_ref()
                .map_or(&entry.inner.name, |outer| &outer.name);
            let Some(outer_capability) = importer.capabilities.get(outer) else {
                mapping_errors.push(Diagnostic::error(
                    ErrorCode::UseNeedsUnmapped,
                    format!(
                        "`{outer}` is not a capability of the importing program (spec section 3.9)"
                    ),
                    entry.span,
                ));
                ok = false;
                continue;
            };
            if outer_capability.kind != *kind {
                mapping_errors.push(Diagnostic::error(
                    ErrorCode::UseNeedsUnmapped,
                    format!(
                        "`{written}` needs `{}` as {} but `{outer}` is {} (spec section 3.9)",
                        name.name,
                        kind_name(*kind),
                        kind_name(outer_capability.kind)
                    ),
                    entry.span,
                ));
                ok = false;
                continue;
            }
            let signatures = if signatures.is_empty() {
                outer_capability.signatures.clone()
            } else {
                signatures.clone()
            };
            capabilities.insert(
                name.name.clone(),
                LinkedCapability {
                    kind: *kind,
                    root_name: outer_capability.root_name.clone(),
                    signatures,
                },
            );
            mapping.push((name.name.clone(), outer.clone(), *kind));
        }
        for entry in &use_decl.mapping {
            let declared = program.decls.iter().any(
                |decl| matches!(decl, Decl::Needs { name, .. } if name.name == entry.inner.name),
            );
            if !declared {
                mapping_errors.push(Diagnostic::error(
                    ErrorCode::UseNeedsUnmapped,
                    format!(
                        "`{written}` declares no `needs {}` for the `with` clause to bind (spec section 3.9)",
                        entry.inner.name
                    ),
                    entry.span,
                ));
                ok = false;
            }
        }
        for error in mapping_errors {
            self.report(index, error);
        }
        if !ok {
            return None;
        }
        Some(LinkedModule {
            alias: qualify(&self.modules[index].alias, alias),
            program,
            path: Some(path),
            use_path: Some(written),
            mapping,
            capabilities,
            visible: BTreeMap::from([(PRELUDE_ALIAS.to_string(), PRELUDE_ALIAS.to_string())]),
            span: use_decl.span,
        })
    }

    /// Record a diagnostic raised in the module at `index`, with that module's
    /// file at the front when it is not the root.
    fn report(&mut self, index: usize, diagnostic: Diagnostic) {
        let located = self.modules[index].locate(diagnostic);
        self.diagnostics.push(located);
    }
}

/// The kind as the spec writes it.
pub fn kind_name(kind: CapabilityKind) -> &'static str {
    match kind {
        CapabilityKind::Agent => "agent",
        CapabilityKind::Person => "person",
        CapabilityKind::Llm => "llm",
        CapabilityKind::Tool => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underscore_names_are_private_to_their_file() {
        assert!(is_exported("read_agent"));
        assert!(!is_exported("_scratch"));
    }

    #[test]
    fn units_are_qualified_by_their_alias() {
        assert_eq!(qualify("harness", "read_agent"), "harness.read_agent");
        assert_eq!(qualify("", "main"), "main");
        assert_eq!(qualify(PRELUDE_ALIAS, "stuck"), "std.stuck");
    }

    #[test]
    fn a_relative_path_is_never_looked_up_in_the_roots() {
        let resolver = Resolver {
            roots: vec![PathBuf::from("/nowhere")],
            ..Resolver::default()
        };
        let error = resolver
            .resolve(Path::new("examples/fix_issue.jev"), "./lib/missing.jev")
            .expect_err("a relative path that does not exist is not found");
        assert_eq!(error, ErrorCode::UseNotFound);
    }

    #[test]
    fn the_prelude_exports_its_three_defs() {
        // Spec sections 7.3 and 8.1 name exactly these.
        let linked = Linked::standalone(&parse("program t\n\ntask main:\n  x = 1\n").unwrap());
        assert_eq!(
            linked.prelude_names(),
            BTreeSet::from(["focus_impl", "stuck", "repeats"])
        );
    }

    #[test]
    fn a_bare_prelude_name_resolves_to_std_unless_shadowed() {
        let linked = Linked::standalone(
            &parse("program t\n\ndef stuck(x):\n  return x\n\ntask main:\n  x = 1\n").unwrap(),
        );
        let root = linked.root();
        assert_eq!(
            linked.resolve(root, &["stuck"]),
            Some(Resolution::Unit {
                qualified: "stuck".to_string(),
                kind: UnitKind::Def
            })
        );
        assert_eq!(
            linked.resolve(root, &["repeats"]),
            Some(Resolution::Unit {
                qualified: "std.repeats".to_string(),
                kind: UnitKind::Def
            })
        );
        assert_eq!(
            linked.resolve(root, &["std", "stuck"]),
            Some(Resolution::Unit {
                qualified: "std.stuck".to_string(),
                kind: UnitKind::Def
            })
        );
        assert_eq!(linked.resolve(root, &["x"]), None);
    }
}
