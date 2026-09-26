//! Completion.
//!
//! What is offered depends on where the cursor is, read from the text of the
//! line being typed:
//!
//! - after `<path>.`: the members of what the path holds — a module's units
//!   (3.9), a capability's verbs (9), a judgment call's results (6.7), a
//!   shaped record's fields (7.2), a handle's verbs (9.1), the fields of a
//!   `choice`, `level` or machine result (6.3, 6.4, 7.8);
//! - after `is`: the labels the compared value was declared with (4.3);
//! - after `->` on an `on` line: the machine's states (7.8);
//! - after `needs <name>:`: the four capability kinds (3.4);
//! - at the start of a top-level line: the declarations of section 3;
//! - anywhere else: the names in scope and the keywords.
//!
//! The line being typed rarely parses, so the scope is read from the buffer
//! as it would be with that one line replaced by an assignment that does.

use std::collections::BTreeSet;
use std::rc::Rc;

use lsp_types::{CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind};

use jevscript_compiler::check::BUILTINS;
use jevscript_compiler::link::{PRELUDE_ALIAS, UnitKind};
use jevscript_syntax::ast::CapabilityKind;

use crate::docs;
use crate::index::{Binding, Index, Source, SymbolId, SymbolKind, kind_verbs};
use crate::world::{SourceFile, World};

/// The words that open a top-level line (spec section 3).
const TOP_LEVEL: [&str; 9] = [
    "program", "use", "in", "out", "needs", "judgment", "task", "def", "machine",
];

/// The words a statement or expression may use.
const BODY_KEYWORDS: [&str; 35] = [
    "if", "elif", "else", "for", "in", "loop", "until", "max", "return", "stop", "escalate",
    "continue", "break", "gate", "proceed", "confirm", "and", "or", "not", "is", "true", "false",
    "none", "each", "feels", "pick", "rate", "among", "other", "shape", "focus", "trail", "on",
    "verify", "log",
];

/// Completion items at byte `offset` of `file`.
pub fn complete(world: &mut World, file: &Rc<SourceFile>, offset: usize) -> Vec<CompletionItem> {
    let offset = offset.min(file.text.len());
    let line_start = file.text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &file.text[line_start..offset];
    let word_len = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .count();
    let before = &prefix[..prefix.len() - word_len];
    let indent = prefix.len() - prefix.trim_start().len();

    if before.trim().is_empty() && indent == 0 {
        return TOP_LEVEL.iter().map(|w| keyword_item(w)).collect();
    }
    let scoped = scoped(world, file, offset, line_start);
    let index = scoped.index.as_ref();
    let scope = index.and_then(|index| {
        let at = line_start + indent.min(scoped.text.len().saturating_sub(line_start));
        index.scope_at(at).map(|s| (index, s))
    });

    if let Some(path) = before.strip_suffix('.').map(trailing_path) {
        if path.is_empty() {
            return Vec::new();
        }
        return members(world, &scoped, scope.map(|(_, s)| s), &path);
    }
    let trimmed = before.trim_end();
    // `log <level>` (spec section 5.8).
    let last_word = trimmed
        .rsplit(|c: char| c.is_whitespace() || "(=[,{:".contains(c))
        .next();
    if before.ends_with(' ')
        && last_word == Some("log")
        && !index.is_some_and(|index| index.units.contains_key("log"))
    {
        return ["debug", "info", "warn", "error"]
            .into_iter()
            .filter_map(|level| {
                docs::log_level(level).map(|d| {
                    item(
                        level,
                        CompletionItemKind::ENUM_MEMBER,
                        Some(d.signature.into()),
                        Some(d.summary.into()),
                    )
                })
            })
            .collect();
    }
    if let Some(compared) = trimmed.strip_suffix(" is") {
        return labels(
            world,
            &scoped,
            scope.map(|(_, s)| s),
            &trailing_path(compared.trim_end()),
        );
    }
    if trimmed.trim_start().starts_with("on ") && trimmed.ends_with("->") {
        return states(&file.text, line_start);
    }
    if trimmed.starts_with("needs ") && trimmed.ends_with(':') {
        return [
            CapabilityKind::Agent,
            CapabilityKind::Person,
            CapabilityKind::Llm,
            CapabilityKind::Tool,
        ]
        .into_iter()
        .map(|kind| {
            let doc = docs::kind(kind);
            item(
                doc.signature,
                CompletionItemKind::ENUM,
                None,
                Some(doc.summary.to_string()),
            )
        })
        .collect();
    }

    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |items: &mut Vec<CompletionItem>, item: CompletionItem| {
        if item.label != "_" && seen.insert(item.label.clone()) {
            items.push(item);
        }
    };
    if let Some((index, scope)) = scope {
        for (name, binding) in &scope.names {
            let detail = binding.symbol.map(|id| index.symbols[id].detail.clone());
            let mut entry = item(name, CompletionItemKind::VARIABLE, detail, None);
            entry.sort_text = Some(format!("0{name}"));
            add(&mut items, entry);
        }
    }
    if let Some(index) = index {
        for (name, binding) in &index.globals {
            let detail = binding.symbol.map(|id| index.symbols[id].detail.clone());
            add(
                &mut items,
                item(name, CompletionItemKind::CONSTANT, detail, None),
            );
        }
        for (name, id) in &index.capabilities {
            let detail = Some(index.symbols[*id].detail.clone());
            add(
                &mut items,
                item(name, CompletionItemKind::INTERFACE, detail, None),
            );
        }
        for (name, id) in &index.units {
            add(
                &mut items,
                unit_item(name, &index.symbols[*id].detail, index.symbols[*id].kind),
            );
        }
        for use_info in &index.uses {
            let detail = Some(index.symbols[use_info.symbol].detail.clone());
            add(
                &mut items,
                item(&use_info.alias, CompletionItemKind::MODULE, detail, None),
            );
        }
    }
    let prelude = world.prelude();
    if let Some(std) = &prelude.index {
        for (name, id) in &std.units {
            if !name.starts_with('_') {
                let mut entry = unit_item(name, &std.symbols[*id].detail, std.symbols[*id].kind);
                entry.label_details = Some(lsp_types::CompletionItemLabelDetails {
                    detail: None,
                    description: Some(PRELUDE_ALIAS.to_string()),
                });
                add(&mut items, entry);
            }
        }
    }
    add(
        &mut items,
        item(
            PRELUDE_ALIAS,
            CompletionItemKind::MODULE,
            Some("the prelude".into()),
            None,
        ),
    );
    for name in BUILTINS {
        let doc = docs::builtin(name);
        add(
            &mut items,
            item(
                name,
                CompletionItemKind::FUNCTION,
                doc.map(|d| d.signature.to_string()),
                doc.map(|d| d.summary.to_string()),
            ),
        );
    }
    for word in BODY_KEYWORDS {
        add(&mut items, keyword_item(word));
    }
    items
}

/// The file as it would be with the cursor's line replaced by `_ = none`,
/// when the cursor's line is what keeps it from parsing.
fn scoped(
    world: &World,
    file: &Rc<SourceFile>,
    offset: usize,
    line_start: usize,
) -> Rc<SourceFile> {
    let broken = file.index.is_none()
        || file
            .recovered
            .skipped
            .iter()
            .any(|(start, end)| *start <= offset && offset <= *end);
    if !broken {
        return file.clone();
    }
    let line_end = file.text[offset..]
        .find('\n')
        .map_or(file.text.len(), |i| offset + i);
    let line = &file.text[line_start..line_end];
    let indent = &line[..line.len() - line.trim_start().len()];
    let patched = format!(
        "{}{indent}_ = none{}",
        &file.text[..line_start],
        &file.text[line_end..]
    );
    Rc::new(SourceFile::new(
        file.uri.clone(),
        file.path.clone(),
        patched,
        world.encoding(),
    ))
}

/// The dotted names at the end of `text`: `j.next` in `if j.next`.
fn trailing_path(text: &str) -> Vec<String> {
    let tail: String = text
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    tail.split('.')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn item(
    label: &str,
    kind: CompletionItemKind,
    detail: Option<String>,
    documentation: Option<String>,
) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail,
        documentation: documentation.map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        }),
        ..CompletionItem::default()
    }
}

fn keyword_item(word: &str) -> CompletionItem {
    let doc = docs::keyword(word);
    let mut entry = item(
        word,
        CompletionItemKind::KEYWORD,
        doc.map(|d| d.signature.to_string()),
        doc.map(|d| format!("{} *Spec section {}.*", d.summary, d.section)),
    );
    entry.sort_text = Some(format!("9{word}"));
    entry
}

fn unit_item(name: &str, detail: &str, kind: SymbolKind) -> CompletionItem {
    let kind = match kind {
        SymbolKind::Unit(UnitKind::Machine) => CompletionItemKind::CLASS,
        _ => CompletionItemKind::FUNCTION,
    };
    item(name, kind, Some(detail.to_string()), None)
}

fn fields(names: &[&str], what: &str, section: &str) -> Vec<CompletionItem> {
    names
        .iter()
        .map(|name| {
            item(
                name,
                CompletionItemKind::FIELD,
                Some(format!("{what} field")),
                Some(format!("*Spec section {section}.*")),
            )
        })
        .collect()
}

/// The fields a judgment answer carries, by the verb its declaration uses.
fn answer_fields(declaration: &str) -> Vec<CompletionItem> {
    if declaration.contains(" pick among ") {
        fields(
            &["label", "confidence", "probabilities", "index", "item"],
            "choice",
            "6.4a",
        )
    } else if declaration.contains(" pick") {
        fields(&["label", "confidence", "probabilities"], "choice", "6.3")
    } else if declaration.contains(" rate") {
        fields(
            &[
                "level",
                "score",
                "normalized",
                "confidence",
                "probabilities",
            ],
            "level",
            "6.4",
        )
    } else {
        Vec::new()
    }
}

fn symbol_items(index: &Index, ids: &[SymbolId], kind: CompletionItemKind) -> Vec<CompletionItem> {
    ids.iter()
        .map(|id| {
            let symbol = &index.symbols[*id];
            item(&symbol.name, kind, Some(symbol.detail.clone()), None)
        })
        .collect()
}

/// What `<path>.` can be followed by.
fn members(
    world: &mut World,
    file: &Rc<SourceFile>,
    scope: Option<&crate::index::Scope>,
    path: &[String],
) -> Vec<CompletionItem> {
    let Some(index) = file.index.as_ref() else {
        return Vec::new();
    };
    let root = path[0].as_str();
    let binding: Option<&Binding> = scope
        .and_then(|s| s.names.get(root))
        .or_else(|| index.globals.get(root));

    if let Some(binding) = binding {
        return match (&binding.source, path.len()) {
            (Some(Source::Unit(unit)), depth) => {
                let Some(resolved) = world.unit(file, unit) else {
                    return Vec::new();
                };
                let unit_index = resolved.file.index.as_ref().expect("indexed");
                match (resolved.symbol().kind, depth) {
                    (SymbolKind::Unit(UnitKind::Judgment), 1) => {
                        let results: Vec<SymbolId> = unit_index
                            .children(Some(resolved.symbol))
                            .into_iter()
                            .filter(|id| unit_index.symbols[*id].kind == SymbolKind::Result)
                            .collect();
                        symbol_items(unit_index, &results, CompletionItemKind::FIELD)
                    }
                    (SymbolKind::Unit(UnitKind::Judgment), 2) => unit_index
                        .child(resolved.symbol, &path[1])
                        .map(|id| answer_fields(&unit_index.symbols[id].detail))
                        .unwrap_or_default(),
                    (SymbolKind::Unit(UnitKind::Machine), 1) => fields(
                        &["state", "steps", "done", "verified", "events"],
                        "machine result",
                        "7.8",
                    ),
                    _ => Vec::new(),
                }
            }
            (Some(Source::Shape(ids) | Source::Record(ids)), 1) => {
                symbol_items(index, ids, CompletionItemKind::FIELD)
            }
            (Some(Source::Handle), 1) => ["observe", "send", "wait", "stop"]
                .into_iter()
                .filter_map(|verb| {
                    docs::handle_verb(verb).map(|d| {
                        item(
                            verb,
                            CompletionItemKind::METHOD,
                            Some(d.signature.into()),
                            Some(d.summary.into()),
                        )
                    })
                })
                .collect(),
            (Some(Source::Handle), 2) if path[1] == "observe" => fields(
                &["status", "last_message", "tail", "exit_code"],
                "observation",
                "9.1",
            ),
            (_, 1) => binding
                .symbol
                .map(|id| answer_fields(&index.symbols[id].detail))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
    }

    if let Some(id) = index.capabilities.get(root)
        && path.len() == 1
    {
        let SymbolKind::Capability(kind) = index.symbols[*id].kind else {
            return Vec::new();
        };
        let declared: Vec<SymbolId> = index.children(Some(*id));
        if !declared.is_empty() {
            return symbol_items(index, &declared, CompletionItemKind::METHOD);
        }
        if kind == CapabilityKind::Tool {
            // An open tool: offer the verbs this file already calls on it.
            let mut verbs = BTreeSet::new();
            let needle = format!("{root}.");
            for (at, _) in file.text.match_indices(&needle) {
                let boundary = file.text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
                let verb: String = file.text[at + needle.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if boundary && !verb.is_empty() {
                    verbs.insert(verb);
                }
            }
            return verbs
                .into_iter()
                .map(|verb| {
                    item(
                        &verb,
                        CompletionItemKind::METHOD,
                        Some(format!("{root}.{verb}")),
                        None,
                    )
                })
                .collect();
        }
        return kind_verbs(kind)
            .iter()
            .filter_map(|verb| {
                docs::verb(kind, verb).map(|d| {
                    item(
                        verb,
                        CompletionItemKind::METHOD,
                        Some(d.signature.into()),
                        Some(d.summary.into()),
                    )
                })
            })
            .collect();
    }

    // A module: `harness.`, `std.`, `a.b.`.
    let Some(module) = world.module(file, path) else {
        return Vec::new();
    };
    let Some(module_index) = module.index.as_ref() else {
        return Vec::new();
    };
    let mut items: Vec<CompletionItem> = module_index
        .units
        .iter()
        .filter(|(name, _)| !name.starts_with('_'))
        .map(|(name, id)| {
            unit_item(
                name,
                &module_index.symbols[*id].detail,
                module_index.symbols[*id].kind,
            )
        })
        .collect();
    items.extend(module_index.uses.iter().map(|u| {
        item(
            &u.alias,
            CompletionItemKind::MODULE,
            Some(module_index.symbols[u.symbol].detail.clone()),
            None,
        )
    }));
    items
}

/// The labels `<path> is` can be compared against.
fn labels(
    world: &mut World,
    file: &Rc<SourceFile>,
    scope: Option<&crate::index::Scope>,
    path: &[String],
) -> Vec<CompletionItem> {
    let Some(index) = file.index.as_ref() else {
        return Vec::new();
    };
    let binding = path
        .first()
        .and_then(|root| scope.and_then(|s| s.names.get(root)));
    match (binding.and_then(|b| b.source.as_ref()), path.len()) {
        (Some(Source::Labels(ids)), 1) => {
            return symbol_items(index, ids, CompletionItemKind::ENUM_MEMBER);
        }
        (Some(Source::Unit(unit)), 2) => {
            if let Some(resolved) = world.unit(file, unit) {
                let unit_index = resolved.file.index.as_ref().expect("indexed");
                if let Some(member) = unit_index.child(resolved.symbol, &path[1]) {
                    let ids = unit_index.labels_of(member);
                    return symbol_items(unit_index, &ids, CompletionItemKind::ENUM_MEMBER);
                }
            }
        }
        _ => {}
    }
    // Any label this file declares.
    let mut seen = BTreeSet::new();
    let ids: Vec<SymbolId> = index
        .all_labels()
        .into_iter()
        .filter(|id| seen.insert(index.symbols[*id].name.clone()))
        .collect();
    symbol_items(index, &ids, CompletionItemKind::ENUM_MEMBER)
}

/// The states of the machine the line at `line_start` sits in, read from the
/// text: an `on` line being typed keeps the machine from parsing, and no
/// statement can stand in for a transition the way `_ = none` stands in for
/// a statement.
fn states(text: &str, line_start: usize) -> Vec<CompletionItem> {
    let header = text[..line_start]
        .rmatch_indices("\nmachine ")
        .next()
        .map(|(at, _)| at + 1)
        .or_else(|| text.starts_with("machine ").then_some(0));
    let Some(header) = header else {
        return Vec::new();
    };
    let mut names = BTreeSet::new();
    for line in text[header..].lines().skip(1) {
        if line.chars().next().is_some_and(|c| !c.is_whitespace()) {
            break;
        }
        let Some(rest) = line.trim_start().strip_prefix("state ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            names.insert(name);
        }
    }
    names
        .into_iter()
        .map(|name| {
            item(
                &name,
                CompletionItemKind::ENUM_MEMBER,
                Some(format!("state {name}")),
                None,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_index::Encoding;
    use crate::world::{canonical, uri_from_path};
    use jevscript_compiler::Resolver;
    use jevscript_syntax::Keyword;
    use std::path::Path;

    fn labels_at(world: &mut World, text: &str, name: &str, cursor: &str) -> Vec<String> {
        let dir = canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"));
        let uri = uri_from_path(&dir.join(name));
        world.open(uri.clone(), text.to_string(), 1);
        let file = world.file(&uri).expect("open");
        // `|` marks the cursor inside the marker text.
        let (left, right) = cursor.split_once('|').unwrap_or((cursor, ""));
        let offset = text.find(&format!("{left}{right}")).expect("cursor marker") + left.len();
        let mut labels: Vec<String> = complete(world, &file, offset)
            .into_iter()
            .map(|i| i.label)
            .collect();
        labels.sort();
        labels
    }

    fn world() -> World {
        World::new(Encoding::Utf16, vec![], Resolver::default())
    }

    #[test]
    fn keywords_are_the_reserved_words() {
        // Spec 2.5; `verify` is written as a call in `until verify(...)`,
        // and `log` is a contextual word (5.8).
        for word in BODY_KEYWORDS.iter().chain(TOP_LEVEL.iter()) {
            assert!(
                Keyword::from_word(word).is_some() || matches!(*word, "verify" | "log"),
                "`{word}` is not a reserved word (spec 2.5)"
            );
        }
    }

    #[test]
    fn log_levels_complete_only_for_a_log_form() {
        // Spec 5.8: log levels follow contextual log forms, but a unit named
        // `log` keeps command-form arguments.
        let source = "program demo\ntask main:\n  log info 1\n  x = log debug 2\n";
        for cursor in ["log |info", "log |debug"] {
            assert_eq!(
                labels_at(&mut world(), source, "log_completion.jev", cursor),
                ["debug", "error", "info", "warn"]
            );
        }
        let shadowed = "program demo\ndef log(info):\n  return info\ntask main:\n  log info 1\n";
        let labels = labels_at(&mut world(), shadowed, "log_completion.jev", "log |info");
        assert!(!labels.contains(&"debug".into()), "{labels:?}");
        assert!(labels.contains(&"log".into()), "{labels:?}");
    }

    #[test]
    fn judgment_results_after_a_dot_while_the_line_is_broken() {
        let source = include_str!("../../../examples/fix_issue_inline.jev").replace(
            "    if stuck(obs.recent)",
            "    if j.\n    if stuck(obs.recent)",
        );
        let labels = labels_at(&mut world(), &source, "fix_issue_inline.jev", "if j.");
        assert_eq!(labels, ["claims_done", "next", "off_scope"]);
    }

    #[test]
    fn choice_fields_and_labels() {
        let source = include_str!("../../../examples/fix_issue_inline.jev").replace(
            "    if stuck(obs.recent)",
            "    x = j.next.\n    if stuck(obs.recent)",
        );
        let fields = labels_at(&mut world(), &source, "fix_issue_inline.jev", "x = j.next.");
        assert_eq!(fields, ["confidence", "label", "probabilities"]);
        let source = include_str!("../../../examples/fix_issue_inline.jev")
            .replace("j.next is needs_me", "j.next is ");
        let labels = labels_at(&mut world(), &source, "fix_issue_inline.jev", "j.next is ");
        assert_eq!(labels, ["keep_working", "needs_me", "stuck"]);
    }

    #[test]
    fn capability_verbs_shape_fields_and_handle_verbs() {
        let base = include_str!("../../../examples/fix_issue_inline.jev");
        let with = |line: &str| {
            base.replace(
                "    j = read_agent obs",
                &format!("    {line}\n    j = read_agent obs"),
            )
        };
        let mut world = world();
        assert_eq!(
            labels_at(&mut world, &with("me."), "fix_issue_inline.jev", "    me."),
            ["ask", "notify", "take_over"]
        );
        assert_eq!(
            labels_at(
                &mut world,
                &with("x = obs."),
                "fix_issue_inline.jev",
                "x = obs."
            ),
            ["files", "recent", "summary", "tests"]
        );
        assert_eq!(
            labels_at(
                &mut world,
                &with("dev."),
                "fix_issue_inline.jev",
                "    dev.|\n"
            ),
            ["observe", "send", "stop", "wait"]
        );
        assert_eq!(
            labels_at(
                &mut world,
                &with("x = issue."),
                "fix_issue_inline.jev",
                "x = issue."
            ),
            ["body", "branch", "title"]
        );
        let tree = labels_at(
            &mut world,
            &with("tree."),
            "fix_issue_inline.jev",
            "    tree.",
        );
        assert!(tree.contains(&"open_pr".to_string()), "{tree:?}");
    }

    #[test]
    fn module_members_across_a_use() {
        let source = include_str!("../../../examples/fix_issue.jev")
            .replace("  dev.stop", "  harness.\n  dev.stop");
        let labels = labels_at(&mut world(), &source, "fix_issue.jev", "  harness.");
        assert_eq!(labels, ["read_agent", "watch"]);
        let source = include_str!("../../../examples/fix_issue.jev")
            .replace("  dev.stop", "  std.\n  dev.stop");
        let labels = labels_at(&mut world(), &source, "fix_issue.jev", "  std.");
        assert_eq!(labels, ["focus_impl", "repeats", "stuck"]);
    }

    #[test]
    fn scope_names_keywords_and_top_level() {
        let source = include_str!("../../../examples/fix_issue_inline.jev")
            .replace("  dev.stop", "  d\n  dev.stop");
        let labels = labels_at(&mut world(), &source, "fix_issue_inline.jev", "  d");
        for expected in [
            "dev",
            "obs",
            "j",
            "issue",
            "claude",
            "read_agent",
            "stuck",
            "len",
            "until",
        ] {
            assert!(
                labels.contains(&expected.to_string()),
                "`{expected}` in {labels:?}"
            );
        }
        let top = labels_at(&mut world(), "program demo\n\nt", "top.jev", "\n\n");
        assert_eq!(top.len(), TOP_LEVEL.len());
    }

    #[test]
    fn machine_targets_and_capability_kinds() {
        let source = include_str!("../../../examples/review_loop.jev").replace(
            "    on resumed \"the agent has started working again\" -> working",
            "    on resumed \"the agent has started working again\" -> ",
        );
        let labels = labels_at(
            &mut world(),
            &source,
            "review_loop.jev",
            "working again\" -> ",
        );
        assert_eq!(
            labels,
            [
                "approved",
                "nudging",
                "reviewing",
                "waiting_on_me",
                "working"
            ]
        );
        let kinds = labels_at(
            &mut world(),
            "program demo\nneeds x: ",
            "k.jev",
            "needs x: ",
        );
        assert_eq!(kinds, ["agent", "llm", "person", "tool"]);
    }
}
