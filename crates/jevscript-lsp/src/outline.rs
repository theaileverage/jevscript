//! The document outline and folding ranges.
//!
//! The outline is the [`Index`]'s declarations as a tree: imports, inputs,
//! outputs and capabilities, then the units with what each declares — a
//! judgment's results and their labels, a machine's states and events, a
//! tool's verbs.
//!
//! Folding comes from the token stream alone, so it works on a buffer that
//! does not parse: every `INDENT` opens a fold at the line that introduced
//! the block and its `DEDENT` closes it (spec section 2.2), and multi-line
//! text literals, runs of comments and runs of `use` lines fold too.

use lsp_types::{DocumentSymbol, FoldingRange, FoldingRangeKind, SymbolKind as LspKind};

use jevscript_compiler::link::UnitKind;
use jevscript_syntax::{Keyword, Lexed, TokenKind};

use crate::index::{Index, SymbolId, SymbolKind};
use crate::line_index::LineIndex;

/// The outline of an indexed file.
pub fn document_symbols(index: &Index, source: &str, lines: &LineIndex) -> Vec<DocumentSymbol> {
    let roots: Vec<SymbolId> = index
        .children(None)
        .into_iter()
        .filter(|id| index.symbols[*id].kind != SymbolKind::Program)
        .collect();
    roots
        .into_iter()
        .filter_map(|id| symbol(index, id, source, lines))
        .collect()
}

fn lsp_kind(kind: SymbolKind) -> Option<LspKind> {
    Some(match kind {
        SymbolKind::Module => LspKind::MODULE,
        SymbolKind::Input => LspKind::VARIABLE,
        SymbolKind::InputField => LspKind::FIELD,
        SymbolKind::Output => LspKind::VARIABLE,
        SymbolKind::Capability(_) => LspKind::INTERFACE,
        SymbolKind::ToolVerb => LspKind::METHOD,
        SymbolKind::Unit(UnitKind::Machine) => LspKind::CLASS,
        SymbolKind::Unit(_) => LspKind::FUNCTION,
        SymbolKind::Result => LspKind::PROPERTY,
        SymbolKind::State => LspKind::ENUM,
        SymbolKind::Event => LspKind::EVENT,
        SymbolKind::Label => LspKind::ENUM_MEMBER,
        SymbolKind::Program
        | SymbolKind::Parameter
        | SymbolKind::Local
        | SymbolKind::ShapeField => return None,
    })
}

#[allow(deprecated, reason = "`DocumentSymbol::deprecated` must be written")]
fn symbol(index: &Index, id: SymbolId, source: &str, lines: &LineIndex) -> Option<DocumentSymbol> {
    let symbol = &index.symbols[id];
    let kind = lsp_kind(symbol.kind)?;
    let children: Vec<DocumentSymbol> = index
        .children(Some(id))
        .into_iter()
        .filter_map(|child| symbol_under(index, child, source, lines))
        .collect();
    let range = lines.range(source, symbol.full);
    let mut selection = lines.range(source, symbol.selection);
    if selection.start < range.start || selection.end > range.end {
        selection = range;
    }
    Some(DocumentSymbol {
        name: symbol.name.clone(),
        detail: (symbol.detail != symbol.name).then(|| symbol.detail.clone()),
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: selection,
        children: (!children.is_empty()).then_some(children),
    })
}

/// A child, kept only when it lies inside its parent's range: an inline
/// judgment's labels belong to the variable they are assigned to, which the
/// outline does not show, so they are left out rather than misplaced.
fn symbol_under(
    index: &Index,
    id: SymbolId,
    source: &str,
    lines: &LineIndex,
) -> Option<DocumentSymbol> {
    let parent = index.symbols[id].parent.map(|p| index.symbols[p].full)?;
    let full = index.symbols[id].full;
    if full.start.offset < parent.start.offset || full.end.offset > parent.end.offset {
        return None;
    }
    symbol(index, id, source, lines)
}

/// Folding ranges for `source`.
pub fn folding_ranges(lexed: &Lexed) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();
    let tokens = &lexed.tokens;
    let mut open: Vec<u32> = Vec::new();
    // The line of the last token that is not layout.
    let mut last_line = 0u32;
    for token in tokens {
        match &token.kind {
            TokenKind::Indent => open.push(last_line),
            TokenKind::Dedent => {
                if let Some(start) = open.pop()
                    && last_line > start
                {
                    ranges.push(region(start, last_line, None));
                }
            }
            TokenKind::Newline | TokenKind::Eof => {}
            kind => {
                let start = token.span.start.line.saturating_sub(1);
                let end = token.span.end.line.saturating_sub(1);
                if matches!(kind, TokenKind::Text(_)) && end > start {
                    ranges.push(region(start, end, None));
                }
                last_line = end;
            }
        }
    }

    // Runs of comment lines.
    let mut run: Option<(u32, u32)> = None;
    for comment in &lexed.comments {
        let line = comment.start.line.saturating_sub(1);
        run = match run {
            Some((start, end)) if line == end + 1 => Some((start, line)),
            Some((start, end)) => {
                if end > start {
                    ranges.push(region(start, end, Some(FoldingRangeKind::Comment)));
                }
                Some((line, line))
            }
            None => Some((line, line)),
        };
    }
    if let Some((start, end)) = run
        && end > start
    {
        ranges.push(region(start, end, Some(FoldingRangeKind::Comment)));
    }

    // Runs of `use` lines.
    let uses: Vec<u32> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Keyword(Keyword::Use) && t.span.start.column == 0)
        .map(|t| t.span.start.line.saturating_sub(1))
        .collect();
    if let (Some(first), Some(last)) = (uses.first(), uses.last())
        && last > first
    {
        ranges.push(region(*first, *last, Some(FoldingRangeKind::Imports)));
    }
    ranges.sort_by_key(|r| (r.start_line, r.end_line));
    ranges
}

fn region(start: u32, end: u32, kind: Option<FoldingRangeKind>) -> FoldingRange {
    FoldingRange {
        start_line: start,
        start_character: None,
        end_line: end,
        end_character: None,
        kind,
        collapsed_text: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_index::Encoding;
    use jevscript_syntax::lex_all;

    #[test]
    fn the_outline_nests_results_labels_states_and_events() {
        let source = include_str!("../../../examples/review_loop.jev");
        let program = jevscript_syntax::parse(source).expect("parses");
        let index = Index::build(&program, source);
        let lines = LineIndex::new(source, Encoding::Utf16);
        let outline = document_symbols(&index, source, &lines);
        let names: Vec<&str> = outline.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["claude", "tree", "me", "review", "main"]);
        let review = &outline[3];
        assert_eq!(review.kind, LspKind::CLASS);
        let states = review.children.as_ref().expect("states");
        assert_eq!(states.len(), 5);
        assert_eq!(states[0].name, "working");
        assert_eq!(states[0].children.as_ref().map(Vec::len), Some(4));
        let tree = &outline[1];
        assert_eq!(tree.children.as_ref().map(Vec::len), Some(2), "tool verbs");
    }

    #[test]
    fn judgment_results_carry_their_labels() {
        let source = include_str!("../../../examples/fix_issue_inline.jev");
        let program = jevscript_syntax::parse(source).expect("parses");
        let index = Index::build(&program, source);
        let lines = LineIndex::new(source, Encoding::Utf16);
        let outline = document_symbols(&index, source, &lines);
        let judgment = outline
            .iter()
            .find(|s| s.name == "read_agent")
            .expect("the judgment");
        let results = judgment.children.as_ref().expect("results");
        let next = results.iter().find(|r| r.name == "next").expect("next");
        let labels: Vec<&str> = next
            .children
            .as_ref()
            .expect("labels")
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(labels, ["keep_working", "stuck", "needs_me"]);
        assert_eq!(
            outline
                .iter()
                .find(|s| s.name == "issue")
                .map(|s| s.children.as_ref().map(Vec::len)),
            Some(Some(3)),
            "the input's record fields"
        );
    }

    #[test]
    fn folding_follows_blocks_even_in_a_broken_buffer() {
        let source = "program demo\n# one\n# two\ntask main:\n  if true:\n    x = (\n";
        let lexed = lex_all(source);
        let ranges: Vec<(u32, u32)> = folding_ranges(&lexed)
            .iter()
            .map(|r| (r.start_line, r.end_line))
            .collect();
        assert!(ranges.contains(&(1, 2)), "the comment run: {ranges:?}");
        assert!(ranges.contains(&(3, 5)), "the task: {ranges:?}");
        assert!(ranges.contains(&(4, 5)), "the if: {ranges:?}");
    }
}
