//! The program header, declarations and units: spec sections 3, 6.7, 7, 7.8,
//! 8 and 9.4, productions `program` through `param` in section 13.

use super::{PResult, Parser};
use crate::ast::{
    BudgetItem, BudgetKey, CapabilityKind, Decl, DefUnit, Expr, FieldDecl, JudgmentLog,
    JudgmentResult, JudgmentUnit, MachineState, MachineUnit, Param, Program, ReturnType, Shape,
    ShapeField, TaskUnit, ThresholdItem, ToolSignature, Transition, TypeName, Unit, UseDecl,
    UseMapping,
};
use crate::diagnostic::ErrorCode;
use crate::span::Span;
use crate::token::{Keyword, Op, TokenKind};

impl Parser<'_> {
    /// `program = "program" NAME NEWLINE { use_decl } { decl } { unit }`.
    ///
    /// The three groups come in that order and nothing else (spec section 3);
    /// a `use` after a declaration is an error that says so.
    pub(super) fn parse_program(&mut self) -> PResult<Program> {
        let start = self.span().start;
        self.expect_kw(Keyword::Program)?;
        let name = self.expect_name("the program name")?;
        self.expect_newline()?;

        let mut uses = Vec::new();
        while self.at_kw(Keyword::Use) {
            uses.push(self.use_decl()?);
        }
        let mut decls = Vec::new();
        while matches!(
            self.kind(),
            TokenKind::Keyword(Keyword::In | Keyword::Out | Keyword::Needs)
        ) {
            decls.push(self.decl()?);
        }
        let mut units = Vec::new();
        while !self.at_eof() {
            units.push(self.unit()?);
        }
        let end = self.prev_end();
        Ok(Program {
            name,
            uses,
            decls,
            units,
            span: Span::new(start, end),
        })
    }

    /// `use_decl = "use" TEXT "as" NAME [ "with" mapping { "," mapping } ] NEWLINE`
    /// (spec section 3.9). `as` and `with` are contextual words.
    fn use_decl(&mut self) -> PResult<UseDecl> {
        let start = self.span().start;
        self.expect_kw(Keyword::Use)?;
        let path = self.text_lit("the module path")?;
        self.expect_word("as")?;
        let alias = self.expect_name("a module alias")?;
        let mut mapping = Vec::new();
        if self.eat_word("with") {
            loop {
                mapping.push(self.use_mapping()?);
                if !self.eat_op(Op::Comma) {
                    break;
                }
            }
        }
        let end = self.prev_end();
        self.expect_newline()?;
        Ok(UseDecl {
            path,
            alias,
            mapping,
            span: Span::new(start, end),
        })
    }

    /// `mapping = NAME [ ":" NAME ]`: the library's name, then the importer's.
    fn use_mapping(&mut self) -> PResult<UseMapping> {
        let start = self.span().start;
        let inner = self.expect_name("a capability name")?;
        let outer = if self.eat_op(Op::Colon) {
            Some(self.expect_name("the importer's capability name")?)
        } else {
            None
        };
        Ok(UseMapping {
            inner,
            outer,
            span: Span::new(start, self.prev_end()),
        })
    }

    /// `decl = "in" NAME ":" shape NEWLINE | "out" NAME NEWLINE
    /// | "needs" NAME ":" kind [ ":" NEWLINE INDENT { signature } DEDENT ] NEWLINE`
    /// (spec sections 3.2 to 3.4 and 9.4).
    fn decl(&mut self) -> PResult<Decl> {
        let start = self.span().start;
        if self.eat_kw(Keyword::In) {
            let name = self.expect_name("an input name")?;
            self.expect_op(Op::Colon)?;
            let shape = self.shape()?;
            let end = self.prev_end();
            self.expect_newline()?;
            return Ok(Decl::In {
                name,
                shape,
                span: Span::new(start, end),
            });
        }
        if self.eat_kw(Keyword::Out) {
            let name = self.expect_name("an output name")?;
            let end = self.prev_end();
            self.expect_newline()?;
            return Ok(Decl::Out {
                name,
                span: Span::new(start, end),
            });
        }
        self.expect_kw(Keyword::Needs)?;
        let name = self.expect_name("a capability name")?;
        self.expect_op(Op::Colon)?;
        let kind = self.capability_kind()?;
        let mut end = self.prev_end();
        let mut signatures = Vec::new();
        if self.at_op(Op::Colon) {
            if kind != CapabilityKind::Tool {
                return Err(self.expected("the end of the line (only a `tool` declares a signature block, spec section 9.4)"));
            }
            self.bump();
            self.open_block("of verb signatures after `tool:`")?;
            while self.block_continues() {
                signatures.push(self.signature()?);
            }
            end = signatures.last().map_or(end, |s| s.span.end);
            self.close_block()?;
        } else {
            self.expect_newline()?;
        }
        Ok(Decl::Needs {
            name,
            kind,
            signatures,
            span: Span::new(start, end),
        })
    }

    /// `kind = "agent" | "person" | "llm" | "tool"` (spec section 9).
    fn capability_kind(&mut self) -> PResult<CapabilityKind> {
        let kind = match self.kind() {
            TokenKind::Name(n) if n == "agent" => CapabilityKind::Agent,
            TokenKind::Name(n) if n == "person" => CapabilityKind::Person,
            TokenKind::Name(n) if n == "llm" => CapabilityKind::Llm,
            TokenKind::Name(n) if n == "tool" => CapabilityKind::Tool,
            _ => {
                return Err(self.expected("a capability kind (`agent`, `person`, `llm` or `tool`)"));
            }
        };
        self.bump();
        Ok(kind)
    }

    /// `signature = NAME "(" [ NAME { "," NAME } ] ")" "->" ret_type NEWLINE`
    /// (spec section 9.4).
    fn signature(&mut self) -> PResult<ToolSignature> {
        let start = self.span().start;
        let name = self.expect_name("a verb name")?;
        self.expect_op(Op::LParen)?;
        let mut params = Vec::new();
        if !self.at_op(Op::RParen) {
            loop {
                params.push(self.expect_name("a parameter name")?);
                if !self.eat_op(Op::Comma) {
                    break;
                }
            }
        }
        self.expect_op(Op::RParen)?;
        self.expect_op(Op::Arrow)?;
        let returns = self.return_type()?;
        let end = self.prev_end();
        self.expect_newline()?;
        Ok(ToolSignature {
            name,
            params,
            returns,
            span: Span::new(start, end),
        })
    }

    /// `ret_type = "text" | "number" | "bool" | "list" | "record" | "handle" | "none"`.
    fn return_type(&mut self) -> PResult<ReturnType> {
        let returns = match self.kind() {
            TokenKind::Name(n) if n == "text" => ReturnType::Text,
            TokenKind::Name(n) if n == "number" => ReturnType::Number,
            TokenKind::Name(n) if n == "bool" => ReturnType::Bool,
            TokenKind::Name(n) if n == "list" => ReturnType::List,
            TokenKind::Name(n) if n == "record" => ReturnType::Record,
            TokenKind::Name(n) if n == "handle" => ReturnType::Handle,
            TokenKind::Keyword(Keyword::None) => ReturnType::None,
            _ => {
                return Err(self.expected(
                    "a return type (`text`, `number`, `bool`, `list`, `record`, `handle` or `none`)",
                ));
            }
        };
        self.bump();
        Ok(returns)
    }

    /// `shape = "text" | "number" | "bool" | "list" | "record"
    /// | "{" [ field_decl { "," field_decl } ] "}"` (spec section 3.2).
    fn shape(&mut self) -> PResult<Shape> {
        let start = self.span().start;
        if self.eat_op(Op::LBrace) {
            let mut fields = Vec::new();
            if !self.at_op(Op::RBrace) {
                loop {
                    fields.push(self.field_decl()?);
                    if !self.eat_op(Op::Comma) {
                        break;
                    }
                }
            }
            self.expect_op(Op::RBrace)?;
            return Ok(Shape::Record {
                fields,
                span: Span::new(start, self.prev_end()),
            });
        }
        let name = match self.kind() {
            TokenKind::Name(n) if n == "text" => TypeName::Text,
            TokenKind::Name(n) if n == "number" => TypeName::Number,
            TokenKind::Name(n) if n == "bool" => TypeName::Bool,
            TokenKind::Name(n) if n == "list" => TypeName::List,
            TokenKind::Name(n) if n == "record" => TypeName::Record,
            _ => {
                return Err(self.expected(
                    "a shape (`text`, `number`, `bool`, `list`, `record` or `{ fields }`)",
                ));
            }
        };
        let span = self.span();
        self.bump();
        Ok(Shape::Type { name, span })
    }

    /// `field_decl = NAME [ ":" shape ]`.
    fn field_decl(&mut self) -> PResult<FieldDecl> {
        let start = self.span().start;
        let name = self.expect_name("a field name")?;
        let shape = if self.eat_op(Op::Colon) {
            Some(self.shape()?)
        } else {
            None
        };
        Ok(FieldDecl {
            name,
            shape,
            span: Span::new(start, self.prev_end()),
        })
    }

    /// `unit = judgment | task | def | machine`.
    fn unit(&mut self) -> PResult<Unit> {
        self.verifies.clear();
        match self.kind() {
            TokenKind::Keyword(Keyword::Judgment) => self.judgment().map(Unit::Judgment),
            TokenKind::Keyword(Keyword::Task) => self.task().map(Unit::Task),
            TokenKind::Keyword(Keyword::Def) => self.def().map(Unit::Def),
            TokenKind::Keyword(Keyword::Machine) => self.machine().map(Unit::Machine),
            TokenKind::Keyword(Keyword::Use) => Err(self.expected(
                "a unit (`use` declarations come before `in`, `out`, `needs` and every unit, spec section 3)",
            )),
            TokenKind::Keyword(Keyword::In | Keyword::Out | Keyword::Needs) => Err(self.expected(
                "a unit (`in`, `out` and `needs` declarations come before every unit, spec section 3)",
            )),
            _ => Err(self.expected("`judgment`, `task`, `def` or `machine`")),
        }
    }

    /// `judgment = "judgment" NAME "(" [ NAME { "," NAME } ] ")" ":" NEWLINE
    /// INDENT { NAME "=" judge_expr NEWLINE } DEDENT` (spec section 6.7).
    ///
    /// The body admits nothing but judgment lines and `log` lines, which is
    /// what makes a judgment pure and one request: a log has no effect and
    /// never reaches Jev (spec section 5.8).
    fn judgment(&mut self) -> PResult<JudgmentUnit> {
        let start = self.span().start;
        self.expect_kw(Keyword::Judgment)?;
        let name = self.expect_name("the judgment's name")?;
        self.expect_op(Op::LParen)?;
        let mut params = Vec::new();
        if !self.at_op(Op::RParen) {
            loop {
                params.push(self.expect_name("a parameter name")?);
                if !self.eat_op(Op::Comma) {
                    break;
                }
            }
        }
        self.expect_op(Op::RParen)?;
        self.expect_op(Op::Colon)?;
        let mut end = self.prev_end();
        self.open_block("of `name = judgment` lines after `judgment ...:`")?;
        let mut results = Vec::new();
        let mut logs = Vec::new();
        while self.block_continues() {
            let line_start = self.span().start;
            if self.at_log_form() {
                let log = self.expr()?;
                if !matches!(log, Expr::Log { .. }) {
                    return Err(self.error_at(
                        ErrorCode::Syntax,
                        "a judgment body holds only `name = judgment` and `log` lines (spec sections 5.8 and 6.7)",
                        log.span(),
                    ));
                }
                let span = log.span();
                self.end_of_line()?;
                end = span.end;
                logs.push(JudgmentLog {
                    after: results.len(),
                    log,
                    span,
                });
                continue;
            }
            let result_name = self.expect_name(
                "a result name (a judgment body holds only `name = judgment` lines)",
            )?;
            self.expect_op(Op::Assign)?;
            let question = self.judge_expr()?;
            let span = Span::new(line_start, question.span.end);
            self.end_of_line()?;
            end = span.end;
            results.push(JudgmentResult {
                name: result_name,
                question,
                span,
            });
        }
        self.close_block()?;
        Ok(JudgmentUnit {
            name,
            params,
            results,
            logs,
            span: Span::new(start, end),
        })
    }

    /// `task = "task" NAME [ "(" [ param { "," param } ] ")" ] [ "budget" budget_list ]
    /// [ "thresholds" threshold_list ] ":" block` (spec section 7).
    fn task(&mut self) -> PResult<TaskUnit> {
        let start = self.span().start;
        self.expect_kw(Keyword::Task)?;
        let name = self.expect_name("the task's name")?;
        let params = if self.at_op(Op::LParen) {
            self.params()?
        } else {
            Vec::new()
        };
        let budgets = self.budget_list()?;
        let thresholds = self.threshold_list()?;
        self.expect_op(Op::Colon)?;
        let body = self.block("after `task ...:`")?;
        Ok(TaskUnit {
            name,
            params,
            budgets,
            thresholds,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `def = "def" NAME "(" [ param { "," param } ] ")" ":" block` (spec section 8).
    fn def(&mut self) -> PResult<DefUnit> {
        let start = self.span().start;
        self.expect_kw(Keyword::Def)?;
        let name = self.expect_name("the def's name")?;
        let params = self.params()?;
        self.expect_op(Op::Colon)?;
        let body = self.block("after `def ...:`")?;
        Ok(DefUnit {
            name,
            params,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    /// `"(" [ param { "," param } ] ")"` with `param = NAME [ "=" expr ]`.
    fn params(&mut self) -> PResult<Vec<Param>> {
        self.expect_op(Op::LParen)?;
        let mut params = Vec::new();
        if !self.at_op(Op::RParen) {
            loop {
                let start = self.span().start;
                let name = self.expect_name("a parameter name")?;
                let default = if self.eat_op(Op::Assign) {
                    Some(self.expr()?)
                } else {
                    None
                };
                params.push(Param {
                    name,
                    default,
                    span: Span::new(start, self.prev_end()),
                });
                if !self.eat_op(Op::Comma) {
                    break;
                }
            }
        }
        self.expect_op(Op::RParen)?;
        Ok(params)
    }

    /// `[ "budget" budget_list ]`, `budget_item = ( "calls" | "minutes" | "usd" | "steps" ) NUMBER`
    /// (spec section 7.1).
    fn budget_list(&mut self) -> PResult<Vec<BudgetItem>> {
        let mut items = Vec::new();
        if !self.eat_kw(Keyword::Budget) {
            return Ok(items);
        }
        loop {
            let start = self.span().start;
            let key = match self.kind() {
                TokenKind::Name(n) if n == "calls" => BudgetKey::Calls,
                TokenKind::Name(n) if n == "minutes" => BudgetKey::Minutes,
                TokenKind::Name(n) if n == "usd" => BudgetKey::Usd,
                TokenKind::Name(n) if n == "steps" => BudgetKey::Steps,
                _ => {
                    return Err(
                        self.expected("a budget key (`calls`, `minutes`, `usd` or `steps`)")
                    );
                }
            };
            self.bump();
            let (value, _) = self.expect_number("the budget's limit")?;
            items.push(BudgetItem {
                key,
                value,
                span: Span::new(start, self.prev_end()),
            });
            if !self.eat_op(Op::Comma) {
                break;
            }
        }
        Ok(items)
    }

    /// `[ "thresholds" threshold_list ]`, `threshold_item = NAME NUMBER`
    /// (spec section 7.6). `thresholds` is not reserved, so it is matched by
    /// spelling here.
    fn threshold_list(&mut self) -> PResult<Vec<ThresholdItem>> {
        let mut items = Vec::new();
        if !self.eat_word("thresholds") {
            return Ok(items);
        }
        loop {
            let start = self.span().start;
            let name = self.expect_name("a threshold name")?;
            let (value, _) = self.expect_number("the threshold's value")?;
            items.push(ThresholdItem {
                name,
                value,
                span: Span::new(start, self.prev_end()),
            });
            if !self.eat_op(Op::Comma) {
                break;
            }
        }
        Ok(items)
    }

    /// `machine = "machine" NAME "(" [ param { "," param } ] ")" [ "budget" budget_list ]
    /// [ "thresholds" threshold_list ] ":" NEWLINE INDENT [ "goal" TEXT NEWLINE ]
    /// [ "initial" NAME NEWLINE ] [ observe ] { state } DEDENT` (spec section 7.8).
    ///
    /// `goal` and `observe` are contextual words and `initial` is reserved;
    /// the header lines come in the grammar's order.
    fn machine(&mut self) -> PResult<MachineUnit> {
        let start = self.span().start;
        self.expect_kw(Keyword::Machine)?;
        let name = self.expect_name("the machine's name")?;
        let params = self.params()?;
        let budgets = self.budget_list()?;
        let thresholds = self.threshold_list()?;
        self.expect_op(Op::Colon)?;
        self.open_block("after `machine ...:`")?;

        let goal = if self.eat_word("goal") {
            let goal = self.text_expr("the goal text")?;
            self.expect_newline()?;
            Some(goal)
        } else {
            None
        };
        let initial = if self.eat_kw(Keyword::Initial) {
            let state = self.expect_name("the initial state's name")?;
            self.expect_newline()?;
            Some(state)
        } else {
            None
        };
        let observe = if self.eat_word("observe") {
            self.observe()?
        } else {
            Vec::new()
        };

        let mut states = Vec::new();
        let mut end = self.prev_end();
        while self.block_continues() {
            let state = self.machine_state()?;
            end = state.span.end;
            states.push(state);
        }
        self.close_block()?;
        Ok(MachineUnit {
            name,
            params,
            budgets,
            thresholds,
            goal,
            initial,
            observe,
            states,
            span: Span::new(start, end),
        })
    }

    /// `observe = "observe" ":" NEWLINE INDENT { shape_field } DEDENT`, the same
    /// form as `shape` (spec section 7.8). The word itself was consumed by the
    /// caller.
    fn observe(&mut self) -> PResult<Vec<ShapeField>> {
        self.expect_op(Op::Colon)?;
        self.open_block("of fields after `observe:`")?;
        let mut fields = Vec::new();
        while self.block_continues() {
            fields.push(self.shape_field()?);
        }
        self.close_block()?;
        Ok(fields)
    }

    /// `state = "state" NAME "done" NEWLINE | "state" NAME ":" NEWLINE INDENT { transition } DEDENT`.
    fn machine_state(&mut self) -> PResult<MachineState> {
        let start = self.span().start;
        if !self.at_kw(Keyword::State) {
            return Err(self.expected(
                "`state` (a machine body is `goal`, `initial`, `observe:` and then states, in that order)",
            ));
        }
        self.bump();
        let name = self.expect_name("the state's name")?;
        if self.eat_word("done") {
            let end = self.prev_end();
            self.expect_newline()?;
            return Ok(MachineState {
                name,
                done: true,
                transitions: Vec::new(),
                span: Span::new(start, end),
            });
        }
        if !self.at_op(Op::Colon) {
            return Err(self.expected("`done` or `:` after the state's name"));
        }
        self.bump();
        let mut end = self.prev_end();
        self.open_block("of `on` transitions after `state ...:`")?;
        let mut transitions = Vec::new();
        while self.block_continues() {
            let transition = self.transition()?;
            end = transition.span.end;
            transitions.push(transition);
        }
        self.close_block()?;
        Ok(MachineState {
            name,
            done: false,
            transitions,
            span: Span::new(start, end),
        })
    }

    /// `transition = "on" NAME TEXT "->" NAME [ "when" expr ] [ "risky" ] ( NEWLINE | ":" block )`
    /// (spec section 7.8). `risky` is a contextual word.
    fn transition(&mut self) -> PResult<Transition> {
        let start = self.span().start;
        self.expect_kw(Keyword::On)?;
        let event = self.expect_name("an event name")?;
        if !matches!(self.kind(), TokenKind::Text(_)) {
            return Err(self.error_at(
                ErrorCode::EventNoDescription,
                format!(
                    "`on {}` needs a description text: it is what Jev reads (spec section 7.8)",
                    event.name
                ),
                Span::new(start, event.span.end),
            ));
        }
        let description = self.text_expr("the event's description")?;
        self.expect_op(Op::Arrow)?;
        let target = self.expect_name("the target state's name")?;
        let when = if self.eat_kw(Keyword::When) {
            Some(self.expr()?)
        } else {
            None
        };
        let risky = self.eat_word("risky");
        let (body, end) = if self.eat_op(Op::Colon) {
            let body = self.block("of actions after `on ...:`")?;
            let end = body.span.end;
            (Some(body), end)
        } else {
            let end = self.prev_end();
            self.expect_newline()?;
            (None, end)
        };
        Ok(Transition {
            event,
            description,
            target,
            when,
            risky,
            body,
            span: Span::new(start, end),
        })
    }
}
