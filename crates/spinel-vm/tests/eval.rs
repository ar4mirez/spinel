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
        // #11; what stays is what later slices own.
        "@a = 1",
        "$a = 1",
        "A = 1",
        "{ a: 1 }",
        "(1..2)",
        "case 1; in Integer then 2; end",
        "begin; 1; rescue; 2; end",
        "'a' + \"#{1}\"",
    ] {
        let parsed = spinel_parse::parse(source.as_bytes());
        assert!(
            compile::program(&parsed.program).is_err(),
            "{source:?} compiled, but this slice cannot mean it"
        );
    }
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
