//! Semantic tokens: highlighting from the real lexer and the name index.
//!
//! Every token the lexer produces is classified — by the [`Index`] when the
//! name has a known role, and by its position otherwise (a contextual word
//! such as `using` or `goal` is a keyword only where spec section 2.5 says it
//! is). A text literal is split around its `{expr}` holes and each hole is
//! highlighted as the expression it is (spec section 2.7). Comments come from
//! the lexer too, so there is no second tokenizer to drift from the first.
//!
//! The types are the protocol's standard ones, so every editor theme colours
//! them. What the spec distinguishes beyond that is carried by modifiers:
//! `unitKind` on `judgment`, `task`, `def` and `machine`, and `judgmentVerb`
//! on `feels`, `pick`, `rate`, `among` and `each`.

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

use jevscript_syntax::token::CONTEXTUAL_KEYWORDS;
use jevscript_syntax::{Keyword, Op, TextPart, Token, TokenKind, lex_all};

use crate::index::{Index, Role, Target};
use crate::line_index::LineIndex;

/// The token types, in legend order.
pub const TYPES: [SemanticTokenType; 15] = [
    SemanticTokenType::KEYWORD,
    SemanticTokenType::COMMENT,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::METHOD,
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::INTERFACE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::ENUM_MEMBER,
    SemanticTokenType::EVENT,
    SemanticTokenType::TYPE,
];

const KEYWORD: u32 = 0;
const COMMENT: u32 = 1;
const STRING: u32 = 2;
const NUMBER: u32 = 3;
const OPERATOR: u32 = 4;
const FUNCTION: u32 = 5;
const METHOD: u32 = 6;
const NAMESPACE: u32 = 7;
const INTERFACE: u32 = 8;
const PARAMETER: u32 = 9;
const VARIABLE: u32 = 10;
const PROPERTY: u32 = 11;
const ENUM_MEMBER: u32 = 12;
const EVENT: u32 = 13;
const TYPE: u32 = 14;

/// The modifiers, in legend order. The last two are Jevscript's own.
pub fn modifiers() -> Vec<SemanticTokenModifier> {
    vec![
        SemanticTokenModifier::DECLARATION,
        SemanticTokenModifier::READONLY,
        SemanticTokenModifier::DEFAULT_LIBRARY,
        SemanticTokenModifier::new("unitKind"),
        SemanticTokenModifier::new("judgmentVerb"),
    ]
}

const DECLARATION: u32 = 1 << 0;
const READONLY: u32 = 1 << 1;
const DEFAULT_LIBRARY: u32 = 1 << 2;
const UNIT_KIND: u32 = 1 << 3;
const JUDGMENT_VERB: u32 = 1 << 4;

/// The legend announced at `initialize`.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TYPES.to_vec(),
        token_modifiers: modifiers(),
    }
}

/// Words that are keywords only in their positions (spec section 2.5), plus
/// the task header's `thresholds`.
fn is_contextual(word: &str) -> bool {
    CONTEXTUAL_KEYWORDS.contains(&word) || matches!(word, "strict" | "tail" | "thresholds")
}

/// One classified range of bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    /// First byte.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
    /// Index into the legend's types.
    pub kind: u32,
    /// Bit set of the legend's modifiers.
    pub modifiers: u32,
}

/// Classify every token of `source`, in order, without overlaps.
pub fn classify(source: &str, index: Option<&Index>) -> Vec<Classified> {
    let lexed = lex_all(source);
    let mut out = Vec::new();
    for comment in &lexed.comments {
        out.push(Classified {
            start: comment.start.offset as usize,
            end: comment.end.offset as usize,
            kind: COMMENT,
            modifiers: 0,
        });
    }
    tokens(&lexed.tokens, 0, index, &mut out);
    out.sort_by_key(|c| (c.start, c.end));
    let mut kept: Vec<Classified> = Vec::with_capacity(out.len());
    for c in out {
        if c.end <= c.start {
            continue;
        }
        if kept.last().is_some_and(|last| c.start < last.end) {
            continue;
        }
        kept.push(c);
    }
    kept
}

/// Classify a token stream whose offsets are relative to `base`.
fn tokens(tokens: &[Token], base: usize, index: Option<&Index>, out: &mut Vec<Classified>) {
    for (i, token) in tokens.iter().enumerate() {
        let start = base + token.span.start.offset as usize;
        let end = base + token.span.end.offset as usize;
        let push = |out: &mut Vec<Classified>, kind: u32, modifiers: u32| {
            out.push(Classified {
                start,
                end,
                kind,
                modifiers,
            })
        };
        let known = index.and_then(|index| {
            let first = index
                .occurrences
                .partition_point(|o| (o.span.start.offset as usize) < start);
            index.occurrences[first..]
                .iter()
                .take_while(|o| o.span.start.offset as usize == start)
                .find(|o| o.span.end.offset as usize == end)
        });
        match &token.kind {
            TokenKind::Keyword(kw) if known.is_none() => {
                let modifiers = match kw {
                    Keyword::Judgment | Keyword::Task | Keyword::Def | Keyword::Machine => {
                        UNIT_KIND
                    }
                    Keyword::Feels
                    | Keyword::Pick
                    | Keyword::Rate
                    | Keyword::Among
                    | Keyword::Each => JUDGMENT_VERB,
                    _ => 0,
                };
                push(out, KEYWORD, modifiers);
            }
            TokenKind::Keyword(_) | TokenKind::Name(_) => {
                let word = match &token.kind {
                    TokenKind::Name(name) => name.as_str(),
                    TokenKind::Keyword(kw) => kw.as_str(),
                    _ => unreachable!(),
                };
                // `log <level>` (spec section 5.8): the parser decided these
                // are a log form, and the index kept where.
                let log = index.and_then(|index| {
                    index.logs.iter().find_map(|(log, level)| {
                        if log.start.offset as usize == start {
                            Some((KEYWORD, 0))
                        } else if level.start.offset as usize == start {
                            Some((ENUM_MEMBER, DEFAULT_LIBRARY))
                        } else {
                            None
                        }
                    })
                });
                let (kind, modifiers) = match (log, known) {
                    (Some(log), _) => log,
                    (None, Some(occurrence)) => classify_occurrence(word, occurrence),
                    (None, None) => classify_bare(word, tokens, i),
                };
                push(out, kind, modifiers);
            }
            TokenKind::Number(_) => push(out, NUMBER, 0),
            TokenKind::Text(text) => {
                let mut cursor = start;
                for part in &text.parts {
                    let TextPart::Interpolation { source, span } = part else {
                        continue;
                    };
                    let hole = base + span.start.offset as usize;
                    // The literal up to and including the `{`.
                    out.push(Classified {
                        start: cursor,
                        end: hole,
                        kind: STRING,
                        modifiers: 0,
                    });
                    let inner = lex_all(source);
                    self::tokens(&inner.tokens, hole, index, out);
                    // Resume at the `}`.
                    cursor = hole + source.len();
                }
                out.push(Classified {
                    start: cursor,
                    end,
                    kind: STRING,
                    modifiers: 0,
                });
            }
            TokenKind::Op(op) => {
                if !matches!(
                    op,
                    Op::LParen
                        | Op::RParen
                        | Op::LBracket
                        | Op::RBracket
                        | Op::LBrace
                        | Op::RBrace
                        | Op::Comma
                        | Op::Colon
                        | Op::Dot
                ) {
                    push(out, OPERATOR, 0);
                }
            }
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof => {}
        }
    }
}

fn classify_occurrence(word: &str, occurrence: &crate::index::Occurrence) -> (u32, u32) {
    let declaration = if occurrence.declaration {
        DECLARATION
    } else {
        0
    };
    let (kind, modifiers) = match occurrence.role {
        Role::Unit(_) => (FUNCTION, 0),
        Role::Prelude | Role::Builtin => (FUNCTION, DEFAULT_LIBRARY),
        Role::Capability => (INTERFACE, READONLY),
        Role::Module => (NAMESPACE, 0),
        // `spawn in tree, prompt ...`: an argument name that is a
        // contextual word is that word's keyword (spec section 2.5).
        Role::Parameter if occurrence.target == Target::None && is_contextual(word) => (KEYWORD, 0),
        Role::Parameter => (PARAMETER, 0),
        // `dev.wait idle`: the mode word of section 9.1.
        Role::Variable if occurrence.target == Target::None && word == "idle" => {
            (ENUM_MEMBER, DEFAULT_LIBRARY)
        }
        Role::Variable => (VARIABLE, 0),
        Role::Input => (VARIABLE, READONLY),
        Role::Output => (VARIABLE, 0),
        Role::Property => (PROPERTY, 0),
        Role::Verb => (METHOD, 0),
        Role::Label | Role::State => (ENUM_MEMBER, 0),
        Role::Event => (EVENT, 0),
        Role::Type => (TYPE, 0),
    };
    (kind, modifiers | declaration)
}

/// A name the index says nothing about: by its position alone.
fn classify_bare(word: &str, tokens: &[Token], i: usize) -> (u32, u32) {
    let kind_at = |at: Option<usize>| at.and_then(|at| tokens.get(at)).map(|t| &t.kind);
    let previous = kind_at(i.checked_sub(1));
    let before = kind_at(i.checked_sub(2));
    let third = kind_at(i.checked_sub(3));
    // `needs <name>: <kind>` and `-> <type>` in a signature block.
    if matches!(previous, Some(TokenKind::Op(Op::Colon)))
        && matches!(before, Some(TokenKind::Name(_)))
        && matches!(third, Some(TokenKind::Keyword(Keyword::Needs)))
    {
        return (TYPE, 0);
    }
    if matches!(previous, Some(TokenKind::Op(Op::Arrow)))
        && matches!(
            word,
            "text" | "number" | "bool" | "list" | "record" | "handle" | "none"
        )
    {
        return (TYPE, 0);
    }
    if matches!(previous, Some(TokenKind::Op(Op::Dot))) {
        return (PROPERTY, 0);
    }
    // `log` is a keyword only where the parser read a log form, which the
    // index reports; anywhere else it is an ordinary name (spec section 5.8).
    if is_contextual(word) && word != "log" {
        return (KEYWORD, 0);
    }
    (VARIABLE, 0)
}

/// Encode classified ranges as the protocol's relative tokens, splitting any
/// range that spans lines (a multi-line text literal) into one per line.
pub fn encode(source: &str, lines: &LineIndex, classified: &[Classified]) -> Vec<SemanticToken> {
    let mut data = Vec::with_capacity(classified.len());
    let mut last_line = 0u32;
    let mut last_start = 0u32;
    for c in classified {
        let mut start = c.start;
        while start < c.end {
            let line = lines.line_of(start);
            let line_end = lines.line_start(line + 1);
            let end = if line + 1 < lines.line_count() {
                c.end.min(line_end.saturating_sub(1))
            } else {
                c.end
            };
            if end > start {
                let from = lines.position(source, start);
                let to = lines.position(source, end);
                let delta_line = from.line - last_line;
                let delta_start = if delta_line == 0 {
                    from.character - last_start
                } else {
                    from.character
                };
                data.push(SemanticToken {
                    delta_line,
                    delta_start,
                    length: to.character - from.character,
                    token_type: c.kind,
                    token_modifiers_bitset: c.modifiers,
                });
                last_line = from.line;
                last_start = from.character;
            }
            if line + 1 >= lines.line_count() {
                break;
            }
            start = line_end;
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_index::Encoding;

    fn kinds(source: &str) -> Vec<(String, u32, u32)> {
        let recovered = crate::recover::parse(source);
        let index = recovered.program.as_ref().map(|p| Index::build(p, source));
        classify(source, index.as_ref())
            .into_iter()
            .map(|c| (source[c.start..c.end].to_string(), c.kind, c.modifiers))
            .collect()
    }

    fn kind_of(all: &[(String, u32, u32)], text: &str) -> (u32, u32) {
        all.iter()
            .find(|(t, _, _)| t == text)
            .map(|(_, k, m)| (*k, *m))
            .unwrap_or_else(|| panic!("`{text}` is classified: {all:?}"))
    }

    #[test]
    fn the_legend_matches_the_constants() {
        let legend = legend();
        assert_eq!(legend.token_types[TYPE as usize], SemanticTokenType::TYPE);
        assert_eq!(legend.token_types[EVENT as usize], SemanticTokenType::EVENT);
        assert_eq!(legend.token_modifiers.len(), 5);
    }

    #[test]
    fn unit_kinds_judgment_verbs_labels_and_capabilities() {
        let source = include_str!("../../../examples/fix_issue_inline.jev");
        let all = kinds(source);
        assert_eq!(kind_of(&all, "judgment"), (KEYWORD, UNIT_KIND));
        assert_eq!(kind_of(&all, "feels"), (KEYWORD, JUDGMENT_VERB));
        assert_eq!(kind_of(&all, "pick"), (KEYWORD, JUDGMENT_VERB));
        assert_eq!(kind_of(&all, "keep_working"), (ENUM_MEMBER, DECLARATION));
        assert_eq!(kind_of(&all, "claude"), (INTERFACE, READONLY | DECLARATION));
        assert_eq!(kind_of(&all, "spawn"), (METHOD, 0));
        assert_eq!(kind_of(&all, "agent"), (TYPE, 0));
        assert_eq!(kind_of(&all, "read_agent"), (FUNCTION, DECLARATION));
        assert_eq!(kind_of(&all, "prompt"), (KEYWORD, 0));
        // `stuck` is both a label of `next` and the prelude def (spec 8.1).
        assert_eq!(kind_of(&all, "stuck"), (ENUM_MEMBER, DECLARATION));
        assert!(all.contains(&("stuck".to_string(), FUNCTION, DEFAULT_LIBRARY)));
        assert_eq!(kind_of(&all, "2k"), (NUMBER, 0));
        assert_eq!(kind_of(&all, "->"), (OPERATOR, 0));
    }

    #[test]
    fn interpolations_are_highlighted_inside_their_strings() {
        // Spec 2.7: `{expr}` inside a text literal is an expression.
        let source =
            "program demo\nin issue: { title }\ntask main:\n  x = \"about {issue.title}!\"\n";
        let all = kinds(source);
        assert_eq!(kind_of(&all, "\"about {"), (STRING, 0));
        assert_eq!(kind_of(&all, "}!\""), (STRING, 0));
        assert!(
            all.iter().any(|(t, k, _)| t == "issue" && *k == VARIABLE),
            "{all:?}"
        );
    }

    #[test]
    fn comments_and_a_broken_buffer_still_highlight() {
        let source = "program demo\n# a note\ntask main:\n  x = (\n";
        let all = kinds(source);
        assert_eq!(kind_of(&all, "# a note"), (COMMENT, 0));
        assert_eq!(kind_of(&all, "task"), (KEYWORD, UNIT_KIND));
        assert_eq!(kind_of(&all, "x"), (VARIABLE, 0));
    }

    #[test]
    fn a_log_form_and_a_name_called_log() {
        // Spec 5.8: `log` and its level only where the parser read a log form.
        let source = "program demo\ntask main:\n  x = log info 1 { n: 2 }\n  log = 3\n";
        let all = kinds(source);
        assert_eq!(
            all[all.iter().position(|(t, _, _)| t == "log").unwrap()],
            ("log".into(), KEYWORD, 0)
        );
        assert_eq!(kind_of(&all, "info"), (ENUM_MEMBER, DEFAULT_LIBRARY));
        assert!(
            all.iter().any(|(t, k, _)| t == "log" && *k == VARIABLE),
            "{all:?}"
        );
    }

    #[test]
    fn machine_words() {
        let source = include_str!("../../../examples/review_loop.jev");
        let all = kinds(source);
        assert_eq!(kind_of(&all, "goal"), (KEYWORD, 0));
        assert_eq!(kind_of(&all, "finished"), (EVENT, DECLARATION));
        assert_eq!(kind_of(&all, "machine"), (KEYWORD, UNIT_KIND));
        assert!(all.iter().any(|(t, k, _)| t == "risky" && *k == KEYWORD));
    }

    #[test]
    fn a_multi_line_string_is_split_per_line() {
        let source = "program demo\ntask main:\n  x = \"\"\"a\nbé\"\"\"\n";
        let lines = LineIndex::new(source, Encoding::Utf16);
        let data = encode(source, &lines, &classify(source, None));
        let strings: Vec<&SemanticToken> = data.iter().filter(|t| t.token_type == STRING).collect();
        assert_eq!(strings.len(), 2);
        assert_eq!(strings[1].delta_line, 1);
        assert_eq!(strings[1].length, 5, "`bé\"\"\"` in UTF-16 units");
    }
}
