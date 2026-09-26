//! Judgment expressions: spec section 6, productions `judge_expr` through
//! `detail_item` in section 13, and the checks the parser owns on them.

use super::{PResult, Parser};
use crate::ast::{Detail, Expr, JudgeExpr, JudgeVerb, PickLabel, RateLevel, Subject, SubjectStep};
use crate::diagnostic::ErrorCode;
use crate::span::{Pos, Span};
use crate::token::{Keyword, Op, TokenKind};

/// Fewest and most labels a `pick` may have, including the escape (spec 6.3).
const PICK_LABELS: std::ops::RangeInclusive<usize> = 2..=8;
/// Fewest and most levels a `rate` may have (spec 6.4).
const RATE_LEVELS: std::ops::RangeInclusive<usize> = 2..=10;

/// Words that name a degree instead of describing a situation. A level whose
/// whole text is one of these is `rate_bare_degree` (spec section 6.4).
const DEGREE_WORDS: [&str; 22] = [
    "low",
    "medium",
    "high",
    "very low",
    "very high",
    "lowest",
    "highest",
    "moderate",
    "middle",
    "mid",
    "minimal",
    "maximal",
    "minimum",
    "maximum",
    "mild",
    "severe",
    "extreme",
    "extremely low",
    "extremely high",
    "none",
    "some",
    "many",
];

impl Parser<'_> {
    /// Whether the current token is a judgment verb (spec section 6).
    pub(super) fn at_judge_verb(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Keyword(Keyword::Feels | Keyword::Pick | Keyword::Rate)
        )
    }

    /// `judge_expr = [ "each" ] subject judge_verb
    /// | subject "pick" "among" TEXT [ ":" NEWLINE INDENT [ "by" NAME NEWLINE ] [ "none" NEWLINE ] DEDENT ]`.
    pub(super) fn judge_expr(&mut self) -> PResult<JudgeExpr> {
        let start = self.span().start;
        let each = self.eat_kw(Keyword::Each);
        let subject = self.expr()?;
        self.judge_rest(each, subject, start)
    }

    /// The verb and detail after a subject that was parsed as an expression.
    ///
    /// `subject = NAME { "." NAME | "[" expr "]" }` and nothing else: the
    /// subject becomes a state path Jev is told to inspect by name, so any
    /// other expression is `subject_not_path` (spec section 6.1).
    pub(super) fn judge_rest(
        &mut self,
        each: bool,
        subject: Expr,
        start: Pos,
    ) -> PResult<JudgeExpr> {
        let subject = subject_from_expr(&subject).ok_or_else(|| {
            self.error_at(
                ErrorCode::SubjectNotPath,
                "a judgment subject must be a variable or a field path; compute first, then judge (spec section 6.1)",
                subject.span(),
            )
        })?;
        let (verb, detail) = match self.kind() {
            TokenKind::Keyword(Keyword::Feels) => {
                self.bump();
                let condition = self.text_expr("the condition text after `feels`")?;
                let detail = if self.at_op(Op::Colon) {
                    Some(self.detail_block()?)
                } else {
                    None
                };
                (JudgeVerb::Feels { condition }, detail)
            }
            TokenKind::Keyword(Keyword::Pick) => {
                self.bump();
                if self.eat_kw(Keyword::Among) {
                    (self.pick_among()?, None)
                } else {
                    self.pick_labels(start)?
                }
            }
            TokenKind::Keyword(Keyword::Rate) => {
                self.bump();
                self.rate_levels(start)?
            }
            _ => return Err(self.expected("a judgment verb (`feels`, `pick` or `rate`)")),
        };
        Ok(JudgeExpr {
            each,
            subject,
            verb,
            detail,
            span: Span::new(start, self.prev_end()),
        })
    }

    /// `"pick" "among" TEXT [ ":" NEWLINE INDENT [ "by" NAME NEWLINE ] [ "none" NEWLINE ] DEDENT ]`
    /// (spec section 6.4a). `pick among` was consumed by the caller.
    fn pick_among(&mut self) -> PResult<JudgeVerb> {
        let question = self.text_expr("the question text after `pick among`")?;
        let mut by = None;
        let mut allow_none = false;
        if self.eat_op(Op::Colon) {
            self.open_block("after `pick among ...:`")?;
            if self.eat_word("by") {
                by = Some(self.expect_name("the description field's name after `by`")?);
                self.expect_newline()?;
            }
            if self.eat_kw(Keyword::None) {
                allow_none = true;
                self.expect_newline()?;
            }
            if self.block_continues() {
                return Err(self.expected(
                    "`by <field>` or `none`, in that order (a `pick among` block holds nothing else)",
                ));
            }
            self.close_block()?;
        }
        Ok(JudgeVerb::PickAmong {
            question,
            by,
            allow_none,
        })
    }

    /// `"pick" ":" NEWLINE INDENT { pick_label } DEDENT` and the checks of
    /// spec section 6.3: two to eight labels (`pick_arity`), exactly one bare
    /// `other` or `none` (`pick_no_other`). `pick` was consumed by the caller.
    fn pick_labels(&mut self, start: Pos) -> PResult<(JudgeVerb, Option<Detail>)> {
        self.expect_op(Op::Colon)?;
        let header = Span::new(start, self.prev_end());
        self.open_block("of labels after `pick:`")?;
        let mut labels = Vec::new();
        let mut detail = Detail::default();
        while self.block_continues() {
            if self.shared_detail_line(&mut detail)? {
                continue;
            }
            labels.push(self.pick_label()?);
        }
        self.close_block()?;

        let escapes = labels.iter().filter(|l| l.escape).count();
        if escapes != 1 {
            return Err(self.error_at(
                ErrorCode::PickNoOther,
                format!(
                    "a `pick` needs exactly one bare `other` or `none` label as its escape option; found {escapes} (spec section 6.3)"
                ),
                header,
            ));
        }
        if !PICK_LABELS.contains(&labels.len()) {
            return Err(self.error_at(
                ErrorCode::PickArity,
                format!(
                    "a `pick` has two to eight labels including the escape; found {} (spec section 6.3)",
                    labels.len()
                ),
                header,
            ));
        }
        let detail = finish_detail(detail);
        Ok((JudgeVerb::Pick { labels }, detail))
    }

    /// `pick_label = NAME [ TEXT | ":" detail_block ] NEWLINE | ( "other" | "none" ) NEWLINE`.
    /// The grammar writes the block form as `NAME ":" detail_block` with the
    /// block's own leading `:` on top; one colon is what section 6.3 shows.
    fn pick_label(&mut self) -> PResult<PickLabel> {
        let start = self.span().start;
        if matches!(
            self.kind(),
            TokenKind::Keyword(Keyword::Other | Keyword::None)
        ) {
            let name = self.expect_word_or_keyword("a label")?;
            if !self.at_newline() {
                return Err(self.expected(&format!(
                    "the end of the line (the escape label `{}` is written bare, spec section 6.3)",
                    name.name
                )));
            }
            self.expect_newline()?;
            return Ok(PickLabel {
                span: name.span,
                name,
                escape: true,
                description: None,
                detail: None,
            });
        }
        let name =
            self.expect_name("a label (an identifier, or the bare escape `other` or `none`)")?;
        let (description, detail) = match self.kind() {
            TokenKind::Text(_) => (Some(self.text_expr("the label's description")?), None),
            TokenKind::Op(Op::Colon) => (None, Some(self.detail_block()?)),
            _ => (None, None),
        };
        let span = Span::new(start, self.prev_end());
        self.end_of_line()?;
        Ok(PickLabel {
            name,
            escape: false,
            description,
            detail,
            span,
        })
    }

    /// `"rate" ":" NEWLINE INDENT { rate_level } DEDENT` and the checks of spec
    /// section 6.4: two to ten levels (`rate_arity`), and no level whose text
    /// is only a number or a degree word (`rate_bare_degree`). `rate` was
    /// consumed by the caller.
    fn rate_levels(&mut self, start: Pos) -> PResult<(JudgeVerb, Option<Detail>)> {
        self.expect_op(Op::Colon)?;
        let header = Span::new(start, self.prev_end());
        self.open_block("of levels after `rate:`")?;
        let mut levels = Vec::new();
        let mut detail = Detail::default();
        while self.block_continues() {
            if self.shared_detail_line(&mut detail)? {
                continue;
            }
            levels.push(self.rate_level()?);
        }
        self.close_block()?;

        if !RATE_LEVELS.contains(&levels.len()) {
            return Err(self.error_at(
                ErrorCode::RateArity,
                format!(
                    "a `rate` has two to ten levels; found {} (spec section 6.4)",
                    levels.len()
                ),
                header,
            ));
        }
        for level in &levels {
            if let Expr::Text { value, .. } = &level.situation
                && let Some(text) = value.as_plain()
                && is_bare_degree(text)
            {
                return Err(self.error_at(
                    ErrorCode::RateBareDegree,
                    format!(
                        "level \"{text}\" is only a number or a degree word; describe the situation that stands alone (spec section 6.4)"
                    ),
                    level.span,
                ));
            }
        }
        let detail = finish_detail(detail);
        Ok((JudgeVerb::Rate { levels }, detail))
    }

    /// `rate_level = [ NAME ] TEXT NEWLINE`.
    fn rate_level(&mut self) -> PResult<RateLevel> {
        let start = self.span().start;
        let name = if matches!(self.kind(), TokenKind::Name(_)) {
            Some(self.expect_name("a level name")?)
        } else {
            None
        };
        let situation = self.text_expr("a level's situation text")?;
        let span = Span::new(start, self.prev_end());
        self.expect_newline()?;
        Ok(RateLevel {
            name,
            situation,
            span,
        })
    }

    /// `detail_block = ":" NEWLINE INDENT { detail_item } DEDENT` (spec section 6.8).
    fn detail_block(&mut self) -> PResult<Detail> {
        let start = self.span().start;
        self.expect_op(Op::Colon)?;
        self.open_block("of detail keys after `:`")?;
        let mut detail = Detail::default();
        let mut end = self.prev_end();
        while self.block_continues() {
            self.detail_item(&mut detail)?;
            end = self.prev_end_before_newline();
        }
        self.close_block()?;
        detail.span = Span::new(start, end);
        Ok(detail)
    }

    /// `detail_item = ( "focus" | "note" ) TEXT NEWLINE | "sample" ( "true" | "false" ) NEWLINE
    /// | "compare" list NEWLINE | ( "yes" | "no" | "examples" ) list NEWLINE
    /// | ( "what" | "not_for" ) TEXT NEWLINE`. `focus` is reserved; the other
    /// keys are contextual words. A key given twice is an error.
    fn detail_item(&mut self, detail: &mut Detail) -> PResult<()> {
        let key_span = self.span();
        let key = match self.kind() {
            TokenKind::Keyword(Keyword::Focus) => "focus".to_string(),
            TokenKind::Name(name) => name.clone(),
            _ => return Err(self.expected(DETAIL_KEYS)),
        };
        self.bump();
        let taken = match key.as_str() {
            "focus" => set(&mut detail.focus, self.text_expr("the `focus` text")?),
            "note" => set(&mut detail.note, self.text_expr("the `note` text")?),
            "what" => set(&mut detail.what, self.text_expr("the `what` text")?),
            "not_for" => set(&mut detail.not_for, self.text_expr("the `not_for` text")?),
            "compare" => set(&mut detail.compare, self.list_value("compare")?),
            "yes" => set(&mut detail.yes, self.list_value("yes")?),
            "no" => set(&mut detail.no, self.list_value("no")?),
            "examples" => set(&mut detail.examples, self.list_value("examples")?),
            "sample" => set(&mut detail.sample, self.bool_value("sample")?),
            _ => {
                return Err(self.error_at(
                    ErrorCode::Syntax,
                    format!("expected {DETAIL_KEYS}, found `{key}`"),
                    key_span,
                ));
            }
        };
        if !taken {
            return Err(self.error_at(
                ErrorCode::DuplicateName,
                format!("the detail key `{key}` is given twice (spec section 12)"),
                key_span,
            ));
        }
        self.expect_newline()
    }

    /// A detail item that may also be written inside a `pick:` or `rate:`
    /// block. Section 6.8 says every verb takes detail and section 6.11 puts
    /// `sample true` on `pick` and `rate`, but the grammar in section 13 gives
    /// those two verbs no detail block. The keys that apply to every verb and
    /// cannot be confused with a label or level line are accepted here:
    /// `focus` (reserved), `compare` before `[`, and `sample` before a bool.
    /// `note` is not, because `note "..."` is also a label. Returns whether a
    /// line was consumed.
    fn shared_detail_line(&mut self, detail: &mut Detail) -> PResult<bool> {
        let shared = match self.kind() {
            TokenKind::Keyword(Keyword::Focus) => true,
            TokenKind::Name(n) if n == "compare" => self.kind_at(1) == &TokenKind::Op(Op::LBracket),
            TokenKind::Name(n) if n == "sample" => matches!(
                self.kind_at(1),
                TokenKind::Keyword(Keyword::True | Keyword::False)
            ),
            _ => false,
        };
        if shared {
            let start = self.span().start;
            self.detail_item(detail)?;
            if detail.span == Span::default() {
                detail.span = Span::new(start, start);
            }
            detail.span = Span::new(detail.span.start, self.prev_end_before_newline());
        }
        Ok(shared)
    }

    /// `list`: a `[...]` literal, as `compare`, `yes`, `no` and `examples` take.
    fn list_value(&mut self, key: &str) -> PResult<Expr> {
        if !self.at_op(Op::LBracket) {
            return Err(self.expected(&format!("a `[...]` list after `{key}`")));
        }
        let value = self.expr()?;
        if matches!(value, Expr::List { .. }) {
            Ok(value)
        } else {
            Err(self.error_at(
                ErrorCode::Syntax,
                format!("`{key}` takes a list literal (spec section 6.8)"),
                value.span(),
            ))
        }
    }

    /// `"true" | "false"`, as `sample` takes.
    fn bool_value(&mut self, key: &str) -> PResult<bool> {
        match self.kind() {
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Ok(true)
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Ok(false)
            }
            _ => Err(self.expected(&format!("`true` or `false` after `{key}`"))),
        }
    }

    /// The end of the last token before the `NEWLINE` that was just consumed.
    fn prev_end_before_newline(&self) -> Pos {
        self.position
            .checked_sub(2)
            .and_then(|i| self.tokens.get(i))
            .map(|t| t.span.end)
            .unwrap_or_else(|| self.prev_end())
    }
}

const DETAIL_KEYS: &str = "a detail key (`focus`, `note`, `compare`, `yes`, `no`, `examples`, `what`, `not_for` or `sample`)";

/// Fills an empty detail slot; false if it was already filled.
fn set<T>(slot: &mut Option<T>, value: T) -> bool {
    if slot.is_some() {
        return false;
    }
    *slot = Some(value);
    true
}

/// A detail collected from lines inside a `pick:` or `rate:` block, or none if
/// no such line was written.
fn finish_detail(detail: Detail) -> Option<Detail> {
    (detail.span != Span::default()).then_some(detail)
}

/// Reads a path expression back as a judgment subject (spec section 6.1).
fn subject_from_expr(expr: &Expr) -> Option<Subject> {
    fn walk(expr: &Expr, path: &mut Vec<SubjectStep>) -> Option<crate::ast::Ident> {
        match expr {
            Expr::Name(ident) => Some(ident.clone()),
            Expr::Field { target, name, .. } => {
                let root = walk(target, path)?;
                path.push(SubjectStep::Field(name.clone()));
                Some(root)
            }
            Expr::Index { target, index, .. } => {
                let root = walk(target, path)?;
                path.push(SubjectStep::Index((**index).clone()));
                Some(root)
            }
            _ => None,
        }
    }
    let mut path = Vec::new();
    let root = walk(expr, &mut path)?;
    Some(Subject {
        root,
        path,
        span: expr.span(),
    })
}

/// Whether a level's text is only a number or a degree word (spec section 6.4).
fn is_bare_degree(text: &str) -> bool {
    let normalized: String = text
        .trim()
        .trim_end_matches(['.', '!'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let numeric = normalized.trim_end_matches('%').parse::<f64>().is_ok();
    numeric || DEGREE_WORDS.contains(&normalized.as_str())
}
