//! What the lowering must get right, stated as Ruby in and tree out.
//!
//! These are the cases where a fold could quietly lose something — the 31-node
//! assignment grid, a literal's base, a binary-encoded string — plus the shapes
//! that a sweep over ruby/spec found broken before they were fixed.

use spinel_ast::*;
use spinel_parse::{Origin, parse};

/// Parse, and insist the source was clean. Returns the top-level statements.
fn body(source: &str) -> Vec<Expr> {
    let parsed = parse(source.as_bytes());
    assert!(
        parsed.errors.is_empty(),
        "{source:?} should lower cleanly: {:?}",
        parsed.errors
    );
    parsed.program.body
}

fn only(source: &str) -> ExprKind {
    let mut body = body(source);
    assert_eq!(body.len(), 1, "{source:?} should be one statement");
    body.pop().expect("checked length").kind
}

/// `def f; <body> end` — reach the single statement in a method body.
fn in_def(kind: ExprKind) -> ExprKind {
    let ExprKind::Def(def) = kind else {
        panic!("should be a Def");
    };
    let mut body = def.body;
    assert_eq!(body.len(), 1);
    body.pop().expect("checked length").kind
}

/// `xs.each { <body> }` — reach the single statement in a block body.
fn in_block(kind: ExprKind) -> ExprKind {
    let ExprKind::Call(call) = kind else {
        panic!("should be a Call");
    };
    let Some(BlockArg::Block(block)) = call.block else {
        panic!("should carry a block");
    };
    let mut body = block.body;
    assert_eq!(body.len(), 1);
    body.pop().expect("checked length").kind
}

#[test]
fn every_assignment_form_folds_into_one_shape() {
    // The largest judgement call in the tree: 31 Prism nodes, one `Assign`. The
    // distinction has to survive in `Target` and `AssignOp`, or the fold lost
    // information rather than removing duplication.
    let cases = [
        ("a = 1", AssignOp::Assign),
        ("a ||= 1", AssignOp::Or),
        ("a &&= 1", AssignOp::And),
        ("a += 1", AssignOp::Binary("+".into())),
        ("@a = 1", AssignOp::Assign),
        ("@a ||= 1", AssignOp::Or),
        ("@@a *= 1", AssignOp::Binary("*".into())),
        ("$a = 1", AssignOp::Assign),
        ("A = 1", AssignOp::Assign),
        ("A::B ||= 1", AssignOp::Or),
        ("a.b ||= 1", AssignOp::Or),
        ("a[0] += 1", AssignOp::Binary("+".into())),
        ("a, b = 1, 2", AssignOp::Assign),
    ];
    for (source, expected) in cases {
        let ExprKind::Assign(assign) = only(source) else {
            panic!("{source:?} should be an Assign");
        };
        assert_eq!(assign.op, expected, "{source:?}");
    }
}

#[test]
fn assignment_targets_keep_the_variable_kind() {
    let kind = |source: &str| {
        let ExprKind::Assign(a) = only(source) else {
            panic!("{source:?} should be an Assign");
        };
        a.target.kind
    };
    assert!(matches!(
        kind("a ||= 1"),
        TargetKind::Var(VarRef::Local { .. })
    ));
    assert!(matches!(
        kind("@a ||= 1"),
        TargetKind::Var(VarRef::Instance(_))
    ));
    assert!(matches!(
        kind("@@a ||= 1"),
        TargetKind::Var(VarRef::Class(_))
    ));
    assert!(matches!(
        kind("$a ||= 1"),
        TargetKind::Var(VarRef::Global(_))
    ));
    assert!(matches!(kind("A ||= 1"), TargetKind::Var(VarRef::Const(_))));
    assert!(matches!(kind("A::B ||= 1"), TargetKind::ConstPath(_)));
    assert!(matches!(kind("a.b ||= 1"), TargetKind::Call(_)));
    assert!(matches!(kind("a[0] ||= 1"), TargetKind::Index(_)));
    assert!(matches!(kind("a, b = xs"), TargetKind::Multi(_)));
}

#[test]
fn call_targets_keep_the_read_name() {
    // Prism carries both `b` and `b=`; the tree keeps the read name and lets the
    // compiler append `=`, which is the rule Prism itself applies.
    let ExprKind::Assign(a) = only("a.b ||= 1") else {
        panic!("should be an Assign");
    };
    let TargetKind::Call(call) = a.target.kind else {
        panic!("should target a call");
    };
    assert_eq!(&*call.name, "b");
}

#[test]
fn integer_literals_keep_their_base() {
    let int = |source: &str| {
        let ExprKind::Int(lit) = only(source) else {
            panic!("{source:?} should be an Int");
        };
        lit
    };
    assert_eq!(int("255").base, IntBase::Decimal);
    assert_eq!(int("0xff").base, IntBase::Hexadecimal);
    assert_eq!(int("0b1010").base, IntBase::Binary);
    assert_eq!(int("0o17").base, IntBase::Octal);
    // Same value three ways: the base is a choice the reader made, the value is not.
    assert_eq!(int("0xff").value, IntValue::Small(255));
    assert_eq!(int("0b11111111").value, IntValue::Small(255));
    assert_eq!(int("255").value, IntValue::Small(255));
}

#[test]
fn bignums_survive_as_digits_in_their_own_base() {
    // Prism hands these back as base-2^32 limbs, so this is the one place the
    // lowering does arithmetic rather than copying.
    let ExprKind::Int(lit) = only("123456789012345678901234567890") else {
        panic!("should be an Int");
    };
    assert_eq!(
        lit.value,
        IntValue::Big("123456789012345678901234567890".into())
    );

    let ExprKind::Int(lit) = only("0xffffffffffffffffffff") else {
        panic!("should be an Int");
    };
    assert_eq!(lit.base, IntBase::Hexadecimal);
    assert_eq!(lit.value, IntValue::Big("ffffffffffffffffffff".into()));

    // The boundary between the i64 path and the digit-string path.
    let ExprKind::Int(lit) = only("9223372036854775807") else {
        panic!("should be an Int");
    };
    assert_eq!(lit.value, IntValue::Small(i64::MAX));
    let ExprKind::Int(lit) = only("9223372036854775808") else {
        panic!("should be an Int");
    };
    assert_eq!(lit.value, IntValue::Big("9223372036854775808".into()));
}

#[test]
fn rationals_keep_the_reduced_pair_prism_computed() {
    // `1.5r` is 3/2, and neither digit appears in the source, so this cannot be
    // recovered by slicing the literal.
    let ExprKind::Rational(r) = only("1.5r") else {
        panic!("should be a Rational");
    };
    assert_eq!(r.numerator, IntValue::Small(3));
    assert_eq!(r.denominator, IntValue::Small(2));
}

#[test]
fn string_content_is_bytes_not_utf8() {
    // A one-byte `"\xFF"` is a valid Ruby String and is not valid UTF-8. If the
    // tree held a `String` this literal would be unrepresentable.
    let ExprKind::Str(s) = only(r#""\xFF""#) else {
        panic!("should be a Str");
    };
    assert_eq!(s.parts, vec![StrPart::Bytes(Box::from(&b"\xFF"[..]))]);
}

#[test]
fn frozen_string_literal_comment_reaches_the_literal() {
    let source = "# frozen_string_literal: true\n\"a\"\n";
    let ExprKind::Str(s) = body(source).pop().expect("one statement").kind else {
        panic!("should be a Str");
    };
    assert_eq!(s.frozen, Some(true));

    // Without the comment the file said nothing, which is not the same as false.
    let ExprKind::Str(s) = only("\"a\"") else {
        panic!("should be a Str");
    };
    assert_eq!(s.frozen, None);
}

#[test]
fn interpolation_is_a_flat_list_of_runs_and_holes() {
    let ExprKind::Str(s) = only(r#""a#{b}c""#) else {
        panic!("should be a Str");
    };
    assert_eq!(s.parts.len(), 3, "{:?}", s.parts);
    assert!(matches!(s.parts[0], StrPart::Bytes(_)));
    assert!(matches!(s.parts[1], StrPart::Interp(_)));
    assert!(matches!(s.parts[2], StrPart::Bytes(_)));
}

#[test]
fn until_keeps_its_keyword_rather_than_becoming_not_while() {
    let ExprKind::While(w) = only("until a\n b\nend") else {
        panic!("should be a While");
    };
    assert!(w.until, "`until` is a choice the reader made");
    assert!(!w.post);

    // `begin ... end while` runs the body once first. Losing that flag would
    // change what the program does.
    let ExprKind::While(w) = only("begin\n b\nend while a") else {
        panic!("should be a While");
    };
    assert!(w.post);
}

#[test]
fn unless_keeps_its_keyword() {
    let ExprKind::If(i) = only("unless a\n b\nend") else {
        panic!("should be an If");
    };
    assert!(i.unless);
}

#[test]
fn elsif_nests_rather_than_flattening() {
    let ExprKind::If(i) = only("if a\n 1\nelsif b\n 2\nelse\n 3\nend") else {
        panic!("should be an If");
    };
    let else_body = i.else_body.expect("elsif is the else branch");
    assert_eq!(else_body.len(), 1);
    assert!(matches!(else_body[0].kind, ExprKind::If(_)));
}

#[test]
fn rescue_chains_become_a_list() {
    let ExprKind::Begin(b) =
        only("begin\n a\nrescue IOError => e\n b\nrescue TypeError\n c\nelse\n d\nensure\n e\nend")
    else {
        panic!("should be a Begin");
    };
    assert_eq!(b.rescues.len(), 2, "rescues sit side by side, not nested");
    assert!(b.rescues[0].reference.is_some());
    assert!(b.rescues[1].reference.is_none());
    assert!(b.else_body.is_some());
    assert!(b.ensure_body.is_some());
}

#[test]
fn a_method_body_with_rescue_needs_no_begin() {
    let ExprKind::Begin(b) = in_def(only("def f\n a\nrescue\n b\nend")) else {
        panic!("a bare rescue in a def is still a Begin");
    };
    assert_eq!(b.rescues.len(), 1);
}

#[test]
fn a_guard_is_lifted_out_of_the_pattern() {
    // Prism hangs `if n > 0` above the pattern as an `if` whose body is the
    // pattern. The tree puts it in `InClause::guard`, where a reader looks.
    let ExprKind::Case(case) = only("case v\nin Integer => n if n > 0\n n\nend") else {
        panic!("should be a Case");
    };
    let CaseBranches::In(ins) = case.branches else {
        panic!("should be `in` branches");
    };
    assert_eq!(ins.len(), 1);
    assert!(matches!(ins[0].guard, Some(Guard::If(_))));
    assert!(matches!(ins[0].pattern.kind, ExprKind::CapturePattern(_)));

    let ExprKind::Case(case) = only("case v\nin Integer unless false\n n\nend") else {
        panic!("should be a Case");
    };
    let CaseBranches::In(ins) = case.branches else {
        panic!("should be `in` branches");
    };
    assert!(matches!(ins[0].guard, Some(Guard::Unless(_))));
}

#[test]
fn pattern_bindings_read_as_variables() {
    // `in [x]` hands over a LocalVariableTargetNode where the tree wants an
    // Expr. Found by sweeping ruby/spec, which had eleven files like this.
    let ExprKind::Case(case) = only("case v\nin [x, *rest]\n x\nend") else {
        panic!("should be a Case");
    };
    let CaseBranches::In(ins) = case.branches else {
        panic!("should be `in` branches");
    };
    let ExprKind::ArrayPattern(p) = &ins[0].pattern.kind else {
        panic!("should be an array pattern");
    };
    assert!(matches!(
        p.requireds[0].kind,
        ExprKind::Var(VarRef::Local { .. })
    ));
    let Some(rest) = &p.rest else {
        panic!("should have a rest");
    };
    assert!(matches!(rest.kind, ExprKind::Splat(Some(_))));
}

#[test]
fn a_trailing_comma_in_a_pattern_is_a_splat_that_binds_nothing() {
    let ExprKind::Case(case) = only("case v\nin [0, 1, ]\n x\nend") else {
        panic!("should be a Case");
    };
    let CaseBranches::In(ins) = case.branches else {
        panic!("should be `in` branches");
    };
    let ExprKind::ArrayPattern(p) = &ins[0].pattern.kind else {
        panic!("should be an array pattern");
    };
    assert!(matches!(
        p.rest.as_ref().map(|r| &r.kind),
        Some(ExprKind::Splat(None))
    ));
}

#[test]
fn destructuring_block_parameters_lower() {
    // `{ |(a, b)| }` nests parameters inside a multi-target, not the target
    // nodes every other multi-target holds. Also found by the ruby/spec sweep.
    let ExprKind::Call(call) = only("xs.each { |(a, b)| a }") else {
        panic!("should be a Call");
    };
    let Some(BlockArg::Block(block)) = call.block else {
        panic!("should carry a block");
    };
    let Params::Explicit(params) = block.params else {
        panic!("should have explicit parameters");
    };
    assert_eq!(params.required.len(), 1);
    let RequiredParamKind::Destructure(multi) = &params.required[0].kind else {
        panic!("should destructure");
    };
    assert_eq!(multi.lefts.len(), 2);
}

fn block_params(source: &str) -> Params {
    let ExprKind::Call(call) = only(source) else {
        panic!("{source:?} should be a Call");
    };
    match call.block {
        Some(BlockArg::Block(block)) => block.params,
        _ => panic!("{source:?} should carry a block"),
    }
}

#[test]
fn a_block_cannot_be_both_numbered_and_named() {
    assert!(matches!(
        block_params("xs.each { _1 }"),
        Params::Numbered(1)
    ));
    assert!(matches!(block_params("xs.each { it }"), Params::It));
    assert!(matches!(
        block_params("xs.each { |a| a }"),
        Params::Explicit(_)
    ));
    assert!(matches!(block_params("xs.each { 1 }"), Params::None));
}

#[test]
fn parameters_carry_their_own_spans() {
    // `duplicated argument name` names one parameter out of a list, so each one
    // needs somewhere for the warning to point.
    let ExprKind::Def(def) = only("def f(a, b = 1, *c, d, e:, f: 2, **g, &h); end") else {
        panic!("should be a Def");
    };
    let Params::Explicit(p) = def.params else {
        panic!("should have explicit parameters");
    };
    assert_eq!(p.required.len(), 1);
    assert_eq!(p.optional.len(), 1);
    assert!(p.rest.is_some());
    assert_eq!(p.posts.len(), 1);
    assert_eq!(p.keywords.len(), 2);
    assert!(p.keyword_rest.is_some());
    assert!(p.block.is_some());
    assert!(!p.required[0].span.is_empty(), "spans are real, not zero");
    assert!(!p.keywords[0].span.is_empty());
}

#[test]
fn forwarding_parameters_land_in_the_keyword_rest_slot() {
    let ExprKind::Def(def) = only("def f(...) = g(...)") else {
        panic!("should be a Def");
    };
    assert!(def.endless);
    let Params::Explicit(p) = def.params else {
        panic!("should have explicit parameters");
    };
    assert!(matches!(
        p.keyword_rest.map(|k| k.kind),
        Some(KeywordRestKind::Forwarding)
    ));
}

#[test]
fn no_keywords_is_not_the_same_as_no_keyword_rest() {
    let ExprKind::Def(def) = only("def f(**nil); end") else {
        panic!("should be a Def");
    };
    let Params::Explicit(p) = def.params else {
        panic!("should have explicit parameters");
    };
    assert!(matches!(
        p.keyword_rest.map(|k| k.kind),
        Some(KeywordRestKind::Forbidden)
    ));
}

#[test]
fn bare_super_is_not_super_with_no_arguments() {
    // `super` forwards the caller's arguments; `super()` passes none. Folding
    // them together would change what the program does.
    let ExprKind::Super(s) = in_def(only("def f; super; end")) else {
        panic!("should be a Super");
    };
    assert!(s.args.is_none(), "bare `super` forwards");

    let ExprKind::Super(s) = in_def(only("def f; super(); end")) else {
        panic!("should be a Super");
    };
    assert_eq!(s.args, Some(vec![]), "`super()` passes nothing");
}

#[test]
fn break_with_several_values_is_break_with_an_array() {
    let ExprKind::Break(Some(value)) = in_block(only("xs.each { break 1, 2 }")) else {
        panic!("should be a Break with a value");
    };
    assert!(matches!(value.kind, ExprKind::Array(_)));

    let ExprKind::Break(Some(value)) = in_block(only("xs.each { break 1 }")) else {
        panic!("should be a Break with a value");
    };
    assert!(matches!(value.kind, ExprKind::Int(_)));

    assert!(matches!(
        in_block(only("xs.each { break }")),
        ExprKind::Break(None)
    ));
}

#[test]
fn spans_point_at_the_thing_a_diagnostic_would_underline() {
    let source = "x = 1";
    let assign = &body(source)[0];
    assert_eq!(assign.span, Span::new(0, 5));

    let ExprKind::Assign(a) = &assign.kind else {
        panic!("should be an Assign");
    };
    // The warning is `assigned but unused variable - x`, so the target span is
    // `x` and not the whole statement.
    assert_eq!(a.target.span, Span::new(0, 1));
    assert_eq!(a.value.span, Span::new(4, 5));
}

#[test]
fn hash_entries_carry_spans_for_the_duplicate_key_warning() {
    let ExprKind::Hash(h) = only("{a: 1, a: 2}") else {
        panic!("should be a Hash");
    };
    assert_eq!(h.entries.len(), 2);
    assert!(h.braces);
    assert_ne!(h.entries[0].span, h.entries[1].span);

    // `f(a: 1)` is the same shape without braces, and the difference is kept.
    let ExprKind::Call(call) = only("f(a: 1)") else {
        panic!("should be a Call");
    };
    let ExprKind::Hash(h) = &call.args[0].kind else {
        panic!("keyword arguments are a bare hash");
    };
    assert!(!h.braces);
}

#[test]
fn a_syntax_error_still_produces_a_tree() {
    // Prism recovers, and both halves have a reader: `spinel run` wants the
    // error, an editor wants the tree anyway.
    let parsed = parse(b"def foo(\n");
    assert!(!parsed.is_ok());
    assert_eq!(parsed.syntax_errors().count(), parsed.errors.len());
    assert_eq!(parsed.lowering_bugs().count(), 0);
    assert!(!parsed.program.body.is_empty(), "the tree survives");
}

#[test]
fn lowering_bugs_are_told_apart_from_syntax_errors() {
    // The sweep's whole design rests on this: ruby/spec ships files that are
    // deliberately invalid, and those must not read as parser bugs.
    let parsed = parse(b"x = 1");
    assert!(parsed.is_ok());
    assert_eq!(parsed.lowering_bugs().count(), 0);

    let parsed = parse(b"1 +");
    assert!(parsed.errors.iter().all(|d| d.origin == Origin::Syntax));
}

#[test]
fn locals_are_recorded_per_scope() {
    let parsed = parse(b"a = 1\ndef f; b = 2; end\n");
    assert_eq!(parsed.program.locals, vec!["a".into()]);
    let ExprKind::Def(def) = &parsed.program.body[1].kind else {
        panic!("should be a Def");
    };
    assert_eq!(def.locals, vec!["b".into()]);
}

#[test]
fn a_captured_local_records_its_depth() {
    let outer = body("a = 1; xs.each { a }").pop().expect("two statements");
    let ExprKind::Var(VarRef::Local { depth, .. }) = in_block(outer.kind) else {
        panic!("should read a local");
    };
    assert_eq!(depth, 1, "the local lives one scope out");
}

#[test]
fn an_empty_file_is_an_empty_program() {
    let parsed = parse(b"");
    assert!(parsed.is_ok());
    assert!(parsed.program.body.is_empty());
}

#[test]
fn invalid_utf8_source_does_not_panic() {
    // A file in a non-UTF-8 encoding is still a Ruby file.
    let parsed = parse(b"# encoding: binary\nx = \"\xFF\xFE\"\n");
    assert_eq!(parsed.lowering_bugs().count(), 0);
}
