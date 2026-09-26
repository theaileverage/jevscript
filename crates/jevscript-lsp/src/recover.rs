//! Parsing a buffer that is being typed into.
//!
//! The parser stops at its first error and does not recover (see
//! `jevscript_syntax::parser`), which is right for `jevscript check` and wrong
//! for an editor: one half-typed line would take the outline, hover and
//! navigation of the whole file with it. This module recovers at the only
//! granularity the grammar makes safe, the top-level declaration (spec
//! section 13: `program`, `use`, `in`, `out`, `needs` and the four units each
//! start in column 0).
//!
//! When a parse fails, the declaration holding the error is blanked out —
//! every character but the line breaks replaced by spaces of the same byte
//! width, so every other byte offset, line and column is exactly where it was —
//! and the file is parsed again. Each round reports one more genuine error, so
//! the editor also sees every broken declaration at once instead of only the
//! first. The result is the rest of the file, parsed by the real parser, with
//! exact spans. No part of the grammar is guessed.

use jevscript_syntax::{Diagnostic, Lexed, Program, Token, TokenKind, lex_all, parse_tokens};

/// How many declarations one parse will blank before giving up.
const MAX_ROUNDS: usize = 64;

/// A parse of a possibly broken buffer.
#[derive(Debug, Clone)]
pub struct Recovered {
    /// The lexer's view of the text as written, errors and all.
    pub lexed: Lexed,
    /// The program, if the `program` line survived. When `errors` is empty
    /// this is exactly what `jevscript_syntax::parse` returns.
    pub program: Option<Program>,
    /// Every lexer and parser error found, in source order.
    pub errors: Vec<Diagnostic>,
    /// The byte ranges that were blanked to get `program`.
    pub skipped: Vec<(usize, usize)>,
}

impl Recovered {
    /// Whether the text parsed with no errors at all.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Parse `source`, recovering at top-level declarations.
pub fn parse(source: &str) -> Recovered {
    let lexed = lex_all(source);
    let chunks = chunks(source, &lexed.tokens);
    let mut errors: Vec<Diagnostic> = lexed
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .cloned()
        .collect();
    let mut blanked = vec![false; chunks.len()];
    for error in &errors {
        if let Some(i) = chunk_at(&chunks, error.span.start.offset as usize) {
            blanked[i] = true;
        }
    }

    let mut program = None;
    let mut last_error: Option<usize> = None;
    let mut last_blamed: Option<usize> = None;
    for _ in 0..MAX_ROUNDS {
        if blanked.first().copied().unwrap_or(false) {
            // The `program` line itself is broken; nothing else can parse.
            break;
        }
        let masked = mask(source, &chunks, &blanked);
        let tokens = if blanked.iter().any(|b| *b) {
            lex_all(&masked).tokens
        } else {
            lexed.tokens.clone()
        };
        match parse_tokens(&tokens) {
            Ok(parsed) => {
                program = Some(parsed);
                break;
            }
            Err(diagnostics) => {
                let Some(error) = diagnostics.into_iter().next() else {
                    break;
                };
                let offset = error.span.start.offset as usize;
                if last_error == Some(offset)
                    && let Some(innocent) = last_blamed
                {
                    // Blaming the previous declaration did not move the
                    // error, so it was not the cause.
                    blanked[innocent] = false;
                }
                let Some(blame) = blame(&chunks, &blanked, &tokens, offset, last_error) else {
                    errors.push(error);
                    break;
                };
                if !errors.contains(&error) {
                    errors.push(error);
                }
                last_error = Some(offset);
                last_blamed = Some(blame);
                blanked[blame] = true;
            }
        }
    }

    errors.sort_by_key(|d| d.span.start.offset);
    errors.dedup();
    let skipped = chunks
        .iter()
        .zip(&blanked)
        .filter(|(_, b)| **b)
        .map(|(c, _)| *c)
        .collect();
    Recovered {
        lexed,
        program,
        errors,
        skipped,
    }
}

/// The byte ranges of the top-level declarations: each starts at a line
/// whose first character is not a space or a comment and runs to the next
/// one. Lines are read from the text rather than the token stream, because an
/// unclosed bracket makes the lexer read every following line as a
/// continuation (spec section 2.1), and a declaration after a half-typed call
/// is still a declaration. A column-0 line inside a multi-line text literal
/// is part of the literal.
fn chunks(source: &str, tokens: &[Token]) -> Vec<(usize, usize)> {
    let texts: Vec<(usize, usize)> = tokens
        .iter()
        .filter(|t| matches!(&t.kind, TokenKind::Text(lit) if lit.multiline))
        .map(|t| (t.span.start.offset as usize, t.span.end.offset as usize))
        .collect();
    let mut starts: Vec<usize> = Vec::new();
    let mut line_start = 0;
    for line in source.split_inclusive('\n') {
        let first = line.chars().next();
        let inside_text = texts
            .iter()
            .any(|(start, end)| *start < line_start && line_start < *end);
        if first.is_some_and(|c| !c.is_whitespace() && c != '#') && !inside_text {
            starts.push(line_start);
        }
        line_start += line.len();
    }
    match starts.first_mut() {
        None => return vec![(0, source.len())],
        // Leading comments belong to the first declaration.
        Some(first) => *first = 0,
    }
    let mut chunks = Vec::with_capacity(starts.len());
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(source.len());
        chunks.push((*start, end));
    }
    chunks
}

fn chunk_at(chunks: &[(usize, usize)], offset: usize) -> Option<usize> {
    chunks
        .iter()
        .position(|(start, end)| *start <= offset && offset < *end)
        .or_else(|| chunks.len().checked_sub(1))
}

/// Which declaration to blank for a parse error at `offset`.
///
/// An error inside a declaration is that declaration's. An error on the first
/// token of a declaration usually means the previous one ended too early (a
/// `task main:` with no body sees the next `task` where it wanted a block),
/// so the previous live declaration is blamed first; if the same error comes
/// back, the declaration it sits on is blamed instead.
fn blame(
    chunks: &[(usize, usize)],
    blanked: &[bool],
    tokens: &[Token],
    offset: usize,
    last_error: Option<usize>,
) -> Option<usize> {
    let at_eof = tokens
        .iter()
        .find(|t| t.span.start.offset as usize >= offset)
        .is_none_or(|t| t.kind == TokenKind::Eof);
    let index = if at_eof {
        blanked.iter().rposition(|b| !*b)?
    } else {
        chunk_at(chunks, offset)?
    };
    let at_start = chunks[index].0 == offset
        || tokens.iter().any(|t| {
            t.span.start.offset as usize == offset
                && t.span.start.column == 0
                && !matches!(t.kind, TokenKind::Dedent | TokenKind::Indent)
        });
    if at_start
        && last_error != Some(offset)
        && !at_eof
        && let Some(previous) = blanked[..index].iter().rposition(|b| !*b)
        && previous > 0
    {
        return Some(previous);
    }
    (index > 0 && !blanked[index]).then_some(index)
}

/// `source` with the blanked declarations replaced by spaces, byte for byte.
fn mask(source: &str, chunks: &[(usize, usize)], blanked: &[bool]) -> String {
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for ((start, end), blank) in chunks.iter().zip(blanked) {
        if !*blank {
            continue;
        }
        out.push_str(&source[cursor..*start]);
        for c in source[*start..*end].chars() {
            if c == '\n' {
                out.push('\n');
            } else {
                out.extend(std::iter::repeat_n(' ', c.len_utf8()));
            }
        }
        cursor = *end;
    }
    out.push_str(&source[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevscript_syntax::ErrorCode;
    use jevscript_syntax::ast::Unit;

    fn unit_names(program: &Program) -> Vec<&str> {
        program
            .units
            .iter()
            .map(|unit| match unit {
                Unit::Judgment(j) => j.name.name.as_str(),
                Unit::Task(t) => t.name.name.as_str(),
                Unit::Def(d) => d.name.name.as_str(),
                Unit::Machine(m) => m.name.name.as_str(),
            })
            .collect()
    }

    #[test]
    fn a_clean_file_parses_exactly_as_the_parser_does() {
        let source = include_str!("../../../examples/fix_issue_inline.jev");
        let recovered = parse(source);
        assert!(recovered.is_clean());
        assert_eq!(
            recovered.program,
            Some(jevscript_syntax::parse(source).expect("parses"))
        );
    }

    #[test]
    fn a_broken_unit_is_skipped_and_the_rest_keeps_exact_spans() {
        let source = "program demo\n\ndef ok(x):\n  return x\n\ntask main:\n  y = (\n\ndef after(z):\n  return z\n";
        let recovered = parse(source);
        let program = recovered.program.expect("the rest parses");
        assert_eq!(unit_names(&program), ["ok", "after"]);
        assert_eq!(recovered.errors.len(), 1);
        assert_eq!(recovered.errors[0].code, ErrorCode::Syntax);
        let Unit::Def(after) = &program.units[1] else {
            panic!("a def");
        };
        let at = after.name.span.start.offset as usize;
        assert_eq!(&source[at..at + 5], "after");
    }

    #[test]
    fn a_unit_with_no_body_blames_itself_not_the_next_unit() {
        let source = "program demo\n\ntask main:\ndef after(z):\n  return z\n";
        let recovered = parse(source);
        let program = recovered.program.expect("the rest parses");
        assert_eq!(unit_names(&program), ["after"]);
    }

    #[test]
    fn a_stray_top_level_line_blames_itself_and_spares_the_unit_before_it() {
        let source = "program demo\n\ndef a(x):\n  return x\n\nfoo = 1\n\ndef b(y):\n  return y\n";
        let recovered = parse(source);
        assert_eq!(recovered.errors.len(), 1, "{:?}", recovered.errors);
        assert_eq!(unit_names(&recovered.program.expect("parses")), ["a", "b"]);
    }

    #[test]
    fn every_broken_unit_is_reported() {
        let source = "program demo\n\ndef a(x):\n  return (\n\ndef b(y):\n  return y\n\ndef c(z):\n  return ]\n";
        let recovered = parse(source);
        assert_eq!(recovered.errors.len(), 2, "{:?}", recovered.errors);
        assert_eq!(unit_names(&recovered.program.expect("parses")), ["b"]);
    }

    #[test]
    fn a_lexer_error_blanks_only_its_declaration() {
        let source = "program demo\n\ndef a(X):\n  return 1\n\ndef b(y):\n  return y\n";
        let recovered = parse(source);
        assert_eq!(recovered.errors[0].code, ErrorCode::UppercaseIdentifier);
        assert_eq!(unit_names(&recovered.program.expect("parses")), ["b"]);
    }

    #[test]
    fn a_broken_program_line_yields_no_program() {
        let recovered = parse("program\n\ndef a(x):\n  return x\n");
        assert!(recovered.program.is_none());
        assert!(!recovered.errors.is_empty());
    }

    #[test]
    fn masking_keeps_offsets_of_multibyte_text() {
        let source = "program demo\n\ndef a(x):\n  y = \"é\" +\n\ndef b(y):\n  return y\n";
        let recovered = parse(source);
        let program = recovered.program.expect("parses");
        let Unit::Def(b) = &program.units[0] else {
            panic!("a def");
        };
        let at = b.name.span.start.offset as usize;
        assert_eq!(&source[at..at + 1], "b");
    }
}
