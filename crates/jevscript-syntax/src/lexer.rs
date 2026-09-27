//! The lexer.
//!
//! This stage is real. It turns `.jev` source into the token stream the parser
//! consumes, including the `INDENT` / `DEDENT` / `NEWLINE` tokens that carry
//! block structure (spec sections 2.1-2.7 and 13).
//!
//! Rules it enforces:
//!
//! - source is UTF-8 and lines end with LF (2.1);
//! - a statement continues onto the next line while a bracket is open (2.1);
//! - blocks are indented with spaces, and a tab is an [`ErrorCode::Indent`]
//!   error (2.2);
//! - `#` runs to the end of the line (2.3);
//! - an identifier is `[a-z_][a-z0-9_]*`, and an uppercase letter is an
//!   [`ErrorCode::UppercaseIdentifier`] error (2.4);
//! - `2k` is 2000 and `80%` is 0.8 (2.6);
//! - `{expr}` inside a text literal is kept as raw source for the parser, and
//!   `{{` / `}}` are literal braces (2.7).

use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::span::{Pos, Span};
use crate::token::{Keyword, Op, TextLit, TextPart, Token, TokenKind};

/// Lex `source` into tokens.
///
/// Returns every error found rather than stopping at the first, so that
/// `jevscript check` can report a whole file.
///
/// # Errors
///
/// Returns the diagnostics collected if any of them has error severity.
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let lexed = lex_all(source);
    if lexed.diagnostics.iter().any(Diagnostic::is_error) {
        Err(lexed.diagnostics)
    } else {
        Ok(lexed.tokens)
    }
}

/// Everything the lexer saw in one source, whether or not it had errors.
///
/// The lexer does not stop at an error, so the token stream is complete even
/// when `diagnostics` is not empty. Editors read it this way: a buffer being
/// typed into is broken most of the time, and it still needs highlighting.
#[derive(Debug, Clone, PartialEq)]
pub struct Lexed {
    /// Every token, ending with [`TokenKind::Eof`].
    pub tokens: Vec<Token>,
    /// Every `#` comment, from the `#` to the end of its line (spec section
    /// 2.3). The parser never sees these.
    pub comments: Vec<Span>,
    /// Every lexical error, in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Lex `source`, keeping the tokens and comments alongside any errors.
pub fn lex_all(source: &str) -> Lexed {
    let mut lexer = Lexer::new(source);
    lexer.run();
    Lexed {
        tokens: lexer.tokens,
        comments: lexer.comments,
        diagnostics: lexer.diagnostics,
    }
}

struct Lexer {
    src: Vec<char>,
    i: usize,
    line: u32,
    column: u32,
    offset: u32,
    indents: Vec<u32>,
    brackets: u32,
    tokens: Vec<Token>,
    comments: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Self {
            src: source.chars().collect(),
            i: 0,
            line: 1,
            column: 0,
            offset: 0,
            indents: vec![0],
            brackets: 0,
            tokens: Vec::new(),
            comments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /* ---------------------------------------------------------------- */
    /* cursor                                                            */
    /* ---------------------------------------------------------------- */

    fn pos(&self) -> Pos {
        Pos::new(self.line, self.column, self.offset)
    }

    fn peek(&self) -> Option<char> {
        self.src.get(self.i).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.src.get(self.i + ahead).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.src.get(self.i).copied()?;
        self.i += 1;
        self.offset += c.len_utf8() as u32;
        if c == '\n' {
            self.line += 1;
            self.column = 0;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn at_end(&self) -> bool {
        self.i >= self.src.len()
    }

    /// Whether the cursor sits on a line ending. A carriage return before the
    /// newline is part of the ending, never a character of the line, so a CRLF
    /// file (git's `autocrlf` on Windows) lexes exactly as its LF form does.
    fn at_newline(&self) -> bool {
        match self.peek() {
            Some('\n') => true,
            Some('\r') => self.peek_at(1) == Some('\n'),
            _ => false,
        }
    }

    /// Consumes one line ending, LF or CRLF.
    fn eat_newline(&mut self) -> bool {
        if !self.at_newline() {
            return false;
        }
        self.eat('\r');
        self.eat('\n')
    }

    fn push(&mut self, kind: TokenKind, start: Pos) {
        let span = Span::new(start, self.pos());
        self.tokens.push(Token::new(kind, span));
    }

    fn error(&mut self, code: ErrorCode, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }

    fn last_is_newline(&self) -> bool {
        matches!(
            self.tokens.last().map(|t| &t.kind),
            None | Some(TokenKind::Newline)
        )
    }

    /* ---------------------------------------------------------------- */
    /* driver                                                            */
    /* ---------------------------------------------------------------- */

    fn run(&mut self) {
        while !self.at_end() {
            if self.brackets == 0 && self.column == 0 && self.line_indent() == LineKind::Blank {
                continue;
            }
            self.lex_line_body();
        }

        if !self.last_is_newline() {
            let pos = self.pos();
            self.push(TokenKind::Newline, pos);
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            let pos = self.pos();
            self.push(TokenKind::Dedent, pos);
        }
        let pos = self.pos();
        self.push(TokenKind::Eof, pos);
    }

    /// Measures the indentation of the line the cursor sits at the start of and
    /// emits the `INDENT` / `DEDENT` tokens it implies.
    fn line_indent(&mut self) -> LineKind {
        let start = self.pos();
        let mut width = 0u32;
        loop {
            match self.peek() {
                Some(' ') => {
                    self.bump();
                    width += 1;
                }
                Some('\t') => {
                    let tab_start = self.pos();
                    self.bump();
                    self.error(
                        ErrorCode::Indent,
                        "tabs are not allowed in indentation; use spaces",
                        Span::new(tab_start, self.pos()),
                    );
                    width += 1;
                }
                _ => break,
            }
        }

        // A blank or comment-only line has no indentation and no newline token.
        if self.at_end() || self.at_newline() {
            self.eat_newline();
            return LineKind::Blank;
        }
        if self.peek() == Some('#') {
            self.skip_comment();
            self.eat_newline();
            return LineKind::Blank;
        }

        let current = *self.indents.last().unwrap_or(&0);
        if width > current {
            self.indents.push(width);
            self.push(TokenKind::Indent, start);
        } else if width < current {
            while *self.indents.last().unwrap_or(&0) > width {
                self.indents.pop();
                self.push(TokenKind::Dedent, start);
            }
            if *self.indents.last().unwrap_or(&0) != width {
                self.error(
                    ErrorCode::Indent,
                    "this line's indentation does not match any enclosing block",
                    Span::new(start, self.pos()),
                );
                self.indents.push(width);
            }
        }
        LineKind::Code
    }

    fn skip_comment(&mut self) {
        let start = self.pos();
        while !self.at_end() && !self.at_newline() {
            self.bump();
        }
        let span = Span::new(start, self.pos());
        self.comments.push(span);
    }

    /// Lexes tokens up to the end of one source line.
    fn lex_line_body(&mut self) {
        loop {
            match self.peek() {
                None => return,
                Some('\n') => {
                    let start = self.pos();
                    self.bump();
                    // Inside brackets a newline is a continuation, not a statement end.
                    if self.brackets == 0 && !self.last_is_newline() {
                        self.tokens
                            .push(Token::new(TokenKind::Newline, Span::new(start, self.pos())));
                    }
                    return;
                }
                Some(' ') | Some('\r') => {
                    self.bump();
                }
                Some('\t') => {
                    let start = self.pos();
                    self.bump();
                    self.error(
                        ErrorCode::Indent,
                        "tabs are not allowed; use spaces",
                        Span::new(start, self.pos()),
                    );
                }
                Some('#') => self.skip_comment(),
                Some(c) => self.lex_token(c),
            }
        }
    }

    fn lex_token(&mut self, c: char) {
        let start = self.pos();
        if c == '"' {
            self.lex_text(start);
        } else if c.is_ascii_digit() {
            self.lex_number(start);
        } else if c.is_ascii_alphabetic() || c == '_' {
            self.lex_word(start);
        } else {
            self.lex_op(start);
        }
    }

    /* ---------------------------------------------------------------- */
    /* words, numbers, operators                                         */
    /* ---------------------------------------------------------------- */

    fn lex_word(&mut self, start: Pos) {
        let mut word = String::new();
        let mut has_uppercase = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                if c.is_ascii_uppercase() {
                    has_uppercase = true;
                }
                word.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let span = Span::new(start, self.pos());
        if has_uppercase {
            self.error(
                ErrorCode::UppercaseIdentifier,
                format!("`{word}` contains an uppercase letter; identifiers are lowercase"),
                span,
            );
        }
        match Keyword::from_word(&word) {
            Some(kw) => self.tokens.push(Token::new(TokenKind::Keyword(kw), span)),
            None => self.tokens.push(Token::new(TokenKind::Name(word), span)),
        }
    }

    fn lex_number(&mut self, start: Pos) {
        let mut digits = String::new();
        while let Some(c) = self.peek() {
            // A `.` continues the number only when a digit follows it, so that
            // `x.field` and `3.5` do not have to be told apart later.
            let part_of_number = c.is_ascii_digit()
                || (c == '.' && self.peek_at(1).is_some_and(|n| n.is_ascii_digit()));
            if !part_of_number {
                break;
            }
            digits.push(c);
            self.bump();
        }

        let mut value: f64 = digits.parse().unwrap_or(f64::NAN);
        if self.peek() == Some('k') && !self.peek_at(1).is_some_and(is_word_char) {
            self.bump();
            value *= 1000.0;
        } else if self.eat('%') {
            value /= 100.0;
        }

        let span = Span::new(start, self.pos());
        if value.is_nan() {
            self.error(
                ErrorCode::Syntax,
                format!("`{digits}` is not a number"),
                span,
            );
        }
        self.tokens.push(Token::new(TokenKind::Number(value), span));
    }

    fn lex_op(&mut self, start: Pos) {
        let first = self.bump().unwrap_or('\0');
        let second = self.peek();
        let two = match (first, second) {
            ('-', Some('>')) => Some(Op::Arrow),
            ('=', Some('=')) => Some(Op::Eq),
            ('!', Some('=')) => Some(Op::NotEq),
            ('<', Some('=')) => Some(Op::LtEq),
            ('>', Some('=')) => Some(Op::GtEq),
            _ => None,
        };
        if let Some(op) = two {
            self.bump();
            self.push_op(op, start);
            return;
        }

        let one = match first {
            '(' => Some(Op::LParen),
            ')' => Some(Op::RParen),
            '[' => Some(Op::LBracket),
            ']' => Some(Op::RBracket),
            '{' => Some(Op::LBrace),
            '}' => Some(Op::RBrace),
            ',' => Some(Op::Comma),
            ':' => Some(Op::Colon),
            '.' => Some(Op::Dot),
            '=' => Some(Op::Assign),
            '<' => Some(Op::Lt),
            '>' => Some(Op::Gt),
            '+' => Some(Op::Plus),
            '-' => Some(Op::Minus),
            '*' => Some(Op::Star),
            '/' => Some(Op::Slash),
            '%' => Some(Op::Percent),
            _ => None,
        };
        match one {
            Some(op) => self.push_op(op, start),
            None => {
                let span = Span::new(start, self.pos());
                self.error(
                    ErrorCode::Syntax,
                    format!("unexpected character `{first}`"),
                    span,
                );
            }
        }
    }

    fn push_op(&mut self, op: Op, start: Pos) {
        match op {
            Op::LParen | Op::LBracket | Op::LBrace => self.brackets += 1,
            Op::RParen | Op::RBracket | Op::RBrace => {
                self.brackets = self.brackets.saturating_sub(1)
            }
            _ => {}
        }
        self.push(TokenKind::Op(op), start);
    }

    /* ---------------------------------------------------------------- */
    /* text literals                                                     */
    /* ---------------------------------------------------------------- */

    fn lex_text(&mut self, start: Pos) {
        let multiline = self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"');
        if multiline {
            self.bump();
            self.bump();
            self.bump();
        } else {
            self.bump();
        }

        let mut parts: Vec<TextPart> = Vec::new();
        let mut literal = String::new();
        let mut terminated = false;

        while let Some(c) = self.peek() {
            if self.at_text_end(multiline) {
                if multiline {
                    self.bump();
                    self.bump();
                    self.bump();
                } else {
                    self.bump();
                }
                terminated = true;
                break;
            }
            if self.at_newline() {
                if !multiline {
                    break;
                }
                self.eat_newline();
                literal.push('\n');
                continue;
            }
            match c {
                '\\' => {
                    self.bump();
                    let escaped = self.bump().unwrap_or('\\');
                    literal.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                }
                '{' if self.peek_at(1) == Some('{') => {
                    self.bump();
                    self.bump();
                    literal.push('{');
                }
                '}' if self.peek_at(1) == Some('}') => {
                    self.bump();
                    self.bump();
                    literal.push('}');
                }
                '{' => {
                    self.bump();
                    if !literal.is_empty() {
                        parts.push(TextPart::Literal(std::mem::take(&mut literal)));
                    }
                    parts.push(self.lex_interpolation());
                }
                _ => {
                    self.bump();
                    literal.push(c);
                }
            }
        }

        if !literal.is_empty() {
            parts.push(TextPart::Literal(literal));
        }
        let span = Span::new(start, self.pos());
        if !terminated {
            self.error(ErrorCode::Syntax, "unterminated text literal", span);
        }
        self.tokens.push(Token::new(
            TokenKind::Text(TextLit { parts, multiline }),
            span,
        ));
    }

    fn at_text_end(&self, multiline: bool) -> bool {
        if multiline {
            self.peek() == Some('"') && self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"')
        } else {
            self.peek() == Some('"')
        }
    }

    /// Captures a `{expr}` hole as raw source. The parser re-lexes it, so the
    /// only structure tracked here is brace nesting and nested text literals.
    fn lex_interpolation(&mut self) -> TextPart {
        let start = self.pos();
        let mut source = String::new();
        let mut depth = 1u32;
        let mut in_text = false;

        while let Some(c) = self.peek() {
            if c == '"' {
                in_text = !in_text;
            } else if !in_text {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            source.push(c);
            self.bump();
        }
        let span = Span::new(start, self.pos());
        if depth == 0 {
            self.bump();
        } else {
            self.error(ErrorCode::Syntax, "unterminated interpolation", span);
        }
        TextPart::Interpolation { source, span }
    }
}

#[derive(PartialEq, Eq)]
enum LineKind {
    Blank,
    Code,
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
