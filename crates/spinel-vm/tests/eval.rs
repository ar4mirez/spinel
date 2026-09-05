//! End-to-end: Ruby source in, a value out.
//!
//! The expectations are not written here. `tests/eval.txt` holds them and
//! `scripts/eval-oracle.rb` is what measured them against a real Ruby, the same
//! arrangement `ancestors.txt` uses for #8 and for the same reason: several of
//! the answers are not what reading the code would suggest. `-7 / 2` is -4 and
//! not -3, `!0` is false, and `1 && 2` is 2. A table CI re-measures cannot drift
//! into agreeing with a bug.
//!
//! What stays in this file is everything the table cannot hold: the cases where
//! Spinel deliberately *refuses*, because Ruby's answer there is a value this
//! slice does not have.

//! Skipped under miri: `spinel_parse` calls into Prism, which is C, and miri
//! cannot run foreign functions. What miri is here to check is the heap's
//! pointer arithmetic, and the interpreter's share of that is covered by
//! `interp::tests::the_interpreter_allocates_and_reads_under_miri`, which builds
//! its `Iseq` by hand and needs no parser.
#![cfg(not(miri))]

use spinel_vm::Heap;
use spinel_vm::compile;
use spinel_vm::interp;

/// The measured table. Kept beside this file so a case is added in one place.
const TABLE: &str = include_str!("eval.txt");
const SEPARATOR: &str = "  #=> ";

/// Compile and run `source`, and render the result the way a report would.
fn eval(source: &str) -> Result<String, String> {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(
        parsed.errors.is_empty(),
        "{source:?} did not parse: {:?}",
        parsed.errors
    );
    let iseq = compile::program(&parsed.program).map_err(|e| format!("unsupported: {e}"))?;
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(iseq.locals.len());
    let mut scope = heap.scope();
    scope.bootstrap();
    // The core library, not just the VM. `Exception#message` and the rest are
    // `core/*.rb` since #151 moved them off fixed slots onto instance
    // variables, and a table that measured the VM without its core library
    // would be measuring a language nobody runs.
    spinel_core::boot(&mut scope);
    let value =
        interp::eval_in(&mut scope, &mut frame, &iseq).map_err(|e| format!("error: {e}"))?;
    Ok(interp::inspect(&mut scope, value))
}

#[test]
fn spinel_agrees_with_the_ruby_that_measured_the_table() {
    let mut checked = 0;
    let mut wrong = Vec::new();
    for (index, line) in TABLE.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((source, want)) = line.split_once(SEPARATOR) else {
            panic!("eval.txt:{}: no `#=>` on a non-comment line", index + 1);
        };
        checked += 1;
        match eval(source) {
            Ok(got) if got == want => {}
            Ok(got) => wrong.push(format!(
                "eval.txt:{}: {source}\n  ruby:   {want}\n  spinel: {got}",
                index + 1
            )),
            Err(why) => wrong.push(format!("eval.txt:{}: {source}\n  {why}", index + 1)),
        }
    }
    assert!(checked > 50, "the table lost its cases: {checked} left");
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn a_construct_this_slice_does_not_compile_is_an_error_never_a_guess() {
    // The property the spec harness depends on: unsupported is loud.
    for source in [
        // `def` and a block literal moved to the other side of this list with
        // #11, and constants, class bodies, and `defined?` with #13; what stays
        // is what later slices own.
        "$a = 1",
        "@@a = 1",
        // #13 answers `defined?` for the kinds it can mean, and refuses the
        // kinds it cannot rather than answering Ruby's `nil` for the wrong
        // reason. See `Compiler::defined`. `defined?(@a)` left this list with
        // #151: an object with a shape can say whether it holds `@a`, so the
        // `nil` is now an answer rather than a coincidence.
        "defined?($a)",
        "defined?(@@a)",
        "A ||= 1",
        "{ a: 1 }",
        "(1..2)",
        "case 1; in Integer then 2; end",
        "'a' + \"#{1}\"",
    ] {
        let parsed = spinel_parse::parse(source.as_bytes());
        assert!(
            compile::program(&parsed.program).is_err(),
            "{source:?} compiled, but this slice cannot mean it"
        );
    }
}

/// The cases `eval.txt` cannot hold, because the oracle only records values.
///
/// ruby/spec asserts on this text, so it is measured against `ruby 4.0.6` here
/// rather than paraphrased. #12 turns each of these into a real exception
/// object; the wording should already be right when it does.
#[test]
fn naming_errors_carry_rubys_own_message() {
    for (source, want) in [
        ("Nope", "uninitialized constant Nope"),
        ("::Nope", "uninitialized constant Nope"),
        ("module M; end; M::Nope", "uninitialized constant M::Nope"),
        ("class C; end; C::Nope", "uninitialized constant C::Nope"),
        // Ruby dropped `Object` as a fallback for a qualified lookup in 2.5, so
        // a top-level constant is *not* reachable through a subclass.
        (
            "TOP = 1; class B; end; class S < B; end; S::TOP",
            "uninitialized constant S::TOP",
        ),
        ("1::Nope", "1 is not a class/module"),
        (
            "class B; end; class S < B; end; class S < String; end",
            "superclass mismatch for class S",
        ),
        (
            "class S < 1; end",
            "superclass must be an instance of Class (given an instance of Integer)",
        ),
        ("module M; end; class M; end", "M is not a class"),
        ("class C; end; module C; end", "C is not a module"),
        ("class << 1; end", "can't define singleton"),
    ] {
        let err = eval(source).unwrap_err();
        assert!(
            err.contains(want),
            "{source:?}\n  want: {want}\n  got:  {err}"
        );
    }
}

/// `defined?` never runs what it is asked about, beyond a receiver chain.
#[test]
fn defined_does_not_evaluate_what_it_reports_on() {
    // The assignment does not happen: Ruby answers `"assignment"` and leaves
    // the local alone.
    assert_eq!(eval("a = 1; defined?(a = 2); a").unwrap(), "1");
    // The method is not called, though naming it is enough to answer.
    assert_eq!(
        eval("class C; def self.boom; raise 'never'; end; end; defined?(C.boom)").unwrap(),
        "\"method\""
    );
}

#[test]
fn dividing_by_zero_says_what_ruby_would_raise() {
    let err = eval("1 / 0").unwrap_err();
    assert!(err.contains("ZeroDivisionError"), "{err}");
}

#[test]
fn a_loop_that_does_not_end_is_stopped_rather_than_hanging() {
    let err = eval("while true; end").unwrap_err();
    assert!(err.contains("budget"), "{err}");
}
