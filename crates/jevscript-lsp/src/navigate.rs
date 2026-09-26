//! Hover, go-to-definition and find-references.
//!
//! All three start from the [`Occurrence`](crate::index::Occurrence) under the
//! cursor and resolve it through [`World::resolve`], so a name means the same
//! thing to each of them, across `use` boundaries included.

use std::rc::Rc;

use lsp_types::{Hover, HoverContents, Location, MarkupContent, MarkupKind, Range};

use jevscript_compiler::link::UnitKind;
use jevscript_syntax::ast::CapabilityKind;
use jevscript_syntax::{Keyword, TokenKind};

use crate::docs;
use crate::index::{SymbolKind, Target};
use crate::world::{Resolved, SourceFile, World};

/// Hover at byte `offset` of `file`.
pub fn hover(world: &mut World, file: &Rc<SourceFile>, offset: usize) -> Option<Hover> {
    let occurrence = file
        .index
        .as_ref()
        .and_then(|index| index.occurrence_at(offset))
        .cloned();
    if let Some(occurrence) = occurrence {
        let text = match &occurrence.target {
            Target::Verb { kind, verb } => docs::verb(*kind, verb).map(|d| docs::markdown(&d)),
            Target::HandleVerb(verb) => docs::handle_verb(verb).map(|d| docs::markdown(&d)),
            Target::Builtin(name) => docs::builtin(name).map(|d| docs::markdown(&d)),
            target => world
                .resolve(file, target)
                .map(|resolved| describe(world, file, &resolved)),
        };
        if let Some(text) = text {
            return Some(markdown(
                text,
                file.lines.range(&file.text, occurrence.span),
            ));
        }
    }
    // The `log` word and its level (spec section 5.8).
    if let Some(index) = &file.index {
        for (log, level) in &index.logs {
            let within = |s: jevscript_syntax::Span| {
                s.start.offset as usize <= offset && offset <= s.end.offset as usize
            };
            let doc = if within(*log) {
                docs::keyword("log").map(|d| (d, *log))
            } else if within(*level) {
                let word = file
                    .text
                    .get(level.start.offset as usize..level.end.offset as usize)
                    .unwrap_or_default();
                docs::log_level(word).map(|d| (d, *level))
            } else {
                None
            };
            if let Some((doc, span)) = doc {
                return Some(markdown(
                    docs::markdown(&doc),
                    file.lines.range(&file.text, span),
                ));
            }
        }
    }
    // A reserved or contextual word, or a capability kind.
    let token =
        file.recovered.lexed.tokens.iter().find(|t| {
            t.span.start.offset as usize <= offset && offset <= t.span.end.offset as usize
        })?;
    let word = match &token.kind {
        TokenKind::Keyword(kw) => kw.as_str(),
        TokenKind::Name(name) => name.as_str(),
        _ => return None,
    };
    let doc = match word {
        "agent" => Some(docs::kind(CapabilityKind::Agent)),
        "person" => Some(docs::kind(CapabilityKind::Person)),
        "llm" => Some(docs::kind(CapabilityKind::Llm)),
        "tool" => Some(docs::kind(CapabilityKind::Tool)),
        _ => None,
    }
    .filter(|_| after_needs(file, token.span.start.offset as usize))
    .or_else(|| {
        let keyword = matches!(token.kind, TokenKind::Keyword(_));
        (keyword || !known_name(file, token.span.start.offset as usize))
            .then(|| docs::keyword(word))
            .flatten()
    })?;
    Some(markdown(
        docs::markdown(&doc),
        file.lines.range(&file.text, token.span),
    ))
}

/// Whether the token at `offset` follows `needs <name>:`.
fn after_needs(file: &SourceFile, offset: usize) -> bool {
    let tokens = &file.recovered.lexed.tokens;
    let Some(at) = tokens
        .iter()
        .position(|t| t.span.start.offset as usize == offset)
    else {
        return false;
    };
    at >= 3 && tokens[at - 3].kind == TokenKind::Keyword(Keyword::Needs)
}

/// Whether the index knows the name at `offset` as something other than a
/// contextual keyword.
fn known_name(file: &SourceFile, offset: usize) -> bool {
    file.index.as_ref().is_some_and(|index| {
        index
            .occurrence_at(offset)
            .is_some_and(|o| o.target != Target::None)
    })
}

fn markdown(value: String, range: Range) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    }
}

/// The comment lines directly above a declaration, as its documentation.
fn doc_comment(file: &SourceFile, start: usize) -> Option<String> {
    let before = file.text.get(..start)?;
    let mut lines: Vec<&str> = Vec::new();
    // The declaration's own line may be indented; skip back to its start.
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    for line in file.text[..line_start].lines().rev() {
        let trimmed = line.trim_start();
        match trimmed.strip_prefix('#') {
            Some(comment) => lines.push(comment.strip_prefix(' ').unwrap_or(comment)),
            None => break,
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
}

/// Hover markdown for a declaration.
fn describe(world: &mut World, from: &Rc<SourceFile>, resolved: &Resolved) -> String {
    let file = &resolved.file;
    let index = file.index.as_ref().expect("resolved in an indexed file");
    let symbol = resolved.symbol();
    let mut out = format!("```jevscript\n{}\n```\n", symbol.detail);
    let (what, section) = match symbol.kind {
        SymbolKind::Program => ("program", "3.1"),
        SymbolKind::Module => ("module", "3.9"),
        SymbolKind::Input | SymbolKind::InputField => ("input", "3.2"),
        SymbolKind::Output => ("output", "3.3"),
        SymbolKind::Capability(_) => ("capability", "3.4"),
        SymbolKind::ToolVerb => ("tool verb", "9.4"),
        SymbolKind::Unit(UnitKind::Judgment) => ("judgment", "6.7"),
        SymbolKind::Unit(UnitKind::Task) => ("task", "7"),
        SymbolKind::Unit(UnitKind::Def) => ("def", "8"),
        SymbolKind::Unit(UnitKind::Machine) => ("machine", "7.8"),
        SymbolKind::Parameter => ("parameter", "3.6"),
        SymbolKind::Result => ("judgment result", "6.7"),
        SymbolKind::Local => ("variable", "5.3"),
        SymbolKind::ShapeField => ("shaped field", "7.2"),
        SymbolKind::State => ("machine state", "7.8"),
        SymbolKind::Event => ("machine event", "7.8"),
        SymbolKind::Label => ("label", "6.3"),
    };
    let place = if file.is_prelude() {
        " in the prelude `std`".to_string()
    } else if file.key() != from.key() {
        let shown = file.path.as_ref().map_or_else(
            || file.uri.as_str().to_string(),
            |p| p.display().to_string(),
        );
        format!(" in `{shown}`")
    } else {
        String::new()
    };
    out.push_str(&format!("*{what}*{place} — spec section {section}\n"));
    if let Some(comment) = doc_comment(file, symbol.full.start.offset as usize) {
        out.push_str(&format!("\n{comment}\n"));
    }

    let children = |kind: SymbolKind| -> Vec<&crate::index::Symbol> {
        index
            .children(Some(resolved.symbol))
            .into_iter()
            .map(|id| &index.symbols[id])
            .filter(|s| s.kind == kind)
            .collect()
    };
    match symbol.kind {
        SymbolKind::Unit(UnitKind::Judgment) => {
            let results = children(SymbolKind::Result);
            if !results.is_empty() {
                out.push_str("\nReturns a record of:\n");
                for result in results {
                    out.push_str(&format!("- `{}`\n", result.detail));
                }
            }
        }
        SymbolKind::Unit(UnitKind::Machine) => {
            out.push_str("\nStates:\n");
            for state in children(SymbolKind::State) {
                out.push_str(&format!("- `{}`\n", state.detail));
            }
            out.push_str(
                "\nA call returns `{ state, steps, done, verified, events }` (spec section 7.8).\n",
            );
        }
        SymbolKind::Unit(_) if file.is_prelude() => {
            let start = symbol.full.start.offset as usize;
            let end = symbol.full.end.offset as usize;
            if let Some(source) = file.text.get(start..end) {
                out.push_str(&format!("\n```jevscript\n{}\n```\n", source.trim_end()));
            }
        }
        SymbolKind::Capability(kind) => {
            let doc = docs::kind(kind);
            out.push_str(&format!("\n{}\n", doc.summary));
            let declared = children(SymbolKind::ToolVerb);
            if !declared.is_empty() {
                out.push_str("\nDeclared verbs:\n");
                for verb in declared {
                    out.push_str(&format!("- `{}`\n", verb.detail));
                }
            } else if kind == CapabilityKind::Tool {
                out.push_str("\nAn open tool: no signature block, so verbs are checked at bind time or at the call (spec section 9.4).\n");
            }
        }
        SymbolKind::Result => {
            let labels = children(SymbolKind::Label);
            if !labels.is_empty() {
                out.push_str("\nLabels:\n");
                for label in labels {
                    out.push_str(&format!("- `{}`\n", label.detail));
                }
            }
        }
        SymbolKind::Module => {
            let written = index
                .uses
                .iter()
                .find(|u| u.symbol == resolved.symbol)
                .and_then(|u| u.path.clone());
            if let Some(written) = written
                && let Some(target) = world.use_target(file, &written)
                && let Some(path) = &target.path
            {
                out.push_str(&format!("\nResolves to `{}`.\n", path.display()));
            }
        }
        _ => {}
    }
    out
}

/// Where the name at byte `offset` is declared.
pub fn definition(world: &mut World, file: &Rc<SourceFile>, offset: usize) -> Vec<Location> {
    let Some(occurrence) = file
        .index
        .as_ref()
        .and_then(|index| index.occurrence_at(offset))
        .cloned()
    else {
        return Vec::new();
    };
    let Some(resolved) = world.resolve(file, &occurrence.target) else {
        return Vec::new();
    };
    if resolved.file.is_prelude() {
        return Vec::new();
    }
    if let Target::UsePath(_) = occurrence.target {
        return vec![Location::new(resolved.file.uri.clone(), Range::default())];
    }
    resolved
        .file
        .location(resolved.symbol)
        .into_iter()
        .collect()
}

/// Every place the name at byte `offset` refers to the same declaration.
pub fn references(
    world: &mut World,
    file: &Rc<SourceFile>,
    offset: usize,
    include_declaration: bool,
) -> Vec<Location> {
    let Some(occurrence) = file
        .index
        .as_ref()
        .and_then(|index| index.occurrence_at(offset))
        .cloned()
    else {
        return Vec::new();
    };
    let Some(wanted) = world.resolve(file, &occurrence.target) else {
        return Vec::new();
    };
    // Only what a module exports can be named from another file (3.9).
    let exported = matches!(
        wanted.symbol().kind,
        SymbolKind::Unit(_)
            | SymbolKind::Result
            | SymbolKind::Label
            | SymbolKind::Parameter
            | SymbolKind::Capability(_)
            | SymbolKind::Module
    );
    let files = if exported {
        world.search_set()
    } else {
        vec![wanted.file.clone()]
    };
    let mut locations = Vec::new();
    for candidate in files {
        let Some(index) = &candidate.index else {
            continue;
        };
        for occurrence in &index.occurrences {
            if occurrence.target == Target::None {
                continue;
            }
            if occurrence.declaration && !include_declaration {
                continue;
            }
            let Some(found) = world.resolve(&candidate, &occurrence.target) else {
                continue;
            };
            if found.same(&wanted) {
                locations.push(Location::new(
                    candidate.uri.clone(),
                    candidate.lines.range(&candidate.text, occurrence.span),
                ));
            }
        }
    }
    locations.sort_by(|a, b| (a.uri.as_str(), a.range.start).cmp(&(b.uri.as_str(), b.range.start)));
    locations.dedup();
    locations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_index::Encoding;
    use crate::world::{canonical, uri_from_path};
    use jevscript_compiler::Resolver;
    use std::path::{Path, PathBuf};

    fn examples() -> PathBuf {
        canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"))
    }

    fn open(world: &mut World, name: &str) -> Rc<SourceFile> {
        let path = examples().join(name);
        let text = std::fs::read_to_string(&path).expect("reads");
        let uri = uri_from_path(&path);
        world.open(uri.clone(), text, 1);
        world.file(&uri).expect("open")
    }

    fn offset(file: &SourceFile, needle: &str) -> usize {
        file.text
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}`"))
    }

    fn hover_text(world: &mut World, file: &Rc<SourceFile>, needle: &str) -> String {
        let at = offset(file, needle);
        let hover = hover(world, file, at).unwrap_or_else(|| panic!("a hover at `{needle}`"));
        match hover.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn go_to_definition_crosses_a_use() {
        // Spec 3.9: `harness.watch` is defined in the imported file.
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let root = open(&mut world, "fix_issue.jev");
        let found = definition(&mut world, &root, offset(&root, "watch(dev"));
        assert_eq!(found.len(), 1);
        assert!(found[0].uri.as_str().ends_with("lib/agent_loop.jev"));
        assert_eq!(found[0].range.start.line, 15);
        let path = definition(&mut world, &root, offset(&root, "./lib/agent_loop.jev"));
        assert!(path[0].uri.as_str().ends_with("lib/agent_loop.jev"));
    }

    #[test]
    fn go_to_definition_on_a_result_and_a_label() {
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let file = open(&mut world, "fix_issue_inline.jev");
        let result = definition(&mut world, &file, offset(&file, "claims_done >"));
        assert_eq!(result[0].range.start.line, 10);
        let label = definition(&mut world, &file, offset(&file, "needs_me:"));
        assert_eq!(label[0].range.start.line, 16);
    }

    #[test]
    fn references_to_a_library_unit_include_its_importers() {
        let mut world = World::new(Encoding::Utf16, vec![examples()], Resolver::default());
        let library = open(&mut world, "lib/agent_loop.jev");
        let found = references(&mut world, &library, offset(&library, "watch(dev"), true);
        let files: Vec<&str> = found.iter().map(|l| l.uri.as_str()).collect();
        assert!(
            files.iter().any(|f| f.ends_with("lib/agent_loop.jev")),
            "{files:?}"
        );
        assert!(
            files.iter().any(|f| f.ends_with("/fix_issue.jev")),
            "{files:?}"
        );
        let without = references(&mut world, &library, offset(&library, "watch(dev"), false);
        assert_eq!(without.len(), found.len() - 1);
    }

    #[test]
    fn references_to_a_local_stay_in_its_unit() {
        let mut world = World::new(Encoding::Utf16, vec![examples()], Resolver::default());
        let file = open(&mut world, "fix_issue_inline.jev");
        let found = references(&mut world, &file, offset(&file, "obs = shape"), true);
        // The shape, the judgment call, `stuck(obs.recent)`, `obs.summary`
        // and the `{obs.tests}` interpolation.
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(found.iter().all(|l| l.uri == file.uri));
    }

    #[test]
    fn hover_shows_signatures_and_spec_docs() {
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let file = open(&mut world, "fix_issue_inline.jev");
        let unit = hover_text(&mut world, &file, "read_agent obs");
        assert!(
            unit.contains("judgment read_agent(summary, files, tests, recent)"),
            "{unit}"
        );
        assert!(unit.contains("next        = summary pick"), "{unit}");
        let keyword = hover_text(&mut world, &file, "feels");
        assert!(keyword.contains("Spec section 6.2"), "{keyword}");
        let builtin = hover_text(&mut world, &file, "count(");
        assert!(builtin.contains("count(probs, above p)"), "{builtin}");
        let verb = hover_text(&mut world, &file, "spawn");
        assert!(verb.contains("returns its `handle`"), "{verb}");
        let prelude = hover_text(&mut world, &file, "stuck(obs");
        assert!(prelude.contains("def stuck(steps)"), "{prelude}");
        assert!(prelude.contains("repeats(steps) >= 2"), "{prelude}");
        let kind = hover_text(&mut world, &file, "agent\n");
        assert!(kind.contains("Spec section 9.1"), "{kind}");
        let capability = hover_text(&mut world, &file, "tree.create");
        assert!(capability.contains("needs tree:   tool"), "{capability}");
    }

    #[test]
    fn hover_across_a_use_names_the_library() {
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let file = open(&mut world, "fix_issue.jev");
        let text = hover_text(&mut world, &file, "watch(dev");
        assert!(text.contains("task watch(dev, title)"), "{text}");
        assert!(text.contains("agent_loop.jev"), "{text}");
        let alias = hover_text(&mut world, &file, "harness with");
        assert!(alias.contains("Resolves to"), "{alias}");
    }

    #[test]
    fn hover_distinguishes_log_forms_from_a_unit_named_log() {
        // Spec 5.8: `log` and its level are contextual; a local unit named
        // `log` keeps the older command form.
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let path = examples().join("log_hover.jev");
        let uri = uri_from_path(&path);
        world.open(
            uri.clone(),
            "program demo\ntask main:\n  log info 1\n  x = log debug 2\n".into(),
            1,
        );
        let file = world.file(&uri).expect("open");
        let statement = hover_text(&mut world, &file, "log info");
        assert!(statement.contains("Spec section 5.8"), "{statement}");
        let level = hover_text(&mut world, &file, "info 1");
        assert!(level.contains("Spec section 5.8"), "{level}");
        let expression = hover_text(&mut world, &file, "log debug");
        assert!(expression.contains("Spec section 5.8"), "{expression}");

        world.open(
            uri.clone(),
            "program demo\ndef log(info):\n  return info\ntask main:\n  log info 1\n".into(),
            2,
        );
        let file = world.file(&uri).expect("reopen");
        let call = hover_text(&mut world, &file, "log info");
        assert!(call.contains("def log(info)"), "{call}");
    }
}
