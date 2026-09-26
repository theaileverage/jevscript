//! Expressions: spec sections 2.6, 2.7, 5.1, 5.2, 5.7, 7.3, 7.5 and 8.2,
//! productions `expr` through `comprehension` in section 13.

use super::{PResult, Parser, parse_expression_in};
use crate::ast::{Arg, BinaryOp, CallForm, Expr, Ident, LogLevel, RecordField, UnaryOp};
use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::span::Span;
use crate::token::{Keyword, Op, TextLit, TextPart, TokenKind};

impl Parser<'_> {
    /// `expr = or_expr`. The functions below are the precedence levels of
    /// spec section 5.1, lowest first.
    pub(super) fn expr(&mut self) -> PResult<Expr> {
        self.or_expr()
    }

    /// `or_expr = and_expr { "or" and_expr }`.
    fn or_expr(&mut self) -> PResult<Expr> {
        let mut left = self.and_expr()?;
        while self.eat_kw(Keyword::Or) {
            let right = self.and_expr()?;
            left = binary(BinaryOp::Or, left, right);
        }
        Ok(left)
    }

    /// `and_expr = not_expr { "and" not_expr }`.
    fn and_expr(&mut self) -> PResult<Expr> {
        let mut left = self.not_expr()?;
        while self.eat_kw(Keyword::And) {
            let right = self.not_expr()?;
            left = binary(BinaryOp::And, left, right);
        }
        Ok(left)
    }

    /// `not_expr = "not" not_expr | cmp_expr`. `not` binds looser than a
    /// comparison, so `not a == b` is `not (a == b)`.
    fn not_expr(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        if self.eat_kw(Keyword::Not) {
            let operand = self.not_expr()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                span: Span::new(start, operand.span().end),
                operand: Box::new(operand),
            });
        }
        self.cmp_expr()
    }

    /// `cmp_expr = add_expr [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) add_expr | "is" NAME ]`.
    /// Comparisons do not chain. The label after `is` may be the bare escape
    /// word `other` or `none`, which are reserved (spec sections 4.3 and 6.3).
    fn cmp_expr(&mut self) -> PResult<Expr> {
        let left = self.add_expr()?;
        if self.eat_kw(Keyword::Is) {
            let label = match self.kind() {
                TokenKind::Name(_) | TokenKind::Keyword(Keyword::Other | Keyword::None) => {
                    self.expect_word_or_keyword("a label")?
                }
                _ => return Err(self.expected("a label name after `is`")),
            };
            return Ok(Expr::Is {
                span: Span::new(left.span().start, label.span.end),
                target: Box::new(left),
                label,
            });
        }
        let op = match self.kind() {
            TokenKind::Op(Op::Eq) => BinaryOp::Eq,
            TokenKind::Op(Op::NotEq) => BinaryOp::NotEq,
            TokenKind::Op(Op::Lt) => BinaryOp::Lt,
            TokenKind::Op(Op::LtEq) => BinaryOp::LtEq,
            TokenKind::Op(Op::Gt) => BinaryOp::Gt,
            TokenKind::Op(Op::GtEq) => BinaryOp::GtEq,
            _ => return Ok(left),
        };
        self.bump();
        let right = self.add_expr()?;
        Ok(binary(op, left, right))
    }

    /// `add_expr = mul_expr { ( "+" | "-" ) mul_expr }`.
    fn add_expr(&mut self) -> PResult<Expr> {
        let mut left = self.mul_expr()?;
        loop {
            let op = match self.kind() {
                TokenKind::Op(Op::Plus) => BinaryOp::Add,
                TokenKind::Op(Op::Minus) => BinaryOp::Sub,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.mul_expr()?;
            left = binary(op, left, right);
        }
    }

    /// `mul_expr = unary { ( "*" | "/" | "%" ) unary }`.
    fn mul_expr(&mut self) -> PResult<Expr> {
        let mut left = self.unary()?;
        loop {
            let op = match self.kind() {
                TokenKind::Op(Op::Star) => BinaryOp::Mul,
                TokenKind::Op(Op::Slash) => BinaryOp::Div,
                TokenKind::Op(Op::Percent) => BinaryOp::Rem,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.unary()?;
            left = binary(op, left, right);
        }
    }

    /// `unary = "-" unary | postfix`.
    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        if self.eat_op(Op::Minus) {
            let operand = self.unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                span: Span::new(start, operand.span().end),
                operand: Box::new(operand),
            });
        }
        self.postfix()
    }

    /// `postfix = primary { "." NAME | "[" expr "]" | "(" [ fn_args ] ")" }`.
    ///
    /// A field name may be a reserved word (`dev.stop`, spec section 9.1).
    /// `(` and `[` always continue the expression before them, so a
    /// command-form list or parenthesised argument needs function form:
    /// `dev.send([1, 2])`, not `dev.send [1, 2]`.
    fn postfix(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        let mut expr = self.primary()?;
        loop {
            if self.eat_op(Op::Dot) {
                let name = self.expect_word_or_keyword("a field name after `.`")?;
                expr = Expr::Field {
                    target: Box::new(expr),
                    span: Span::new(start, name.span.end),
                    name,
                };
            } else if self.eat_op(Op::LBracket) {
                let index = self.expr()?;
                self.expect_op(Op::RBracket)?;
                expr = Expr::Index {
                    target: Box::new(expr),
                    index: Box::new(index),
                    span: Span::new(start, self.prev_end()),
                };
            } else if self.eat_op(Op::LParen) {
                let args = self.function_args()?;
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                    form: CallForm::Function,
                    span: Span::new(start, self.prev_end()),
                };
            } else {
                return Ok(expr);
            }
        }
    }

    /// `primary = NUMBER | TEXT | "true" | "false" | "none" | NAME | list | record
    /// | "(" expr ")" | comprehension | "focus" expr "on" TEXT "," "max" NUMBER
    /// | "trail" NUMBER`.
    ///
    /// `max` is reserved (spec 2.5) and also the builtin `max(xs)` (5.7), so
    /// the keyword followed by `(` is read as that name.
    fn primary(&mut self) -> PResult<Expr> {
        let span = self.span();
        match self.kind() {
            TokenKind::Number(value) => {
                self.bump();
                Ok(Expr::Number {
                    value: *value,
                    span,
                })
            }
            TokenKind::Text(_) => self.text_expr("an expression"),
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Ok(Expr::Bool { value: true, span })
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Ok(Expr::Bool { value: false, span })
            }
            TokenKind::Keyword(Keyword::None) => {
                self.bump();
                Ok(Expr::None { span })
            }
            TokenKind::Name(_) if self.at_log_form() => self.log_expr(),
            TokenKind::Name(name) => {
                self.bump();
                Ok(Expr::Name(Ident {
                    name: name.clone(),
                    span,
                }))
            }
            TokenKind::Keyword(Keyword::Max) if self.kind_at(1) == &TokenKind::Op(Op::LParen) => {
                self.bump();
                Ok(Expr::Name(Ident {
                    name: "max".to_string(),
                    span,
                }))
            }
            TokenKind::Op(Op::LBracket) => self.list_or_comprehension(),
            TokenKind::Op(Op::LBrace) => self.record(),
            TokenKind::Op(Op::LParen) => {
                self.bump();
                let inner = self.expr()?;
                self.expect_op(Op::RParen)?;
                Ok(inner)
            }
            TokenKind::Keyword(Keyword::Focus) => self.focus_expr(),
            TokenKind::Keyword(Keyword::Trail) => {
                self.bump();
                let (count, _) = self.expect_number("the number of records after `trail`")?;
                Ok(Expr::Trail {
                    count,
                    span: Span::new(span.start, self.prev_end()),
                })
            }
            _ => Err(self.expected("an expression")),
        }
    }

    /// `"focus" expr "on" TEXT "," "max" NUMBER` (spec section 7.3). `on` and
    /// `max` are reserved, which is why `focus` is a form and not a def.
    fn focus_expr(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        self.expect_kw(Keyword::Focus)?;
        let text = self.expr()?;
        self.expect_kw(Keyword::On)?;
        let on = self.text_expr("the purpose text after `on`")?;
        if !(self.at_op(Op::Comma) && self.kind_at(1) == &TokenKind::Keyword(Keyword::Max)) {
            return Err(self.expected("`, max N` (a `focus` always states its token cap)"));
        }
        self.bump();
        self.bump();
        let (max, _) = self.expect_number("the token cap after `max`")?;
        Ok(Expr::Focus {
            text: Box::new(text),
            on: Box::new(on),
            max,
            span: Span::new(start, self.prev_end()),
        })
    }

    /// Whether the parser is at the start of a `log` form (spec section 5.8):
    /// the contextual word `log`, a word in the level position, and a token
    /// that can begin the logged expression. Anywhere else `log` is an
    /// ordinary identifier, so `log = 3`, `log.count` and `f(log)` keep their
    /// meaning. A word that is not a level is still read as one, and reported
    /// as `log_level`, because `log trace x` has no other valid reading. In a
    /// file that declares a unit named `log` there is no `log` form at all,
    /// so its existing command-form calls keep their meaning.
    pub(super) fn at_log_form(&self) -> bool {
        !self.log_is_unit
            && self.at_word("log")
            && matches!(self.kind_at(1), TokenKind::Name(_))
            && self.starts_operand(self.kind_at(2))
    }

    /// `"log" LEVEL expr [ record ]` (spec section 5.8).
    ///
    /// The logged expression extends as far to the right as an expression
    /// can, as `focus`'s does: `log debug a + b` logs `a + b`. It ends where
    /// an expression cannot continue, so a record literal after it is the
    /// log's fields rather than part of the value. Field names are unique,
    /// which is `duplicate_name` (spec section 12).
    fn log_expr(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        self.expect_word("log")?;
        let level_span = self.span();
        let word = self.expect_name("a log level")?;
        let level = LogLevel::from_word(&word.name).ok_or_else(|| {
            self.error_at(
                ErrorCode::LogLevel,
                format!(
                    "`{}` is not a log level; write `debug`, `info`, `warn` or `error` (spec section 5.8)",
                    word.name
                ),
                level_span,
            )
        })?;
        let value = self.expr()?;
        let mut fields = Vec::new();
        if self.at_op(Op::LBrace) {
            let Expr::Record {
                fields: written, ..
            } = self.record()?
            else {
                unreachable!("record() builds a record");
            };
            for (i, field) in written.iter().enumerate() {
                if written[..i].iter().any(|f| f.name.name == field.name.name) {
                    return Err(self.error_at(
                        ErrorCode::DuplicateName,
                        format!(
                            "the log field `{}` is given twice (spec section 12)",
                            field.name.name
                        ),
                        field.name.span,
                    ));
                }
            }
            fields = written;
        }
        Ok(Expr::Log {
            level,
            value: Box::new(value),
            fields,
            span: Span::new(start, self.prev_end()),
        })
    }

    /// Whether a token can begin an expression: a value, or `-`, `(`, `[`, or
    /// the builtin `max(`.
    fn starts_operand(&self, kind: &TokenKind) -> bool {
        self.starts_value(kind)
            || matches!(
                kind,
                TokenKind::Op(Op::Minus | Op::LParen | Op::LBracket)
                    | TokenKind::Keyword(Keyword::Max)
            )
    }

    /// `list = "[" [ expr { "," expr } ] "]"` and
    /// `comprehension = "[" expr "for" NAME [ "," NAME ] "in" expr [ "if" expr ] "]"`
    /// (spec sections 2.6 and 8.2), told apart by the `for` after the first
    /// element.
    fn list_or_comprehension(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        self.expect_op(Op::LBracket)?;
        if self.eat_op(Op::RBracket) {
            return Ok(Expr::List {
                items: Vec::new(),
                span: Span::new(start, self.prev_end()),
            });
        }
        let first = self.expr()?;
        if self.eat_kw(Keyword::For) {
            let names = self.loop_names()?;
            self.expect_kw(Keyword::In)?;
            let iterable = self.expr()?;
            let test = if self.eat_kw(Keyword::If) {
                Some(Box::new(self.expr()?))
            } else {
                None
            };
            self.expect_op(Op::RBracket)?;
            return Ok(Expr::Comprehension {
                expr: Box::new(first),
                names,
                iterable: Box::new(iterable),
                test,
                span: Span::new(start, self.prev_end()),
            });
        }
        let mut items = vec![first];
        while self.eat_op(Op::Comma) {
            items.push(self.expr()?);
        }
        self.expect_op(Op::RBracket)?;
        Ok(Expr::List {
            items,
            span: Span::new(start, self.prev_end()),
        })
    }

    /// `record = "{" [ rec_field { "," rec_field } ] "}"`, `rec_field = NAME [ ":" expr ]`
    /// (spec section 2.6). `{ title }` is shorthand for `{ title: title }`.
    /// A reserved word is accepted as a key only with a value, since the
    /// shorthand would name a variable that cannot exist.
    fn record(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        self.expect_op(Op::LBrace)?;
        let mut fields = Vec::new();
        if !self.at_op(Op::RBrace) {
            loop {
                let field_start = self.span().start;
                let is_keyword = matches!(self.kind(), TokenKind::Keyword(_));
                let name = self.expect_word_or_keyword("a field name")?;
                let value = if self.eat_op(Op::Colon) {
                    Some(self.expr()?)
                } else if is_keyword {
                    return Err(self.expected(&format!(
                        "`:` (`{{ {} }}` shorthand needs a variable, and `{}` is reserved)",
                        name.name, name.name
                    )));
                } else {
                    None
                };
                fields.push(RecordField {
                    name,
                    value,
                    span: Span::new(field_start, self.prev_end()),
                });
                if !self.eat_op(Op::Comma) {
                    break;
                }
            }
        }
        self.expect_op(Op::RBrace)?;
        Ok(Expr::Record {
            fields,
            span: Span::new(start, self.prev_end()),
        })
    }

    /* ---------------------------------------------------------------- */
    /* arguments                                                         */
    /* ---------------------------------------------------------------- */

    /// `fn_args = fn_arg { "," fn_arg }` up to and including the closing `)`;
    /// `fn_arg = NAME ":" expr | NAME expr | expr` (spec sections 5.2 and 13).
    fn function_args(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        if !self.at_op(Op::RParen) {
            loop {
                args.push(self.call_arg(CallForm::Function)?);
                if !self.eat_op(Op::Comma) {
                    break;
                }
            }
        }
        self.expect_op(Op::RParen)?;
        Ok(args)
    }

    /// `cmd_args = cmd_arg { "," cmd_arg }`, `cmd_arg = NAME expr | expr`
    /// (spec section 5.2).
    ///
    /// Two departures from the production, both from the spec's own examples:
    /// `using` needs no comma before it (`llm.write "..." using x`, spec 9.3),
    /// and in a shape field a trailing `, max N` is the field's cap and ends
    /// the arguments (spec 7.2).
    pub(super) fn command_args(&mut self, in_shape_field: bool) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        loop {
            args.push(self.call_arg(CallForm::Command)?);
            if self.at_word("using") && self.starts_value(self.kind_at(1)) {
                continue;
            }
            if !self.at_op(Op::Comma) {
                break;
            }
            if in_shape_field && self.kind_at(1) == &TokenKind::Keyword(Keyword::Max) {
                break;
            }
            self.bump();
        }
        Ok(args)
    }

    /// One argument in either form.
    ///
    /// The named-argument rule (spec sections 5.2 and 13): a `NAME`, or a
    /// reserved word that cannot begin a value, is an argument name when the
    /// token after it can start a value, or, in function form, is `:`. What
    /// can start a value is a `NAME`, a number, a text, `true`, `false`,
    /// `none`, `not`, `focus`, `trail` or `{`. A following `-` is binary
    /// minus, and a following `(` or `[` is a call or an index on the name,
    /// so `f(g(x))` and `f(xs[0])` are positional. Reserved words are
    /// accepted as names because the spec's own verbs use them: `spawn in
    /// tree`, `review(dev, max 20)`.
    fn call_arg(&mut self, form: CallForm) -> PResult<Arg> {
        let start = self.span().start;
        let name = if self.at_log_form() {
            None
        } else if self.can_name_argument(self.kind()) {
            let next = self.kind_at(1);
            let named_with_colon = form == CallForm::Function && next == &TokenKind::Op(Op::Colon);
            if named_with_colon || self.starts_value(next) {
                let name = self.expect_word_or_keyword("an argument name")?;
                if named_with_colon {
                    self.bump();
                }
                Some(name)
            } else {
                None
            }
        } else {
            None
        };
        let value = self.expr()?;
        Ok(Arg {
            name,
            span: Span::new(start, value.span().end),
            value,
        })
    }

    /// Whether a token may serve as an argument name: any identifier, or a
    /// reserved word that could not begin a value in that position.
    fn can_name_argument(&self, kind: &TokenKind) -> bool {
        match kind {
            TokenKind::Name(_) => true,
            TokenKind::Keyword(_) => !self.starts_value(kind),
            _ => false,
        }
    }

    /// Whether a token can begin an argument value. See [`Parser::call_arg`].
    fn starts_value(&self, kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Name(_)
                | TokenKind::Number(_)
                | TokenKind::Text(_)
                | TokenKind::Keyword(
                    Keyword::True
                        | Keyword::False
                        | Keyword::None
                        | Keyword::Not
                        | Keyword::Focus
                        | Keyword::Trail
                )
                | TokenKind::Op(Op::LBrace)
        )
    }

    /// Whether the tokens after a path begin command-form arguments: a value,
    /// or an argument name followed by a value (spec section 5.2).
    pub(super) fn starts_command_arg(&self) -> bool {
        let kind = self.kind();
        self.starts_value(kind)
            || (self.can_name_argument(kind) && self.starts_value(self.kind_at(1)))
    }

    /* ---------------------------------------------------------------- */
    /* text                                                              */
    /* ---------------------------------------------------------------- */

    /// A `TEXT` token as a literal, with every `{expr}` hole checked
    /// (spec section 2.7).
    pub(super) fn text_lit(&mut self, what: &str) -> PResult<TextLit> {
        let TokenKind::Text(lit) = self.kind() else {
            return Err(self.expected(what));
        };
        for part in &lit.parts {
            if let TextPart::Interpolation { source, span } = part
                && let Err(diagnostics) = parse_expression_in(source, *span, self.log_is_unit)
            {
                return Err(diagnostics.into_iter().next().unwrap_or_else(|| {
                    Diagnostic::error(ErrorCode::Syntax, "malformed interpolation", *span)
                }));
            }
        }
        self.bump();
        Ok(lit.clone())
    }

    /// A `TEXT` token as an expression.
    pub(super) fn text_expr(&mut self, what: &str) -> PResult<Expr> {
        let span = self.span();
        let value = self.text_lit(what)?;
        Ok(Expr::Text { value, span })
    }
}

fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op,
        span: Span::new(left.span().start, right.span().end),
        left: Box::new(left),
        right: Box::new(right),
    }
}
