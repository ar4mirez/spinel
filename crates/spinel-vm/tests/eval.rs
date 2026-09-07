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
    eval_in_file("(eval)", source)
}

/// The same, for source that came from a named file.
fn eval_in_file(path: &str, source: &str) -> Result<String, String> {
    let parsed = spinel_parse::parse_file(path, source.as_bytes());
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
        "@@a = 1",
        // #13 answers `defined?` for the kinds it can mean, and refuses the
        // kinds it cannot rather than answering Ruby's `nil` for the wrong
        // reason. See `Compiler::defined`. `defined?(@a)` left this list with
        // #151: an object with a shape can say whether it holds `@a`, so the
        // `nil` is now an answer rather than a coincidence, and `$a` with #166
        // for the same reason: the heap's table knows whether one was assigned.
        "defined?(@@a)",
        // A back-reference is read off the last match, not out of the global
        // table, so #166 deliberately leaves it where #14 put it.
        "defined?($&)",
        "$~ = nil",
        "A ||= 1",
        // A hash literal, a range literal, an array splat, a multiple
        // assignment and string interpolation left this list with #157 and
        // #154. What replaces them is the call-convention half of the same
        // syntax, which #11 owns: `CallSite::keywords` names each keyword by
        // symbol, so a non-symbol key and a `**` argument have nowhere to go.
        // A destructuring parameter binds several names in one slot, so
        // anything after it would be bound to the wrong one. See
        // `Compiler::spec_from_list`.
        "proc { |(a, b), c| }",
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

/// `__FILE__` and `__LINE__` (#174).
///
/// Not rows in `eval.txt`: the oracle's `eval` answers `"(eval at ...)"` for
/// `__FILE__`, which is a path into the oracle script and not a fact about
/// Ruby that Spinel can be held to. The *rules* are what is measured here —
/// the path the source was parsed with, and the line the keyword was written
/// on — and both were checked against ruby 4.0.6 by running the same shapes
/// from a file.
#[test]
fn source_position_keywords_answer_the_file_and_the_line() {
    assert_eq!(
        eval_in_file("lt.rb", "__FILE__"),
        Ok("\"lt.rb\"".to_owned()),
        "`__FILE__` is the path the source was parsed with"
    );
    assert_eq!(
        eval_in_file("a/b.rb", "__FILE__"),
        Ok("\"a/b.rb\"".to_owned()),
        "as given, not resolved: `ruby lt.rb` answers \"lt.rb\""
    );
    // Source with no file. CRuby says "(eval at <where>)"; the part Spinel can
    // agree with is that it names no file of the program's.
    assert_eq!(eval("__FILE__"), Ok("\"(eval)\"".to_owned()));

    // One row per line, so an off-by-one in either direction shows up.
    assert_eq!(eval("__LINE__"), Ok("1".to_owned()));
    assert_eq!(eval("\n__LINE__"), Ok("2".to_owned()));
    assert_eq!(eval("nil\nnil\n__LINE__"), Ok("3".to_owned()));
    assert_eq!(
        eval("[__LINE__,\n __LINE__]"),
        Ok("[1, 2]".to_owned()),
        "each keyword answers its own line, not the expression's"
    );
    // A `\r\n` file counts the same lines: the offset table splits on `\n`.
    assert_eq!(eval("nil\r\n__LINE__"), Ok("2".to_owned()));
}

/// `__ENCODING__` is deferred, and says so under its own name (#174).
///
/// Refusing beats answering: it needs an `Encoding` object, and a wrong one
/// would make `__ENCODING__.name` a measurement coincidence.
#[test]
fn the_encoding_keyword_is_refused_under_its_own_reason() {
    let error = eval("__ENCODING__").expect_err("`__ENCODING__` has no object yet");
    assert!(
        error.contains("Encoding"),
        "the reason has to name what is missing, not the keyword family: {error}"
    );
}

/// `super` off the end of the chain, and `super` where there is no method
/// (#187).
///
/// Not rows in `eval.txt`: the table records values, and both of these raise.
/// The messages are CRuby's, measured on ruby 4.0.6.
#[test]
fn super_with_nothing_above_it_raises_the_way_ruby_does() {
    let error = eval("class ZZ; def m; super; end; end; ZZ.new.m")
        .expect_err("`super` past the end of the chain raises");
    assert!(
        error.contains("super: no superclass method 'm'"),
        "Ruby names the keyword in the message, and super_spec.rb asserts on \
         it: {error}"
    );

    // Outside a method the compiler cannot even build the argument list, so it
    // refuses rather than emitting a call that would raise at run time.
    let error = eval("super").expect_err("`super` at the top level is not a call");
    assert!(
        error.contains("super"),
        "the refusal has to name the keyword: {error}"
    );
}

/// Pattern matching's two protocols and its two raises (#165).
///
/// Not rows in `eval.txt`: two of these answer a `Hash`, whose `inspect`
/// Spinel still writes the pre-3.4 way, and two raise. The values are CRuby's,
/// measured on ruby 4.0.6.
#[test]
fn pattern_matching_protocols_and_raises() {
    // `**rest` binds what the pattern did not name.
    assert_eq!(
        eval("case({a: 1, b: 2}); in {a: Integer, **r} then r.keys; end"),
        Ok("[:b]".to_owned())
    );
    // `deconstruct_keys` is handed the keys the pattern names, or `nil` when a
    // `**rest` means it may want all of them.
    assert_eq!(
        eval(
            "o = Object.new; def o.deconstruct_keys(k); $seen = k; {a: 1}; end; \
             (o in {a: Integer}); $seen"
        ),
        Ok("[:a]".to_owned())
    );
    assert_eq!(
        eval(
            "o = Object.new; def o.deconstruct_keys(k); $seen = k; {a: 1}; end; \
             (o in {a: Integer, **r}); $seen"
        ),
        Ok("nil".to_owned())
    );
    // An object with a `deconstruct` matches an array pattern...
    assert_eq!(
        eval("o = Object.new; def o.deconstruct; [1, 2]; end; o in [1, 2]"),
        Ok("true".to_owned())
    );
    // ...and one whose `deconstruct` answers something else is an error, not a
    // failed match.
    let error = eval("o = Object.new; def o.deconstruct; 5; end; o in [1]")
        .expect_err("a `deconstruct` that is not an Array raises");
    assert!(
        error.contains("deconstruct must return Array"),
        "CRuby's own message: {error}"
    );

    // No `deconstruct` at all is a failed match rather than an error.
    assert_eq!(eval("1 in [a]"), Ok("false".to_owned()));

    // `case`/`in` with nothing matching and no `else`, and the `=>` form.
    for source in ["case [0, 1]; in String then 1; end", "[0, 1] => String"] {
        let error = eval(source).expect_err("a pattern that matches nothing raises");
        assert!(
            error.contains("NoMatchingPatternError") && error.contains("[0, 1]"),
            "the message names the subject, which the spec asserts on: {error}"
        );
    }

    // A hash pattern that fails on a *missing key* raises the key error rather
    // than the general one — but only where CRuby names it, which is a form
    // with one pattern. Measured on ruby 4.0.6; a second clause makes even the
    // same failing pattern report generally.
    for (source, want) in [
        ("case({a: 1}); in {b: 2}; end", "NoMatchingPatternKeyError"),
        ("{a: 1} => {b: 2}", "NoMatchingPatternKeyError"),
        // Nested: the key is the inner one, the subject is still the outer.
        (
            "case({x: {a: 1}}); in {x: {b: 2}}; end",
            "NoMatchingPatternKeyError",
        ),
        // A guard does not change which error the pattern failed with.
        (
            "case({a: 1}); in {b: 2} if true; end",
            "NoMatchingPatternKeyError",
        ),
        // Two clauses: the general error, even though a key was missing.
        (
            "case({a: 1}); in {b: 2}; in {c: 3}; end",
            "NoMatchingPatternError",
        ),
        (
            "case({a: 1}); in [x]; in {b: 2}; end",
            "NoMatchingPatternError",
        ),
        // The key was there; the value disagreed. Not a key error.
        (
            "case({a: 1}); in {a: String}; end",
            "NoMatchingPatternError",
        ),
    ] {
        let error = eval(source).expect_err("a pattern that matches nothing raises");
        assert!(
            error.contains(want),
            "{source} should raise {want}, got: {error}"
        );
    }
    // The last miss wins, which is what an alternative naming the *second*
    // pattern's key means.
    let error = eval("case({a: 1}); in {b: 2} | {c: 3}; end").expect_err("neither matches");
    assert!(
        error.contains("key not found: :c"),
        "the message names the last key tried: {error}"
    );
    // `x in pat` answers false and never raises, so it reports no key at all.
    assert_eq!(eval("{a: 1} in {b: 2}"), Ok("false".to_owned()));

    // `deconstruct` is called once per `case` subject, however many clauses
    // try it — observable on an object whose `deconstruct` has an effect.
    assert_eq!(
        eval(
            "$n = 0; o = Object.new; def o.deconstruct; $n = $n + 1; [0, 1]; end; \
             case o; in [1, 2] then :a; in [0, 1] then :b; end; $n"
        ),
        Ok("1".to_owned())
    );
}
