//! Tokens.
//!
//! Indentation is handled by the lexer, which emits [`TokenKind::Indent`],
//! [`TokenKind::Dedent`] and [`TokenKind::Newline`] as Python's does (spec
//! sections 2.2 and 13).

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::span::Span;

/// A reserved word (spec section 2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the keyword it names")]
pub enum Keyword {
    Program,
    Use,
    In,
    Out,
    Needs,
    Judgment,
    Task,
    Def,
    Machine,
    Return,
    Feels,
    Pick,
    Rate,
    Each,
    Other,
    Among,
    On,
    Shape,
    Focus,
    Trail,
    Max,
    State,
    When,
    Stay,
    Initial,
    Until,
    Loop,
    For,
    If,
    Elif,
    Else,
    And,
    Or,
    Not,
    Is,
    Continue,
    Break,
    Stop,
    Escalate,
    Gate,
    Proceed,
    Confirm,
    Budget,
    True,
    False,
    None,
}

impl Keyword {
    /// The reserved words, in the order the spec lists them.
    pub const ALL: [(&'static str, Keyword); 46] = [
        ("program", Keyword::Program),
        ("use", Keyword::Use),
        ("in", Keyword::In),
        ("out", Keyword::Out),
        ("needs", Keyword::Needs),
        ("judgment", Keyword::Judgment),
        ("task", Keyword::Task),
        ("def", Keyword::Def),
        ("machine", Keyword::Machine),
        ("return", Keyword::Return),
        ("feels", Keyword::Feels),
        ("pick", Keyword::Pick),
        ("rate", Keyword::Rate),
        ("each", Keyword::Each),
        ("other", Keyword::Other),
        ("among", Keyword::Among),
        ("on", Keyword::On),
        ("shape", Keyword::Shape),
        ("focus", Keyword::Focus),
        ("trail", Keyword::Trail),
        ("max", Keyword::Max),
        ("state", Keyword::State),
        ("when", Keyword::When),
        ("stay", Keyword::Stay),
        ("initial", Keyword::Initial),
        ("until", Keyword::Until),
        ("loop", Keyword::Loop),
        ("for", Keyword::For),
        ("if", Keyword::If),
        ("elif", Keyword::Elif),
        ("else", Keyword::Else),
        ("and", Keyword::And),
        ("or", Keyword::Or),
        ("not", Keyword::Not),
        ("is", Keyword::Is),
        ("continue", Keyword::Continue),
        ("break", Keyword::Break),
        ("stop", Keyword::Stop),
        ("escalate", Keyword::Escalate),
        ("gate", Keyword::Gate),
        ("proceed", Keyword::Proceed),
        ("confirm", Keyword::Confirm),
        ("budget", Keyword::Budget),
        ("true", Keyword::True),
        ("false", Keyword::False),
        ("none", Keyword::None),
    ];

    /// The keyword this word spells, if it is reserved.
    pub fn from_word(word: &str) -> Option<Keyword> {
        Self::ALL
            .iter()
            .find(|(text, _)| *text == word)
            .map(|(_, kw)| *kw)
    }

    /// How the keyword is written.
    pub const fn as_str(self) -> &'static str {
        // `ALL` is the single source of truth, but `const fn` cannot iterate it.
        match self {
            Keyword::Program => "program",
            Keyword::Use => "use",
            Keyword::In => "in",
            Keyword::Out => "out",
            Keyword::Needs => "needs",
            Keyword::Judgment => "judgment",
            Keyword::Task => "task",
            Keyword::Def => "def",
            Keyword::Machine => "machine",
            Keyword::Return => "return",
            Keyword::Feels => "feels",
            Keyword::Pick => "pick",
            Keyword::Rate => "rate",
            Keyword::Each => "each",
            Keyword::Other => "other",
            Keyword::Among => "among",
            Keyword::On => "on",
            Keyword::Shape => "shape",
            Keyword::Focus => "focus",
            Keyword::Trail => "trail",
            Keyword::Max => "max",
            Keyword::State => "state",
            Keyword::When => "when",
            Keyword::Stay => "stay",
            Keyword::Initial => "initial",
            Keyword::Until => "until",
            Keyword::Loop => "loop",
            Keyword::For => "for",
            Keyword::If => "if",
            Keyword::Elif => "elif",
            Keyword::Else => "else",
            Keyword::And => "and",
            Keyword::Or => "or",
            Keyword::Not => "not",
            Keyword::Is => "is",
            Keyword::Continue => "continue",
            Keyword::Break => "break",
            Keyword::Stop => "stop",
            Keyword::Escalate => "escalate",
            Keyword::Gate => "gate",
            Keyword::Proceed => "proceed",
            Keyword::Confirm => "confirm",
            Keyword::Budget => "budget",
            Keyword::True => "true",
            Keyword::False => "false",
            Keyword::None => "none",
        }
    }
}

/// Words that are keywords only in the positions the spec names, and ordinary
/// identifiers everywhere else (spec section 2.5): `use ... as ... with ...`,
/// `llm.write ... using ...`, `count(..., above ...)`, `spawn ... prompt ...`,
/// `pick among ... by <field>`, a machine's `goal` / `observe` headers, a
/// transition's `risky` marker, a `done` state, the `sample` detail key and
/// `log` before a level word (spec section 5.8). The lexer emits them as
/// [`TokenKind::Name`]; the parser decides.
pub const CONTEXTUAL_KEYWORDS: [&str; 13] = [
    "as", "with", "using", "above", "below", "by", "prompt", "done", "risky", "sample", "goal",
    "observe", "log",
];

/// The four levels a `log` takes, lowest first (spec section 5.8). Each is a
/// contextual word, special only directly after `log`.
pub const LOG_LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];

/// A punctuation or operator token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(missing_docs, reason = "each variant is the symbol it names")]
pub enum Op {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Arrow,
    Assign,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
}

impl Op {
    /// How the operator is written.
    pub const fn as_str(self) -> &'static str {
        match self {
            Op::LParen => "(",
            Op::RParen => ")",
            Op::LBracket => "[",
            Op::RBracket => "]",
            Op::LBrace => "{",
            Op::RBrace => "}",
            Op::Comma => ",",
            Op::Colon => ":",
            Op::Dot => ".",
            Op::Arrow => "->",
            Op::Assign => "=",
            Op::Eq => "==",
            Op::NotEq => "!=",
            Op::Lt => "<",
            Op::LtEq => "<=",
            Op::Gt => ">",
            Op::GtEq => ">=",
            Op::Plus => "+",
            Op::Minus => "-",
            Op::Star => "*",
            Op::Slash => "/",
            Op::Percent => "%",
        }
    }
}

/// A piece of a text literal. Interpolations are kept as raw source; they are
/// parsed as expressions by the parser (spec section 2.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextPart {
    /// Literal characters, with escapes and `{{` / `}}` already resolved.
    Literal(String),
    /// A `{expr}` hole: the source between the braces, and its span.
    Interpolation {
        /// The expression source, without the braces.
        source: String,
        /// Where the expression sits in the file.
        span: Span,
    },
}

/// A text literal, split into its literal and interpolated parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextLit {
    /// The parts, in order.
    pub parts: Vec<TextPart>,
    /// Whether the literal was written with `"""`.
    pub multiline: bool,
}

impl TextLit {
    /// The literal's text, if it has no interpolations.
    pub fn as_plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [] => Some(""),
            [TextPart::Literal(text)] => Some(text),
            _ => None,
        }
    }
}

/// What a token is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TokenKind {
    /// An identifier: `[a-z_][a-z0-9_]*` (spec section 2.4).
    Name(String),
    /// A reserved word.
    Keyword(Keyword),
    /// A number. `2k` is 2000 and `80%` is 0.8 by the time it gets here.
    Number(f64),
    /// A text literal.
    Text(TextLit),
    /// Punctuation or an operator.
    Op(Op),
    /// End of a logical line.
    Newline,
    /// Start of a deeper block.
    Indent,
    /// End of a block.
    Dedent,
    /// End of the file.
    Eof,
}

/// A token and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    /// What the token is.
    pub kind: TokenKind,
    /// Where it sits in the source.
    pub span: Span,
}

impl Token {
    /// A token of `kind` covering `span`.
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// The identifier this token spells, if it is a [`TokenKind::Name`].
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Name(name) => Some(name),
            _ => None,
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Name(name) => write!(f, "{name}"),
            TokenKind::Keyword(kw) => write!(f, "{}", kw.as_str()),
            TokenKind::Number(value) => write!(f, "{value}"),
            TokenKind::Text(_) => f.write_str("text"),
            TokenKind::Op(op) => write!(f, "{}", op.as_str()),
            TokenKind::Newline => f.write_str("newline"),
            TokenKind::Indent => f.write_str("indent"),
            TokenKind::Dedent => f.write_str("dedent"),
            TokenKind::Eof => f.write_str("end of file"),
        }
    }
}
