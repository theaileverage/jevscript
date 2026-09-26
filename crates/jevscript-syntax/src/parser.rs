//! The parser.
//!
//! Consumes the token stream from [`crate::lexer`] and produces the [`Program`]
//! the compiler lowers to IR. Section 13 of the spec is the grammar and the
//! authority for every production; every parsing function names the
//! production it implements.
//!
//! The parser is hand-written recursive descent. Blocks arrive as `INDENT` /
//! `DEDENT` / `NEWLINE` tokens from the lexer, expressions use one function
//! per precedence level of spec section 5.1, and command form is accepted only
//! where section 5.2 allows it: as a whole statement or as the right-hand side
//! of an assignment. Text literals keep their `{expr}` holes as raw source
//! (spec 2.7), but every hole is parsed while the literal is, so a malformed
//! hole is a syntax error at its own span.
//!
//! It stops at the first error and reports it with an exact span. There is no
//! recovery: the first error is the one worth fixing, and a recovered parse
//! would only guess at the rest.

mod expr;
mod judge;
mod stmt;
mod units;

use crate::ast::{Expr, Ident, Program};
use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::lexer::lex;
use crate::span::{Pos, Span};
use crate::token::{Keyword, Op, Token, TokenKind};

/// One production's result: a node, or the diagnostic that stopped it.
type PResult<T> = Result<T, Diagnostic>;

/// Parse a token stream into a [`Program`].
///
/// Checks this stage owns, all from spec section 12:
///
/// - `unbounded_loop`: `loop` or `until` without `max` (5.5);
/// - `pick_no_other` and `pick_arity`: exactly one bare `other` or `none`, two
///   to eight labels (6.3);
/// - `rate_arity` and `rate_bare_degree`: two to ten levels, no level that is
///   only a number or a degree word (6.4);
/// - `subject_not_path`: a judgment subject that is not a variable or field
///   path (6.1);
/// - `verify_twice`: more than one `verify` in a task (7.4);
/// - `event_no_description`: an `on` line without a description text (7.8).
///   The grammar makes the text mandatory, so nothing after the parser could
///   ever see the line without it;
/// - `duplicate_name` for the repetitions the grammar admits but section 12
///   forbids: a detail key given twice in one block, a gate argument given
///   twice, a gate arm written twice. Units, aliases, states, events and
///   judgment results are the compiler's, which sees them all at once.
///
/// Everything else the grammar rejects is [`ErrorCode::Syntax`] with a message
/// naming what was expected and what was found. Scope and side-effect checks
/// belong to `jevscript-compiler`, which sees the whole program.
///
/// # Errors
///
/// Returns the first diagnostic found, with the span of the offending token.
pub fn parse_tokens(tokens: &[Token]) -> Result<Program, Vec<Diagnostic>> {
    Parser::new(tokens).program()
}

/// Lex and parse `source` in one step.
///
/// # Errors
///
/// Returns the lexer's diagnostics if lexing failed, otherwise the parser's.
pub fn parse(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let tokens = lex(source)?;
    parse_tokens(&tokens)
}

/// Parse one expression on its own, as a text literal's `{expr}` hole is
/// (spec section 2.7).
///
/// `base` is where `source` sits in the enclosing file; every span in the
/// result and in any diagnostic is rebased onto `base.start`, so an error in a
/// hole points at the hole. The parser uses this to check every hole while
/// parsing the literal, and the compiler uses it to lower holes with the same
/// grammar. Line structure has no meaning inside a hole, so the lexer's
/// `NEWLINE` / `INDENT` / `DEDENT` tokens are dropped before parsing.
///
/// # Errors
///
/// Returns the lexer's diagnostics if lexing failed, otherwise the parser's.
pub fn parse_expression(source: &str, base: Span) -> Result<Expr, Vec<Diagnostic>> {
    parse_expression_in(source, base, false)
}

/// [`parse_expression`] for a hole in a file that declares a unit named
/// `log`, where `log <word> <expr>` is a command-form call to that unit and
/// never a `log` expression (spec section 5.8). `log_is_unit` is what
/// [`crate::ast::Program::declares_unit`] says for `"log"`.
///
/// # Errors
///
/// As [`parse_expression`].
pub fn parse_expression_in(
    source: &str,
    base: Span,
    log_is_unit: bool,
) -> Result<Expr, Vec<Diagnostic>> {
    let rebase = |span: Span| {
        Span::new(
            rebase_pos(span.start, base.start),
            rebase_pos(span.end, base.start),
        )
    };
    let tokens = match lex(source) {
        Ok(tokens) => tokens,
        Err(diagnostics) => {
            return Err(diagnostics
                .into_iter()
                .map(|d| Diagnostic {
                    span: rebase(d.span),
                    ..d
                })
                .collect());
        }
    };
    let tokens: Vec<Token> = tokens
        .into_iter()
        .filter(|t| {
            !matches!(
                t.kind,
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
            )
        })
        .map(|t| Token::new(t.kind, rebase(t.span)))
        .collect();
    let mut parser = Parser::new(&tokens).with_log_unit(log_is_unit);
    let result = parser.expr().and_then(|expr| {
        if parser.at_eof() {
            Ok(expr)
        } else {
            Err(parser.expected("the end of the interpolation"))
        }
    });
    // The fragment's end is the hole's closing brace, not the file's end.
    result.map_err(|d| {
        vec![Diagnostic {
            message: d
                .message
                .replace("the end of the file", "the end of the interpolation"),
            ..d
        }]
    })
}

/// Moves a position lexed from a fragment onto the fragment's place in the
/// file. Only the fragment's first line shares a line with `base`.
fn rebase_pos(pos: Pos, base: Pos) -> Pos {
    if pos.line <= 1 {
        Pos::new(
            base.line,
            base.column + pos.column,
            base.offset + pos.offset,
        )
    } else {
        Pos::new(
            base.line + pos.line - 1,
            pos.column,
            base.offset + pos.offset,
        )
    }
}

/// Whether a token stream declares a unit named `log`: `judgment`, `task`,
/// `def` or `machine` directly followed by the name (spec section 5.8).
fn declares_log_unit(tokens: &[Token]) -> bool {
    tokens.windows(2).any(|pair| {
        matches!(
            pair[0].kind,
            TokenKind::Keyword(Keyword::Judgment | Keyword::Task | Keyword::Def | Keyword::Machine)
        ) && pair[1].name() == Some("log")
    })
}

/// The token the parser reports when it runs past the end of its input.
static EOF: TokenKind = TokenKind::Eof;

/// A hand-written recursive-descent parser over the token stream.
///
/// The type exists so the shape of the stage is fixed: precedence climbing for
/// expressions (spec section 5.1), `INDENT` / `DEDENT` for blocks, and command
/// form accepted only as a whole statement or as the right-hand side of an
/// assignment (spec section 5.2).
pub struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    diagnostics: Vec<Diagnostic>,
    /// The `verify` conditions seen in the unit being parsed; a second one is
    /// `verify_twice` (spec section 7.4).
    verifies: Vec<Span>,
    /// Whether the file declares a unit named `log`, which keeps `log` an
    /// ordinary callee and turns the `log` expression off (spec section 5.8).
    log_is_unit: bool,
}

impl<'a> Parser<'a> {
    /// A parser positioned at the start of `tokens`.
    pub fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            position: 0,
            diagnostics: Vec::new(),
            verifies: Vec::new(),
            log_is_unit: declares_log_unit(tokens),
        }
    }

    /// Say whether the file these tokens come from declares a unit named
    /// `log` (spec section 5.8). [`Parser::new`] finds it in a whole file on
    /// its own; a fragment such as an interpolation hole cannot.
    #[must_use]
    pub fn with_log_unit(mut self, log_is_unit: bool) -> Self {
        self.log_is_unit = log_is_unit;
        self
    }

    /// The token the parser is looking at, if any.
    pub fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.position)
    }

    /// The span of the current token, or an empty span at the end of the file.
    pub fn span(&self) -> Span {
        self.peek().map(|t| t.span).unwrap_or_default()
    }

    /// The diagnostics collected so far.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Parse the whole file: `program = "program" NAME NEWLINE { use_decl }
    /// { decl } { unit }` (spec section 13).
    ///
    /// # Errors
    ///
    /// Returns the first diagnostic found. It is also kept in
    /// [`Parser::diagnostics`].
    pub fn program(&mut self) -> Result<Program, Vec<Diagnostic>> {
        match self.parse_program() {
            Ok(program) => Ok(program),
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic.clone());
                Err(vec![diagnostic])
            }
        }
    }

    /* ---------------------------------------------------------------- */
    /* cursor                                                            */
    /* ---------------------------------------------------------------- */

    /// The kind of the current token; `Eof` past the end.
    fn kind(&self) -> &'a TokenKind {
        self.kind_at(0)
    }

    /// The kind of the token `ahead` positions on; `Eof` past the end.
    fn kind_at(&self, ahead: usize) -> &'a TokenKind {
        self.tokens
            .get(self.position + ahead)
            .map_or(&EOF, |t| &t.kind)
    }

    /// Consumes the current token.
    fn bump(&mut self) -> Option<&'a Token> {
        let token = self.tokens.get(self.position)?;
        self.position += 1;
        Some(token)
    }

    /// Where the last consumed token ended. This is the end of whatever was
    /// just parsed, which is what node spans want.
    fn prev_end(&self) -> Pos {
        self.position
            .checked_sub(1)
            .and_then(|i| self.tokens.get(i))
            .map(|t| t.span.end)
            .unwrap_or_default()
    }

    /// Whether the last consumed token closed a block.
    fn prev_was_dedent(&self) -> bool {
        self.position
            .checked_sub(1)
            .and_then(|i| self.tokens.get(i))
            .is_some_and(|t| t.kind == TokenKind::Dedent)
    }

    fn at_eof(&self) -> bool {
        matches!(self.kind(), TokenKind::Eof)
    }

    fn at_op(&self, op: Op) -> bool {
        matches!(self.kind(), TokenKind::Op(o) if *o == op)
    }

    fn at_kw(&self, kw: Keyword) -> bool {
        matches!(self.kind(), TokenKind::Keyword(k) if *k == kw)
    }

    /// Whether the current token is the identifier `word`. Contextual keywords
    /// (spec section 2.5) lex as identifiers and are recognised this way, only
    /// in the positions the spec names.
    fn at_word(&self, word: &str) -> bool {
        matches!(self.kind(), TokenKind::Name(n) if n == word)
    }

    fn at_newline(&self) -> bool {
        matches!(self.kind(), TokenKind::Newline)
    }

    /// Whether the block being parsed has more lines.
    fn block_continues(&self) -> bool {
        !matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof)
    }

    fn eat_op(&mut self, op: Op) -> bool {
        let hit = self.at_op(op);
        if hit {
            self.bump();
        }
        hit
    }

    fn eat_kw(&mut self, kw: Keyword) -> bool {
        let hit = self.at_kw(kw);
        if hit {
            self.bump();
        }
        hit
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let hit = self.at_word(word);
        if hit {
            self.bump();
        }
        hit
    }

    fn expect_op(&mut self, op: Op) -> PResult<Span> {
        if self.at_op(op) {
            Ok(self.bump().map(|t| t.span).unwrap_or_default())
        } else {
            Err(self.expected(&format!("`{}`", op.as_str())))
        }
    }

    fn expect_kw(&mut self, kw: Keyword) -> PResult<Span> {
        if self.at_kw(kw) {
            Ok(self.bump().map(|t| t.span).unwrap_or_default())
        } else {
            Err(self.expected(&format!("`{}`", kw.as_str())))
        }
    }

    fn expect_word(&mut self, word: &str) -> PResult<Span> {
        if self.at_word(word) {
            Ok(self.bump().map(|t| t.span).unwrap_or_default())
        } else {
            Err(self.expected(&format!("`{word}`")))
        }
    }

    /// An identifier. A reserved word here is an error that says so, because
    /// `as loop` is the mistake spec section 3.9 calls out.
    fn expect_name(&mut self, what: &str) -> PResult<Ident> {
        match self.kind() {
            TokenKind::Name(name) => {
                let span = self.span();
                self.bump();
                Ok(Ident {
                    name: name.clone(),
                    span,
                })
            }
            _ => Err(self.expected(what)),
        }
    }

    /// An identifier or a reserved word, taken by its spelling. Field names
    /// (`dev.stop`) and argument names (`spawn in tree`) may be reserved words
    /// (spec sections 5.2 and 9.1).
    fn expect_word_or_keyword(&mut self, what: &str) -> PResult<Ident> {
        match self.kind() {
            TokenKind::Name(name) => {
                let span = self.span();
                self.bump();
                Ok(Ident {
                    name: name.clone(),
                    span,
                })
            }
            TokenKind::Keyword(kw) => {
                let span = self.span();
                self.bump();
                Ok(Ident {
                    name: kw.as_str().to_string(),
                    span,
                })
            }
            _ => Err(self.expected(what)),
        }
    }

    fn expect_number(&mut self, what: &str) -> PResult<(f64, Span)> {
        match self.kind() {
            TokenKind::Number(value) => {
                let span = self.span();
                self.bump();
                Ok((*value, span))
            }
            _ => Err(self.expected(what)),
        }
    }

    fn expect_newline(&mut self) -> PResult<()> {
        if self.at_newline() {
            self.bump();
            Ok(())
        } else {
            Err(self.expected("the end of the line"))
        }
    }

    /// The end of a statement line. A statement that ended with an indented
    /// block has already consumed its `NEWLINE`; the lexer puts the block's
    /// `DEDENT` after it and never emits a `NEWLINE` after a `DEDENT`.
    fn end_of_line(&mut self) -> PResult<()> {
        if self.prev_was_dedent() {
            Ok(())
        } else {
            self.expect_newline()
        }
    }

    /// `NEWLINE INDENT`, the start of every block (spec sections 2.2 and 13).
    fn open_block(&mut self, what: &str) -> PResult<()> {
        self.expect_newline()?;
        if matches!(self.kind(), TokenKind::Indent) {
            self.bump();
            Ok(())
        } else {
            Err(self.expected(&format!("an indented block {what}")))
        }
    }

    fn close_block(&mut self) -> PResult<()> {
        if matches!(self.kind(), TokenKind::Dedent) {
            self.bump();
            Ok(())
        } else {
            Err(self.expected("the end of the block"))
        }
    }

    /* ---------------------------------------------------------------- */
    /* diagnostics                                                       */
    /* ---------------------------------------------------------------- */

    /// A syntax error at the current token: "expected X, found Y".
    fn expected(&self, what: &str) -> Diagnostic {
        Diagnostic::error(
            ErrorCode::Syntax,
            format!("expected {what}, found {}", self.describe()),
            self.span(),
        )
    }

    /// A diagnostic with a spec code at a span the caller chose.
    fn error_at(&self, code: ErrorCode, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::error(code, message, span)
    }

    /// The current token, as an error message names it.
    fn describe(&self) -> String {
        match self.kind() {
            TokenKind::Name(name) => format!("`{name}`"),
            TokenKind::Keyword(kw) => format!("the reserved word `{}`", kw.as_str()),
            TokenKind::Number(value) => format!("the number `{value}`"),
            TokenKind::Text(_) => "a text literal".to_string(),
            TokenKind::Op(op) => format!("`{}`", op.as_str()),
            TokenKind::Newline => "the end of the line".to_string(),
            TokenKind::Indent => "an indented block".to_string(),
            TokenKind::Dedent => "the end of the block".to_string(),
            TokenKind::Eof => "the end of the file".to_string(),
        }
    }
}
