//! The lexer over the example programs from spec section 14.
//!
//! These fixtures are copied verbatim from the spec, so a change here means the
//! spec moved and the lexer has to follow.

use jevscript_syntax::{ErrorCode, Keyword, Op, TextPart, TokenKind, lex, lex_all};

const FIX_ISSUE: &str = include_str!("../../../examples/fix_issue.jev");
const AGENT_LOOP: &str = include_str!("../../../examples/lib/agent_loop.jev");
const INBOX_TRIAGE: &str = include_str!("../../../examples/inbox_triage.jev");
const REVIEW_LOOP: &str = include_str!("../../../examples/review_loop.jev");

fn kinds(source: &str) -> Vec<TokenKind> {
    lex(source)
        .expect("lexes")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

#[test]
fn lexes_the_coding_harness() {
    let tokens = lex(FIX_ISSUE).expect("fix_issue.jev lexes");
    assert_eq!(
        tokens.first().map(|t| &t.kind),
        Some(&TokenKind::Keyword(Keyword::Program))
    );
    assert_eq!(tokens.last().map(|t| &t.kind), Some(&TokenKind::Eof));
    // Blocks open and close in balance.
    let indents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Indent)
        .count();
    let dedents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Dedent)
        .count();
    assert_eq!(indents, dedents, "every INDENT is matched by a DEDENT");
    assert!(indents > 0, "the program has blocks");
}

#[test]
fn lexes_the_judgment_only_program() {
    let tokens = lex(INBOX_TRIAGE).expect("inbox_triage.jev lexes");
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(Keyword::Judgment))
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(Keyword::Pick))
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(Keyword::Rate))
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(Keyword::Feels))
    );
    let indents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Indent)
        .count();
    let dedents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Dedent)
        .count();
    assert_eq!(indents, dedents);
}

#[test]
fn lexes_the_agent_loop_library() {
    let tokens = lex(AGENT_LOOP).expect("lib/agent_loop.jev lexes");
    assert_eq!(
        tokens.first().map(|t| &t.kind),
        Some(&TokenKind::Keyword(Keyword::Program))
    );
    let indents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Indent)
        .count();
    let dedents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Dedent)
        .count();
    assert_eq!(indents, dedents);
    // A library declares `needs` but no `in` or `out` (spec section 3.9).
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&TokenKind::Keyword(Keyword::Needs)));
    assert!(!kinds.contains(&TokenKind::Keyword(Keyword::Out)));
}

#[test]
fn every_judgment_verb_and_gate_arm_appears_in_the_library() {
    let kinds = kinds(AGENT_LOOP);
    for keyword in [
        Keyword::Needs,
        Keyword::Judgment,
        Keyword::Task,
        Keyword::Budget,
        Keyword::Until,
        Keyword::Shape,
        Keyword::Gate,
        Keyword::Confirm,
        Keyword::Escalate,
        Keyword::Proceed,
        Keyword::Trail,
        Keyword::Max,
        Keyword::Each,
        Keyword::Return,
    ] {
        assert!(
            kinds.contains(&TokenKind::Keyword(keyword)),
            "missing {keyword:?}"
        );
    }
    assert!(
        kinds.contains(&TokenKind::Op(Op::Arrow)),
        "gate arms use `->`"
    );
}

#[test]
fn a_use_declaration_lexes_with_contextual_as_and_with() {
    // `use "./lib/agent_loop.jev" as loop with claude, tree, me` (spec 3.9).
    let kinds = kinds(FIX_ISSUE);
    let start = kinds
        .iter()
        .position(|k| *k == TokenKind::Keyword(Keyword::Use))
        .expect("the program imports a module");
    // `use` is reserved; `as` and `with` are contextual, so the lexer emits them
    // as ordinary names and only the parser reads them as keywords.
    assert!(
        matches!(kinds[start + 1], TokenKind::Text(_)),
        "the path is a text literal"
    );
    assert_eq!(kinds[start + 2], TokenKind::Name("as".to_string()));
    // The alias is `harness`, not `loop`: the spec renamed it so that an alias
    // is never a reserved word (section 2.5 reserves `loop`).
    assert_eq!(kinds[start + 3], TokenKind::Name("harness".to_string()));
    assert_eq!(kinds[start + 4], TokenKind::Name("with".to_string()));
    assert_eq!(kinds[start + 5], TokenKind::Name("claude".to_string()));
}

#[test]
fn lexes_the_review_machine() {
    let tokens = lex(REVIEW_LOOP).expect("review_loop.jev lexes");
    let indents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Indent)
        .count();
    let dedents = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Dedent)
        .count();
    assert_eq!(indents, dedents, "every INDENT is matched by a DEDENT");
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|t| t.kind).collect();
    // The machine unit's reserved words (spec sections 2.5 and 7.8).
    for keyword in [Keyword::Machine, Keyword::State, Keyword::On, Keyword::When] {
        assert!(
            kinds.contains(&TokenKind::Keyword(keyword)),
            "missing {keyword:?}"
        );
    }
    assert!(
        kinds.contains(&TokenKind::Op(Op::Arrow)),
        "transitions use `->`"
    );
}

#[test]
fn a_machines_contextual_words_stay_identifiers() {
    // `goal`, `observe`, `risky` and `done` are contextual: only the parser
    // reads them as keywords, and a program may still use them as names
    // (spec section 2.5).
    let kinds = kinds(REVIEW_LOOP);
    for word in ["goal", "observe", "risky", "done"] {
        assert!(
            kinds.contains(&TokenKind::Name(word.to_string())),
            "`{word}` should lex as a name"
        );
    }
}

#[test]
fn a_tool_signature_block_lexes_as_verbs_and_return_types() {
    // `needs tree: tool:` opens a signature block (spec section 9.4).
    let kinds = kinds(REVIEW_LOOP);
    let needs = kinds
        .iter()
        .position(|k| *k == TokenKind::Keyword(Keyword::Needs))
        .expect("the program declares capabilities");
    assert!(kinds[needs..].contains(&TokenKind::Name("tests_pass".to_string())));
    assert!(kinds[needs..].contains(&TokenKind::Name("bool".to_string())));
}

#[test]
fn numbers_carry_their_suffix_meaning() {
    // `2k` is 2000 and `80%` is 0.8 (spec 2.6).
    let kinds = kinds("program p\n\nin a: number\n\ntask main:\n  x = 2k\n  y = 80%\n  z = 0.75\n");
    assert!(kinds.contains(&TokenKind::Number(2000.0)));
    assert!(kinds.contains(&TokenKind::Number(0.8)));
    assert!(kinds.contains(&TokenKind::Number(0.75)));
}

#[test]
fn text_interpolation_is_kept_as_source() {
    let tokens =
        lex("program p\n\ntask main:\n  dev.send \"Tests fail:\\n{obs.tests}\"\n").expect("lexes");
    let text = tokens
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::Text(lit) => Some(lit),
            _ => None,
        })
        .expect("a text literal");
    assert_eq!(text.parts.len(), 2);
    assert_eq!(
        text.parts[0],
        TextPart::Literal("Tests fail:\n".to_string())
    );
    match &text.parts[1] {
        TextPart::Interpolation { source, .. } => assert_eq!(source, "obs.tests"),
        other => panic!("expected an interpolation, got {other:?}"),
    }
}

#[test]
fn a_tab_in_indentation_is_an_indent_error() {
    let errors = lex("program p\n\ntask main:\n\tx = 1\n").expect_err("tabs are rejected");
    assert!(errors.iter().any(|d| d.code == ErrorCode::Indent));
}

#[test]
fn an_uppercase_identifier_is_rejected() {
    let errors = lex("program p\n\ntask main:\n  Total = 1\n").expect_err("uppercase is rejected");
    assert!(
        errors
            .iter()
            .any(|d| d.code == ErrorCode::UppercaseIdentifier)
    );
}

#[test]
fn a_comment_only_line_produces_no_tokens() {
    let kinds = kinds("program p\n# just a comment\n");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Keyword(Keyword::Program),
            TokenKind::Name("p".to_string()),
            TokenKind::Newline,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn a_bracket_holds_a_statement_open_across_lines() {
    let kinds = kinds("program p\n\ntask main:\n  xs = [\n    1,\n    2,\n  ]\n");
    let newlines = kinds.iter().filter(|k| **k == TokenKind::Newline).count();
    // One each for `program p` and `task main:`, and one for the whole
    // `xs = [...]` statement: the open bracket swallows the lines inside it.
    assert_eq!(newlines, 3);
}

#[test]
fn lex_all_keeps_tokens_and_comments_past_an_error() {
    // Spec sections 2.3 and 2.4: a comment runs from `#` to the end of its
    // line, and an uppercase identifier is an error the lexer reports without
    // stopping, so an editor still gets every token of a broken buffer.
    let source = "# heading\nprogram Demo  # trailing\ntask main:\n  x = \"# not a comment\"\n";
    let lexed = lex_all(source);
    assert_eq!(lexed.diagnostics.len(), 1);
    assert_eq!(lexed.diagnostics[0].code, ErrorCode::UppercaseIdentifier);
    let comments: Vec<&str> = lexed
        .comments
        .iter()
        .map(|span| &source[span.start.offset as usize..span.end.offset as usize])
        .collect();
    assert_eq!(comments, ["# heading", "# trailing"]);
    assert!(
        lexed
            .tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword(Keyword::Task)),
        "lexing continued past the error"
    );
    assert!(lex(source).is_err(), "`lex` still refuses the source");
}
