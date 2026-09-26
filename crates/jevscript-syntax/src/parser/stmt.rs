//! Statements and blocks: spec sections 5.2 to 5.6, 7.2, 7.4 and 7.6,
//! productions `block` through `shape_field` in section 13.

use super::{PResult, Parser};
use crate::ast::{
    Block, Branch, CallForm, Expr, GateArm, GateStmt, Ident, ShapeField, ShapeStmt, Stmt, Target,
    Verdict,
};
use crate::diagnostic::ErrorCode;
use crate::span::{Pos, Span};
use crate::token::{Keyword, Op, TokenKind};

/// Where a right-hand side sits, which decides what it may be (spec 5.2, 7.2).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RhsContext {
    /// `target = ...`: a judgment, a command or an expression.
    Assign,
    /// A `shape` or `observe` field: a command or an expression, and a trailing
    /// `, max N` belongs to the field, not to the command.
    ShapeField,
}

impl Parser<'_> {
    /// `block = NEWLINE INDENT { statement } DEDENT`.
    ///
    /// The span runs from the first statement to the last, not to the
    /// `DEDENT`, which the lexer places on the following line.
    pub(super) fn block(&mut self, what: &str) -> PResult<Block> {
        self.open_block(what)?;
        let start = self.span().start;
        let mut end = start;
        let mut statements = Vec::new();
        while self.block_continues() {
            let statement = self.statement()?;
            end = stmt_span(&statement).end;
            statements.push(statement);
        }
        self.close_block()?;
        Ok(Block {
            statements,
            span: Span::new(start, end),
        })
    }

    /// `statement = assign | call_stmt | if_stmt | for_stmt | loop_stmt | until_stmt
    /// | gate_stmt | shape_assign | "return" [ expr ] NEWLINE | "continue" NEWLINE
    /// | "break" NEWLINE | "stop" TEXT NEWLINE | "escalate" TEXT NEWLINE`.
    pub(super) fn statement(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        match self.kind() {
            TokenKind::Keyword(Keyword::If) => self.if_stmt(),
            TokenKind::Keyword(Keyword::For) => self.for_stmt(),
            TokenKind::Keyword(Keyword::Loop) => self.loop_stmt(),
            TokenKind::Keyword(Keyword::Until) => self.until_stmt(),
            TokenKind::Keyword(Keyword::Gate) => self.gate_stmt(),
            TokenKind::Keyword(Keyword::Return) => {
                self.bump();
                let value = if self.at_newline() {
                    None
                } else {
                    Some(self.expr()?)
                };
                let span = Span::new(start, self.prev_end());
                self.expect_newline()?;
                Ok(Stmt::Return { value, span })
            }
            TokenKind::Keyword(Keyword::Continue) => {
                self.bump();
                let span = Span::new(start, self.prev_end());
                self.expect_newline()?;
                Ok(Stmt::Continue { span })
            }
            TokenKind::Keyword(Keyword::Break) => {
                self.bump();
                let span = Span::new(start, self.prev_end());
                self.expect_newline()?;
                Ok(Stmt::Break { span })
            }
            TokenKind::Keyword(Keyword::Stop) => {
                self.bump();
                let reason = self.text_expr("the reason text after `stop`")?;
                let span = Span::new(start, self.prev_end());
                self.expect_newline()?;
                Ok(Stmt::Stop { reason, span })
            }
            TokenKind::Keyword(Keyword::Escalate) => {
                self.bump();
                let reason = self.text_expr("the reason text after `escalate`")?;
                let span = Span::new(start, self.prev_end());
                self.expect_newline()?;
                Ok(Stmt::Escalate { reason, span })
            }
            TokenKind::Keyword(Keyword::Elif) => {
                Err(self.expected("a statement (`elif` has no `if` at this indentation)"))
            }
            TokenKind::Keyword(Keyword::Else) => {
                Err(self.expected("a statement (`else` has no `if` at this indentation)"))
            }
            TokenKind::Name(_) => self.simple_stmt(),
            _ => Err(self.expected("a statement")),
        }
    }

    /// `assign = target "=" ( judge_expr | command | expr ) NEWLINE`,
    /// `call_stmt = command NEWLINE` and `shape_assign` (spec sections 5.2,
    /// 5.3 and 7.2), all of which begin with a `target = NAME { "." NAME }`.
    ///
    /// The head is parsed as an expression, which also covers function-form
    /// calls in statement position (`harness.watch(dev, tree)`, spec 3.9) and
    /// a `log` written as a statement (spec 5.8).
    /// A statement that is neither an assignment, a call nor a bare path is
    /// an error: an expression on its own does nothing.
    fn simple_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        let head = self.expr()?;

        if self.at_op(Op::Assign) {
            let target = target_from_expr(&head).ok_or_else(|| {
                self.error_at(
                    ErrorCode::Syntax,
                    "cannot assign to this expression; a target is a name or a field path (spec section 5.3)",
                    head.span(),
                )
            })?;
            self.bump();
            if self.at_kw(Keyword::Shape) {
                return self.shape_stmt(target, start);
            }
            let value = self.rhs(RhsContext::Assign)?;
            let span = Span::new(start, value.span().end);
            self.end_of_line()?;
            return Ok(Stmt::Assign {
                target,
                value,
                span,
            });
        }

        if self.at_judge_verb() {
            return Err(self
                .expected("`=` (a judgment's answer must be assigned to a name, spec section 6)"));
        }
        let expr = self.command_tail(head, RhsContext::Assign)?;
        if !is_path(&expr) && !matches!(expr, Expr::Call { .. } | Expr::Log { .. }) {
            return Err(self.error_at(
                ErrorCode::Syntax,
                "expected a statement; an expression on its own does nothing (assign it, call something or `log` it)",
                expr.span(),
            ));
        }
        let span = Span::new(start, expr.span().end);
        self.end_of_line()?;
        Ok(Stmt::Expr { expr, span })
    }

    /// The right-hand side of an assignment or a shape field:
    /// `judge_expr | command | expr` (spec sections 5.2 and 13).
    ///
    /// Command form is permitted here and as a whole statement, nowhere else
    /// (spec 5.2). A bare path such as `dev.stop` or `tree.diff.files` is
    /// returned as the path expression written; the compiler decides which
    /// trailing identifiers on capabilities and handles are zero-argument
    /// calls.
    pub(super) fn rhs(&mut self, ctx: RhsContext) -> PResult<Expr> {
        let start = self.span().start;
        if self.at_kw(Keyword::Each) {
            if ctx == RhsContext::ShapeField {
                return Err(self.expected(
                    "a command or an expression (a shape field cannot be a judgment; assign it first)",
                ));
            }
            return Ok(Expr::Judge(Box::new(self.judge_expr()?)));
        }
        let head = self.expr()?;
        if self.at_judge_verb() {
            if ctx == RhsContext::ShapeField {
                return Err(self.expected(
                    "the end of the field (a shape field cannot be a judgment; assign it first)",
                ));
            }
            return Ok(Expr::Judge(Box::new(self.judge_rest(false, head, start)?)));
        }
        self.command_tail(head, ctx)
    }

    /// `command = target [ cmd_args ]`: turns a path followed by arguments into
    /// a command-form call, and leaves anything else alone (spec section 5.2).
    fn command_tail(&mut self, head: Expr, ctx: RhsContext) -> PResult<Expr> {
        if is_path(&head) && self.starts_command_arg() {
            let args = self.command_args(ctx == RhsContext::ShapeField)?;
            let span = Span::new(head.span().start, self.prev_end());
            return Ok(Expr::Call {
                callee: Box::new(head),
                args,
                form: CallForm::Command,
                span,
            });
        }
        Ok(head)
    }

    /// `if_stmt = "if" expr ":" block { "elif" expr ":" block } [ "else" ":" block ]`
    /// (spec section 5.4).
    fn if_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        self.expect_kw(Keyword::If)?;
        let mut branches = vec![self.branch("after `if ...:`")?];
        while self.eat_kw(Keyword::Elif) {
            branches.push(self.branch("after `elif ...:`")?);
        }
        let otherwise = if self.eat_kw(Keyword::Else) {
            self.expect_op(Op::Colon)?;
            Some(self.block("after `else:`")?)
        } else {
            None
        };
        let end = otherwise.as_ref().map_or_else(
            || branches.last().map_or(start, |b| b.span.end),
            |b| b.span.end,
        );
        Ok(Stmt::If {
            branches,
            otherwise,
            span: Span::new(start, end),
        })
    }

    /// `expr ":" block`, one `if` or `elif` arm.
    fn branch(&mut self, what: &str) -> PResult<Branch> {
        let start = self.span().start;
        let test = self.expr()?;
        self.expect_op(Op::Colon)?;
        let body = self.block(what)?;
        Ok(Branch {
            test,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `for_stmt = "for" NAME [ "," NAME ] "in" expr ":" block` (spec section 5.5).
    fn for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        self.expect_kw(Keyword::For)?;
        let names = self.loop_names()?;
        self.expect_kw(Keyword::In)?;
        let iterable = self.expr()?;
        self.expect_op(Op::Colon)?;
        let body = self.block("after `for ...:`")?;
        Ok(Stmt::For {
            names,
            iterable,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `NAME [ "," NAME ]`, the variables of a `for` or a comprehension.
    pub(super) fn loop_names(&mut self) -> PResult<Vec<Ident>> {
        let mut names = vec![self.expect_name("a loop variable")?];
        if self.eat_op(Op::Comma) {
            names.push(self.expect_name("a second loop variable")?);
        }
        Ok(names)
    }

    /// `loop_stmt = "loop" "max" NUMBER ":" block`. A `loop` without `max` is
    /// `unbounded_loop` (spec section 5.5).
    fn loop_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        let keyword = self.expect_kw(Keyword::Loop)?;
        if !self.at_kw(Keyword::Max) {
            return Err(self.error_at(
                ErrorCode::UnboundedLoop,
                "`loop` needs `max N`: every loop has a compile-time bound (spec section 5.5)",
                keyword,
            ));
        }
        self.bump();
        let (max, _) = self.expect_number("the loop's bound after `max`")?;
        self.expect_op(Op::Colon)?;
        let body = self.block("after `loop max N:`")?;
        Ok(Stmt::Loop {
            max,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `until_stmt = "until" [ "verify" "(" expr ")" | expr ] "," "max" NUMBER ":" block`
    /// (spec sections 5.5 and 7.4).
    ///
    /// `verify` is a contextual word, read as the marker only here and only
    /// when a `(` follows. The grammar brackets the condition as optional,
    /// but an `until` with nothing to wait for has no meaning and the AST has
    /// no way to hold it, so a condition is required. A missing `, max N` is
    /// `unbounded_loop`; a second `verify` in one unit is `verify_twice`.
    fn until_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        self.expect_kw(Keyword::Until)?;
        let (test, verify) =
            if self.at_word("verify") && self.kind_at(1) == &TokenKind::Op(Op::LParen) {
                let marker = self.span();
                self.bump();
                self.bump();
                let test = self.expr()?;
                self.expect_op(Op::RParen)?;
                if !self.verifies.is_empty() {
                    return Err(self.error_at(
                    ErrorCode::VerifyTwice,
                    "a task may declare `verify` once, on its outermost loop (spec section 7.4)",
                    marker,
                ));
                }
                self.verifies.push(marker);
                (test, true)
            } else {
                (self.expr()?, false)
            };
        if !(self.at_op(Op::Comma) && self.kind_at(1) == &TokenKind::Keyword(Keyword::Max)) {
            return Err(self.error_at(
                ErrorCode::UnboundedLoop,
                "`until` needs `, max N`: every loop has a compile-time bound (spec section 5.5)",
                Span::new(start, self.prev_end()),
            ));
        }
        self.bump();
        self.bump();
        let (max, _) = self.expect_number("the loop's bound after `max`")?;
        self.expect_op(Op::Colon)?;
        let body = self.block("after `until ..., max N:`")?;
        Ok(Stmt::Until {
            test,
            verify,
            max,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `gate_stmt = "gate" gate_arg { "," gate_arg } ":" NEWLINE INDENT { gate_arm } DEDENT`,
    /// `gate_arg = ( "risk" | "confidence" | "done" ) expr` (spec section 7.6).
    ///
    /// Whether `risk` and `confidence` are present is a whole-program rule
    /// and is left to the compiler; the grammar admits any subset.
    fn gate_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        self.expect_kw(Keyword::Gate)?;
        let mut gate = GateStmt {
            risk: None,
            confidence: None,
            done: None,
            arms: Vec::new(),
            span: Span::default(),
        };
        loop {
            let arg_span = self.span();
            let (slot, word) = match self.kind() {
                TokenKind::Name(n) if n == "risk" => (&mut gate.risk, "risk"),
                TokenKind::Name(n) if n == "confidence" => (&mut gate.confidence, "confidence"),
                TokenKind::Name(n) if n == "done" => (&mut gate.done, "done"),
                _ => {
                    return Err(self.expected("a gate argument (`risk`, `confidence` or `done`)"));
                }
            };
            if slot.is_some() {
                return Err(self.error_at(
                    ErrorCode::DuplicateName,
                    format!("the gate argument `{word}` is given twice (spec section 12)"),
                    arg_span,
                ));
            }
            self.bump();
            *slot = Some(self.expr()?);
            if !self.eat_op(Op::Comma) {
                break;
            }
        }
        self.expect_op(Op::Colon)?;
        self.open_block("of arms after `gate ...:`")?;
        let mut end = self.prev_end();
        while self.block_continues() {
            let arm = self.gate_arm()?;
            if gate.arms.iter().any(|a| a.verdict == arm.verdict) {
                return Err(self.error_at(
                    ErrorCode::DuplicateName,
                    "this gate arm is written twice (spec section 12)",
                    arm.span,
                ));
            }
            end = arm.span.end;
            gate.arms.push(arm);
        }
        self.close_block()?;
        gate.span = Span::new(start, end);
        Ok(Stmt::Gate(gate))
    }

    /// `gate_arm = ( "proceed" | "confirm" | "escalate" | "stop" ) "->" ( statement | block )`.
    /// A statement on the same line is wrapped as a one-statement block.
    fn gate_arm(&mut self) -> PResult<GateArm> {
        let start = self.span().start;
        let verdict = match self.kind() {
            TokenKind::Keyword(Keyword::Proceed) => Verdict::Proceed,
            TokenKind::Keyword(Keyword::Confirm) => Verdict::Confirm,
            TokenKind::Keyword(Keyword::Escalate) => Verdict::Escalate,
            TokenKind::Keyword(Keyword::Stop) => Verdict::Stop,
            _ => {
                return Err(
                    self.expected("a gate arm (`proceed`, `confirm`, `escalate` or `stop`)")
                );
            }
        };
        self.bump();
        self.expect_op(Op::Arrow)?;
        let body = if self.at_newline() {
            self.block("after `->`")?
        } else {
            let statement = self.statement()?;
            let span = stmt_span(&statement);
            Block {
                statements: vec![statement],
                span,
            }
        };
        Ok(GateArm {
            verdict,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `shape_assign = NAME "=" "shape" [ "strict" ] ":" NEWLINE INDENT { shape_field } DEDENT`
    /// (spec section 7.2). `strict` is a contextual word. The target and `=`
    /// were consumed by the caller.
    fn shape_stmt(&mut self, target: Target, start: Pos) -> PResult<Stmt> {
        if !target.path.is_empty() {
            return Err(self.error_at(
                ErrorCode::Syntax,
                "`shape` assigns to a plain name, not a field path (spec section 7.2)",
                target.span,
            ));
        }
        self.expect_kw(Keyword::Shape)?;
        let strict = self.eat_word("strict");
        self.expect_op(Op::Colon)?;
        self.open_block("of fields after `shape:`")?;
        let mut fields = Vec::new();
        let mut end = self.prev_end();
        while self.block_continues() {
            let field = self.shape_field()?;
            end = field.span.end;
            fields.push(field);
        }
        self.close_block()?;
        Ok(Stmt::Shape(ShapeStmt {
            target: target.root,
            strict,
            fields,
            span: Span::new(start, end),
        }))
    }

    /// `shape_field = NAME ( command | expr ) [ "," "max" NUMBER [ "," "tail" ] ] NEWLINE`
    /// (spec sections 7.2 and 7.8).
    ///
    /// A `focus ... on ..., max N` value keeps its `, max N` for itself
    /// (spec 13, `primary`); only a further `, max N` caps the field. `tail`
    /// is a contextual word, read as the overflow policy only right after
    /// the cap.
    pub(super) fn shape_field(&mut self) -> PResult<ShapeField> {
        let start = self.span().start;
        let name = self.expect_name("a field name")?;
        let value = self.rhs(RhsContext::ShapeField)?;
        let mut max = None;
        let mut tail = false;
        if self.at_op(Op::Comma) && self.kind_at(1) == &TokenKind::Keyword(Keyword::Max) {
            self.bump();
            self.bump();
            max = Some(self.expect_number("the field's token cap after `max`")?.0);
            if self.at_op(Op::Comma) && self.kind_at(1) == &TokenKind::Name("tail".to_string()) {
                self.bump();
                self.bump();
                tail = true;
            }
        }
        let span = Span::new(start, self.prev_end());
        self.expect_newline()?;
        Ok(ShapeField {
            name,
            value,
            max,
            tail,
            span,
        })
    }
}

/// Where a statement was written.
pub(super) fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Assign { span, .. }
        | Stmt::Expr { span, .. }
        | Stmt::If { span, .. }
        | Stmt::For { span, .. }
        | Stmt::Loop { span, .. }
        | Stmt::Until { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::Continue { span }
        | Stmt::Break { span }
        | Stmt::Stop { span, .. }
        | Stmt::Escalate { span, .. } => *span,
        Stmt::Gate(gate) => gate.span,
        Stmt::Shape(shape) => shape.span,
    }
}

/// Whether an expression is a `target = NAME { "." NAME }`, the only callee
/// command form accepts (spec section 5.2).
pub(super) fn is_path(expr: &Expr) -> bool {
    match expr {
        Expr::Name(_) => true,
        Expr::Field { target, .. } => is_path(target),
        _ => false,
    }
}

/// Reads a path expression back as an assignment target.
fn target_from_expr(expr: &Expr) -> Option<Target> {
    fn walk(expr: &Expr, path: &mut Vec<Ident>) -> Option<Ident> {
        match expr {
            Expr::Name(ident) => Some(ident.clone()),
            Expr::Field { target, name, .. } => {
                let root = walk(target, path)?;
                path.push(name.clone());
                Some(root)
            }
            _ => None,
        }
    }
    let mut path = Vec::new();
    let root = walk(expr, &mut path)?;
    Some(Target {
        root,
        path,
        span: expr.span(),
    })
}
