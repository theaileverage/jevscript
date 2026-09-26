//! Byte offsets to editor positions and back.
//!
//! The front end reports positions as a 1-based line, a column in characters
//! and a byte offset (spec section 11.1). LSP positions are a 0-based line and
//! a column in the negotiated encoding, UTF-16 unless the client offers
//! another. Every conversion here goes through the byte offset, which is the
//! one coordinate both sides agree on exactly.

use lsp_types::{Position, PositionEncodingKind, Range};

use jevscript_syntax::Span;

/// How a column counts characters, as negotiated at `initialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Columns count bytes.
    Utf8,
    /// Columns count UTF-16 code units, the LSP default.
    Utf16,
    /// Columns count Unicode scalar values.
    Utf32,
}

impl Encoding {
    /// The first encoding in the client's preference list that the server
    /// supports, or UTF-16, which every client must accept.
    pub fn negotiate(offered: Option<&[PositionEncodingKind]>) -> Self {
        for kind in offered.unwrap_or_default() {
            if *kind == PositionEncodingKind::UTF8 {
                return Encoding::Utf8;
            }
            if *kind == PositionEncodingKind::UTF32 {
                return Encoding::Utf32;
            }
            if *kind == PositionEncodingKind::UTF16 {
                return Encoding::Utf16;
            }
        }
        Encoding::Utf16
    }

    /// The kind to announce in the server's capabilities.
    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
            Encoding::Utf32 => PositionEncodingKind::UTF32,
        }
    }

    fn width(self, c: char) -> u32 {
        match self {
            Encoding::Utf8 => c.len_utf8() as u32,
            Encoding::Utf16 => c.len_utf16() as u32,
            Encoding::Utf32 => 1,
        }
    }
}

/// Where each line of a text starts.
#[derive(Debug, Clone)]
pub struct LineIndex {
    starts: Vec<usize>,
    encoding: Encoding,
}

impl LineIndex {
    /// Index `text`. Lines end with LF (spec section 2.1); a CR before it is
    /// an ordinary character of the line.
    pub fn new(text: &str, encoding: Encoding) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { starts, encoding }
    }

    /// The encoding columns are counted in.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// How many lines the text has, counting a final empty one.
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The byte offset where 0-based `line` starts, clamped to the text.
    pub fn line_start(&self, line: usize) -> usize {
        self.starts
            .get(line)
            .copied()
            .unwrap_or_else(|| *self.starts.last().unwrap_or(&0))
    }

    /// The 0-based line holding byte `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }

    /// The editor position of byte `offset` in `text`.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = floor_char_boundary(text, offset.min(text.len()));
        let line = self.line_of(offset);
        let start = self.line_start(line);
        let column: u32 = text[start..offset]
            .chars()
            .map(|c| self.encoding.width(c))
            .sum();
        Position::new(line as u32, column)
    }

    /// The byte offset of an editor position in `text`. A column past the end
    /// of its line is the end of the line, and a line past the end of the
    /// text is the end of the text, as the protocol asks.
    pub fn offset(&self, text: &str, position: Position) -> usize {
        let line = position.line as usize;
        if line >= self.starts.len() {
            return text.len();
        }
        let start = self.starts[line];
        let end = self
            .starts
            .get(line + 1)
            .map_or(text.len(), |next| next - 1);
        let mut column = 0u32;
        for (i, c) in text[start..end].char_indices() {
            if column >= position.character {
                return start + i;
            }
            column += self.encoding.width(c);
        }
        end
    }

    /// The editor range of a front-end span.
    pub fn range(&self, text: &str, span: Span) -> Range {
        self.range_of(text, span.start.offset as usize, span.end.offset as usize)
    }

    /// The editor range of the bytes `start..end`.
    pub fn range_of(&self, text: &str, start: usize, end: usize) -> Range {
        Range::new(
            self.position(text, start),
            self.position(text, end.max(start)),
        )
    }
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns_count_surrogate_pairs_twice() {
        // LSP positions default to UTF-16; the front end counts characters.
        let text = "x = \"🙂é\"\ny\n";
        let index = LineIndex::new(text, Encoding::Utf16);
        let quote = text.rfind('"').expect("closing quote");
        assert_eq!(index.position(text, quote), Position::new(0, 8));
        assert_eq!(index.offset(text, Position::new(0, 8)), quote);
        assert_eq!(
            index.position(text, text.find('y').unwrap()),
            Position::new(1, 0)
        );
    }

    #[test]
    fn utf8_and_utf32_columns() {
        let text = "é = 1";
        let eq = text.find('=').unwrap();
        let utf8 = LineIndex::new(text, Encoding::Utf8);
        let utf32 = LineIndex::new(text, Encoding::Utf32);
        assert_eq!(utf8.position(text, eq), Position::new(0, 3));
        assert_eq!(utf32.position(text, eq), Position::new(0, 2));
        assert_eq!(utf32.offset(text, Position::new(0, 2)), eq);
    }

    #[test]
    fn positions_past_the_end_clamp() {
        let text = "ab\ncd";
        let index = LineIndex::new(text, Encoding::Utf16);
        assert_eq!(index.offset(text, Position::new(0, 99)), 2);
        assert_eq!(index.offset(text, Position::new(9, 0)), text.len());
    }

    #[test]
    fn negotiation_prefers_the_clients_first_supported_choice() {
        assert_eq!(Encoding::negotiate(None), Encoding::Utf16);
        assert_eq!(
            Encoding::negotiate(Some(&[PositionEncodingKind::UTF8])),
            Encoding::Utf8
        );
        assert_eq!(
            Encoding::negotiate(Some(&[
                PositionEncodingKind::new("utf-7"),
                PositionEncodingKind::UTF32
            ])),
            Encoding::Utf32
        );
    }
}
