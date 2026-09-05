//! `Class.new` and `Module.new`, held to what CRuby actually does.
//!
//! The expectations are in `tests/anonymous.txt`, measured by
//! `scripts/anonymous-oracle.rb`, the same arrangement `eval.txt` and
//! `ancestors.txt` use — and for a stronger reason than either. Three of the
//! four rules here read backwards: the block moves `self` without moving the
//! lexical scope, so a constant on the superclass is a `NameError` inside it;
//! only the *first* constant assignment names a class; and `inherited` fires
//! before the block despite ruby/spec's example being called "after".
//!
//! This file runs the whole core library, which `eval.rs` does not, because
//! `name`, `to_s`, and `is_a?` are `core/*.rb`'s and half the table asks them.

//! Skipped under miri: `spinel_parse` calls into Prism, which is C.
#![cfg(not(miri))]

use spinel_vm::Heap;
use spinel_vm::compile;
use spinel_vm::interp;

const TABLE: &str = include_str!("anonymous.txt");
const SEPARATOR: &str = "  #=> ";

/// Compile and run `source` against a booted heap, rendering the answer the way
/// the oracle renders CRuby's: a value inspected, a raise as `!Class: message`,
/// and every address flattened to `0xXX` — the shape of `#<Class:0x...>` is the
/// rule, the address is not.
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
    spinel_core::boot(&mut scope);
    let rendered = match interp::eval_in(&mut scope, &mut frame, &iseq) {
        Ok(value) => interp::inspect(&mut scope, value),
        // A refusal is not a Ruby answer, so it is reported as one — the test
        // says which row and the reason, rather than "expected X got !Y".
        Err(err @ interp::Error::Unknowable { .. }) => return Err(format!("refused: {err}")),
        // `Uncaught` is the usual shape: #12 turns a `Raise` into a real
        // exception object the moment it leaves the instruction, and what
        // reaches the top is the object that found no handler. `Raise` still
        // arrives for the few refusals decided outside a frame.
        Err(interp::Error::Uncaught { class, message }) => format!("!{class}: {message}"),
        Err(interp::Error::Raise { class, message }) => format!("!{class}: {message}"),
        Err(other) => return Err(format!("error: {other}")),
    };
    Ok(flatten_addresses(&rendered))
}

/// `0x` followed by hex digits becomes `0xXX`, so two runs of the same snippet
/// agree. Hand-rolled rather than a regex crate: the VM has no dependency on
/// one and this is the only place that wants it.
fn flatten_addresses(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("0x") {
        out.push_str(&rest[..at]);
        let digits = rest[at + 2..]
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(rest.len() - at - 2);
        if digits == 0 {
            out.push_str("0x");
            rest = &rest[at + 2..];
            continue;
        }
        out.push_str("0xXX");
        rest = &rest[at + 2 + digits..];
    }
    out.push_str(rest);
    out
}

#[test]
fn spinel_agrees_with_the_ruby_that_measured_the_table() {
    let mut rows = 0;
    let mut checked = 0;
    let mut wrong = Vec::new();
    for (index, line) in TABLE.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((source, want)) = line.split_once(SEPARATOR) else {
            panic!(
                "anonymous.txt:{}: no `#=>` on a non-comment line",
                index + 1
            );
        };
        rows += 1;
        // Rows this slice deliberately does not answer. Each names why, and
        // each is a refusal rather than a wrong value — the property that
        // matters is that Spinel says so, which `refusals_are_refusals` checks.
        if SKIPPED.iter().any(|(prefix, _)| source.starts_with(prefix)) {
            continue;
        }
        checked += 1;
        match eval(source) {
            Ok(got) if got == want => {}
            Ok(got) => wrong.push(format!(
                "anonymous.txt:{}: {source}\n  ruby:   {want}\n  spinel: {got}",
                index + 1
            )),
            Err(why) => wrong.push(format!("anonymous.txt:{}: {source}\n  {why}", index + 1)),
        }
    }
    // Every row, skipped or not: the tripwire is against the table shrinking,
    // and a row moving onto the skip list must not quietly disarm it.
    assert!(rows > 55, "the table lost its cases: {rows} rows left");
    assert!(
        checked > 35,
        "too much of the table is skipped: {checked} run"
    );
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

/// The rows above that are skipped, and the reason each one is.
///
/// A list rather than a comment, because `refusals_are_refusals` runs it: a row
/// that starts answering — rightly or wrongly — fails here instead of sitting
/// skipped for a slice that already fixed it.
const SKIPPED: &[(&str, &str)] = &[
    (
        "Class.new(Object.new.singleton_class)",
        "`Object#singleton_class` is #28's",
    ),
    (
        "k = Class.new { OracleAssigned = 1 }; k.const_defined?",
        "`Module#const_defined?` is #28's",
    ),
    (
        "k = Class.new { def hi; end }; k.instance_methods(false)",
        "`Module#instance_methods` is #28's",
    ),
    (
        "k = Class.new { [1].each { def deep; end } }; k.instance_methods(false)",
        "`Module#instance_methods` is #28's",
    ),
    (
        "m = Module.new { def q; end }; m.instance_methods(false)",
        "`Module#instance_methods` is #28's",
    ),
    (
        "k = Class.new { @iv = 7 }; k.instance_variable_get",
        "instance variables are not compiled yet (#151)",
    ),
    (
        "x = 5; Class.new { $oracle_local",
        "a global variable is not compiled yet",
    ),
    (
        "k = Class.new { |c| $oracle_arg",
        "a global variable is not compiled yet",
    ),
    (
        "k = Class.new { $oracle_self",
        "a global variable is not compiled yet",
    ),
    (
        "m = Module.new { $oracle_ms",
        "a global variable is not compiled yet",
    ),
    (
        "Class.new { |a, b| $oracle_two",
        "a global variable is not compiled yet",
    ),
    (
        "class OracleQ; CV = \"from-Q\"; end; class OracleR",
        "a global variable is not compiled yet",
    ),
    (
        "p1 = Class.new { def self.inherited",
        "`inherited` refuses rather than firing — see `refusals_are_refusals`",
    ),
    (
        "$oracle_pad = []",
        "`inherited` refuses rather than firing — see `refusals_are_refusals`",
    ),
    (
        "Class.allocate",
        "`Class.allocate` answers an uninitialised class, which #13 shut the door on",
    ),
    (
        "Module.allocate",
        "`Module.allocate` is a `NoMethodError` in Ruby; Spinel refuses instead",
    ),
    (
        "OracleI = Class.new; OracleI.new.to_s",
        "an instance's `to_s` has no address — #15's gap, unchanged",
    ),
];

/// What this slice will not answer, it refuses — it never guesses.
///
/// The `inherited` rows are the load-bearing ones. A VM that defined the class
/// and skipped the hook would report a state the program never reached, which
/// is the failure mode #15 named for `singleton_method_added`.
#[test]
fn refusals_are_refusals() {
    for (source, why) in [
        (
            "p1 = Class.new { def self.inherited(sub); end }; Class.new(p1)",
            "`Class.new` with a hook on the superclass",
        ),
        (
            "class P; def self.inherited(sub); end; end; class C < P; end",
            "the `class` keyword with a hook on the superclass",
        ),
        ("Class.allocate", "an uninitialised class"),
        ("Module.allocate", "an uninitialised module"),
    ] {
        let answer = eval(source);
        assert!(
            matches!(&answer, Err(why) if why.starts_with("refused:")),
            "{why} should refuse, but {source:?} answered {answer:?}"
        );
    }
}

/// Reopening a class does not fire `inherited`, so it must not refuse either —
/// the guard is on definition, not on the `class` keyword.
#[test]
fn reopening_does_not_refuse() {
    let source = "class P; def self.inherited(sub); end; end; class P; def m; 1; end; end; P.new.m";
    assert_eq!(eval(source).as_deref(), Ok("1"));
}
