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
        // `skips_are_still_needed` fails once one of them answers rightly.
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
/// A list rather than a comment, because `skips_are_still_needed` runs it: a
/// row that starts answering rightly fails there instead of sitting skipped
/// for a slice that already fixed it.
const SKIPPED: &[(&str, &str)] = &[(
    "k = Class.new { [1].each { def deep; end } }; k.instance_methods(false)",
    "a `def` in a block inside the `Class.new` block is private, the top level's default",
)];

/// Every skipped row still gets CRuby's answer wrong, or refuses it. One that
/// agrees has been fixed by some slice, and comes off the list (#28 took the
/// two `allocate` rows off this way).
#[test]
fn skips_are_still_needed() {
    let mut fixed = Vec::new();
    for (index, line) in TABLE.lines().enumerate() {
        let Some((source, want)) = line.split_once(SEPARATOR) else {
            continue;
        };
        if line.trim_start().starts_with('#')
            || !SKIPPED.iter().any(|(prefix, _)| source.starts_with(prefix))
        {
            continue;
        }
        if eval(source).as_deref() == Ok(want) {
            fixed.push(format!("anonymous.txt:{}: {source}", index + 1));
        }
    }
    assert!(
        fixed.is_empty(),
        "skipped, but answering as CRuby does:\n{}",
        fixed.join("\n")
    );
}

/// Reopening a class does not fire `inherited` —
/// the guard is on definition, not on the `class` keyword.
#[test]
fn reopening_does_not_refuse() {
    let source = "class P; def self.inherited(sub); end; end; class P; def m; 1; end; end; P.new.m";
    assert_eq!(eval(source).as_deref(), Ok("1"));
}

/// `inherited` fires on definition, through both spellings, with the new
/// class (#28). It used to be refused: skipping it would report a state the
/// program never reached.
#[test]
fn inherited_fires() {
    for source in [
        "$s = []; p1 = Class.new { def self.inherited(sub) = $s << sub.superclass.equal?(self) }; Class.new(p1); $s",
        "$s = []; class P; def self.inherited(sub) = $s << sub.superclass.equal?(self); end; class C < P; end; $s",
    ] {
        assert_eq!(eval(source), Ok("[true]".to_owned()), "{source:?}");
    }
}
