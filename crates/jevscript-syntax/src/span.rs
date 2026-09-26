//! Source positions.
//!
//! Every AST and IR node carries a [`Span`] so that pauses and errors point
//! back at lines (spec section 11.1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A position in a source file. Lines are 1-based, columns 0-based, both in
/// characters rather than bytes so that diagnostics line up with what an editor
/// shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Pos {
    /// 1-based line number.
    pub line: u32,
    /// 0-based column, in characters.
    pub column: u32,
    /// 0-based byte offset into the source.
    pub offset: u32,
}

impl Pos {
    /// A position at the given line, column and byte offset.
    pub const fn new(line: u32, column: u32, offset: u32) -> Self {
        Self {
            line,
            column,
            offset,
        }
    }
}

/// A half-open source range, `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Span {
    /// First position covered by the span.
    pub start: Pos,
    /// One past the last position covered by the span.
    pub end: Pos,
}

impl Span {
    /// A span covering `start..end`.
    pub const fn new(start: Pos, end: Pos) -> Self {
        Self { start, end }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn join(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}
