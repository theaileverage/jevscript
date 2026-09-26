//! Lexer, parser and AST for the Jevscript language.
//!
//! `spec/jevscript-language-specification.md` is the authority. Section 2 is the
//! lexical structure and section 13 is the grammar; every type here mirrors a
//! production in one of them, and the doc comments name the section they come
//! from.
//!
//! Status: the lexer and the parser are implemented.
//!
//! ```
//! use jevscript_syntax::lex;
//!
//! let tokens = lex("program inbox_triage\n").expect("lexes");
//! assert!(!tokens.is_empty());
//! ```

#![forbid(unsafe_code)]

pub mod ast;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

pub use ast::Program;
pub use diagnostic::{Diagnostic, ErrorCode, RelatedLocation, Severity};
pub use lexer::{Lexed, lex, lex_all};
pub use parser::{Parser, parse, parse_expression, parse_expression_in, parse_tokens};
pub use span::{Pos, Span};
pub use token::{Keyword, Op, TextLit, TextPart, Token, TokenKind};
