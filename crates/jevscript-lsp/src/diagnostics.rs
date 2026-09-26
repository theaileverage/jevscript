//! Compile diagnostics, as the editor shows them.
//!
//! A buffer that does not lex or parse reports every syntax error the
//! recovering parse found (see [`crate::recover`]) and nothing else: running
//! the whole-program checks over a partial program would report the missing
//! declarations as errors of their own. A buffer that parses is compiled
//! exactly as `jevscript check` compiles it — linking, checks, lowering —
//! with its own path, so relative `use` paths resolve as they would on disk,
//! and with every open buffer overlaid on its file
//! ([`World::linking_resolver`]), so an import is linked as it is being
//! edited and a library's error is placed in the text the editor shows.
//!
//! Each diagnostic carries what spec section 12 asks a renderer for: the
//! stable code, a link to its entry in `docs/error-reference.md`, the spec
//! section the rule lives in, and the related location when there is one — a
//! duplicate's first declaration, or the place inside a library where a
//! library's own error sits (its errors are reported on the `use` line of the
//! importing file, with the library location attached).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;

use lsp_types::{
    CodeDescription, Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    Location, NumberOrString, Range, Uri,
};

use jevscript_compiler::analyze;
use jevscript_syntax::{Diagnostic, ErrorCode, Severity};

use crate::docs::code_section;
use crate::index::SymbolKind;
use crate::world::{SourceFile, World, canonical};

/// Where the error reference is published. `#<code>` is appended; spec
/// section 12 makes that anchor stable.
pub const DEFAULT_ERROR_REFERENCE: &str =
    "https://github.com/theaileverage/jevscript/blob/main/docs/error-reference.md";

/// The `source` every diagnostic carries.
pub const SOURCE: &str = "jevscript";

/// Every diagnostic for one file.
pub fn diagnostics(
    world: &mut World,
    file: &Rc<SourceFile>,
    reference: &str,
) -> Vec<LspDiagnostic> {
    if !file.recovered.errors.is_empty() {
        return file
            .recovered
            .errors
            .iter()
            .map(|d| convert(file, d, reference, Vec::new()))
            .collect();
    }
    let Some(program) = &file.recovered.program else {
        return Vec::new();
    };
    let compilation = analyze(program, file.path.as_deref(), world.linking_resolver());
    let root = file.path.as_deref().map(canonical);
    compilation
        .diagnostics
        .iter()
        .map(|d| {
            let elsewhere = d
                .file
                .as_deref()
                .map(|f| canonical(Path::new(f)))
                .filter(|f| Some(f) != root.as_ref());
            match elsewhere {
                None => {
                    let related = related_here(file, d);
                    convert(file, d, reference, related)
                }
                Some(library) => in_library(world, file, d, &library, reference),
            }
        })
        .collect()
}

/// The LSP form of a diagnostic located in `file`.
fn convert(
    file: &SourceFile,
    diagnostic: &Diagnostic,
    reference: &str,
    related: Vec<DiagnosticRelatedInformation>,
) -> LspDiagnostic {
    let range = file.lines.range(&file.text, diagnostic.span);
    lsp_diagnostic(
        diagnostic,
        range,
        diagnostic.message.clone(),
        reference,
        related,
    )
}

fn lsp_diagnostic(
    diagnostic: &Diagnostic,
    range: Range,
    message: String,
    reference: &str,
    related: Vec<DiagnosticRelatedInformation>,
) -> LspDiagnostic {
    let code = diagnostic.code;
    let section = code_section(code);
    let message = if message.contains("spec section") {
        message
    } else {
        format!("{message} (spec section {section})")
    };
    LspDiagnostic {
        range,
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(code.as_str().to_string())),
        code_description: Uri::from_str(&format!("{reference}#{}", code.as_str()))
            .ok()
            .map(|href| CodeDescription { href }),
        source: Some(SOURCE.to_string()),
        message,
        related_information: (!related.is_empty()).then_some(related),
        tags: None,
        data: Some(serde_json::json!({ "specSection": section })),
    }
}

/// The related location of a diagnostic raised in this file: the first
/// declaration of a `duplicate_name` (spec section 12 asks for it).
fn related_here(file: &SourceFile, diagnostic: &Diagnostic) -> Vec<DiagnosticRelatedInformation> {
    if diagnostic.code != ErrorCode::DuplicateName {
        return Vec::new();
    }
    let Some(index) = &file.index else {
        return Vec::new();
    };
    let Some(name) = backticked(&diagnostic.message) else {
        return Vec::new();
    };
    let at = diagnostic.span.start.offset;
    let first = index
        .symbols
        .iter()
        .filter(|s| s.name == name && s.selection.start.offset < at)
        .filter(|s| !matches!(s.kind, SymbolKind::Local | SymbolKind::Parameter))
        .min_by_key(|s| s.selection.start.offset);
    first
        .map(|symbol| DiagnosticRelatedInformation {
            location: Location::new(
                file.uri.clone(),
                file.lines.range(&file.text, symbol.selection),
            ),
            message: format!("`{name}` is first declared here"),
        })
        .into_iter()
        .collect()
}

/// A diagnostic whose file is a library `file` imports: shown on the `use`
/// line that brings the library in, with the library location related.
fn in_library(
    world: &mut World,
    file: &Rc<SourceFile>,
    diagnostic: &Diagnostic,
    library: &Path,
    reference: &str,
) -> LspDiagnostic {
    let uses = file
        .index
        .as_ref()
        .map(|index| index.uses.clone())
        .unwrap_or_default();
    let mut anchor = uses.first().map(|u| u.span);
    for use_info in &uses {
        let Some(written) = &use_info.path else {
            continue;
        };
        if reaches(world, file, written, library, 0) {
            anchor = Some(use_info.span);
            break;
        }
    }
    let range = anchor.map_or_else(Range::default, |span| file.lines.range(&file.text, span));
    let shown = diagnostic
        .file
        .clone()
        .unwrap_or_else(|| library.display().to_string());
    let related = world
        .file_at(library)
        .map(|lib| DiagnosticRelatedInformation {
            location: Location::new(lib.uri.clone(), lib.lines.range(&lib.text, diagnostic.span)),
            message: diagnostic.message.clone(),
        })
        .into_iter()
        .collect();
    lsp_diagnostic(
        diagnostic,
        range,
        format!("in `{shown}`: {}", diagnostic.message),
        reference,
        related,
    )
}

/// Whether the module `written` from `from` is `library` or imports it.
fn reaches(
    world: &mut World,
    from: &Rc<SourceFile>,
    written: &str,
    library: &Path,
    depth: usize,
) -> bool {
    if depth > 16 {
        return false;
    }
    let Some(target) = world.use_target(from, written) else {
        // An unresolvable path is where `use_not_found` belongs.
        return canonical(&resolve_lexically(from, written)) == library;
    };
    if target.path.as_deref().map(canonical).as_deref() == Some(library) {
        return true;
    }
    let nested: Vec<String> = target
        .index
        .as_ref()
        .map(|i| i.uses.iter().filter_map(|u| u.path.clone()).collect())
        .unwrap_or_default();
    nested
        .iter()
        .any(|next| reaches(world, &target, next, library, depth + 1))
}

fn resolve_lexically(from: &SourceFile, written: &str) -> PathBuf {
    from.path
        .as_deref()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from(written), |dir| dir.join(written))
}

/// The first `name` in backticks in a message.
fn backticked(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let len = message[start..].find('`')?;
    Some(&message[start..start + len])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_index::Encoding;
    use crate::world::uri_from_path;
    use jevscript_compiler::Resolver;

    fn check(world: &mut World, uri: &Uri) -> Vec<LspDiagnostic> {
        let file = world.file(uri).expect("open");
        diagnostics(world, &file, DEFAULT_ERROR_REFERENCE)
    }

    fn world() -> World {
        World::new(Encoding::Utf16, vec![], Resolver::default())
    }

    fn open(world: &mut World, name: &str, text: &str) -> Uri {
        let uri = Uri::from_str(&format!("file:///virtual/{name}")).expect("parses");
        world.open(uri.clone(), text.to_string(), 1);
        uri
    }

    #[test]
    fn an_error_carries_its_code_link_and_section() {
        // Spec 12: the stable code, a link to its documentation.
        let mut world = world();
        let uri = open(
            &mut world,
            "a.jev",
            "program demo\n\ntask main:\n  loop:\n    x = 1\n",
        );
        let found = check(&mut world, &uri);
        assert_eq!(found.len(), 1, "{found:?}");
        let d = &found[0];
        assert_eq!(
            d.code,
            Some(NumberOrString::String("unbounded_loop".into()))
        );
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            d.code_description.as_ref().map(|c| c.href.as_str()),
            Some(format!("{DEFAULT_ERROR_REFERENCE}#unbounded_loop").as_str())
        );
        assert!(d.message.ends_with("(spec section 5.5)"), "{}", d.message);
        assert_eq!(d.range.start.line, 3);
    }

    #[test]
    fn a_warning_is_a_warning() {
        // Spec 7.2: an uncapped shape field is `uncapped_field`, a warning.
        let mut world = world();
        let uri = open(
            &mut world,
            "w.jev",
            "program demo\nin notes: text\n\ntask main:\n  s = shape:\n    body notes\n  x = s\n",
        );
        let found = check(&mut world, &uri);
        let warning = found
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("uncapped_field".into())))
            .unwrap_or_else(|| panic!("{found:?}"));
        assert_eq!(warning.severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn every_broken_declaration_is_reported_and_nothing_else() {
        let mut world = world();
        let uri = open(
            &mut world,
            "b.jev",
            "program demo\n\ndef a(x):\n  return (\n\ndef b(y):\n  return ]\n\ntask main:\n  z = a(1)\n",
        );
        let found = check(&mut world, &uri);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found
                .iter()
                .all(|d| d.code == Some(NumberOrString::String("syntax".into())))
        );
    }

    #[test]
    fn a_duplicate_points_at_its_first_declaration() {
        let mut world = world();
        let uri = open(
            &mut world,
            "d.jev",
            "program demo\n\ndef twice(x):\n  return x\n\ndef twice(y):\n  return y\n",
        );
        let found = check(&mut world, &uri);
        let duplicate = found
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("duplicate_name".into())))
            .unwrap_or_else(|| panic!("{found:?}"));
        let related = duplicate.related_information.as_ref().expect("related");
        assert_eq!(related[0].location.range.start.line, 2);
    }

    #[test]
    fn a_library_error_is_shown_on_the_use_line_with_the_library_related() {
        let dir = std::env::temp_dir().join(format!("jevscript-lsp-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("lib.jev"),
            "program lib\n\ndef helper(x):\n  loop max 2:\n    break\n  return y\n",
        )
        .expect("writes");
        let root = dir.join("main.jev");
        let text = "program main\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = lib.helper(1)\n";
        std::fs::write(&root, text).expect("writes");
        let mut world = world();
        let uri = uri_from_path(&root);
        world.open(uri.clone(), text.to_string(), 1);
        let found = check(&mut world, &uri);
        let _ = std::fs::remove_dir_all(&dir);
        let d = found
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("unassigned_read".into())))
            .unwrap_or_else(|| panic!("{found:?}"));
        assert_eq!(d.range.start.line, 2, "on the `use` line");
        assert!(d.message.starts_with("in `"), "{}", d.message);
        let related = d.related_information.as_ref().expect("related");
        assert!(related[0].location.uri.as_str().ends_with("lib.jev"));
        assert_eq!(related[0].location.range.start.line, 5);
    }

    #[test]
    fn the_shipped_examples_have_no_errors() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        for name in [
            "fix_issue.jev",
            "fix_issue_inline.jev",
            "review_loop.jev",
            "chief_of_staff.jev",
            "inbox_triage.jev",
            "lib/agent_loop.jev",
        ] {
            let path = canonical(&examples.join(name));
            let mut world = world();
            let uri = uri_from_path(&path);
            let text = std::fs::read_to_string(&path).expect("reads");
            world.open(uri.clone(), text, 1);
            let errors: Vec<_> = check(&mut world, &uri)
                .into_iter()
                .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                .collect();
            assert!(errors.is_empty(), "{name}: {errors:?}");
        }
    }
}
