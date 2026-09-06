//! The numeric coercion protocol, held to what CRuby actually does.
//!
//! The expectations are in `tests/coerce.txt`, measured by
//! `scripts/coerce-oracle.rb`, the same arrangement `coerce.txt`,
//! `eval.txt` and `ancestors.txt` use.
//!
//! Worth measuring rather than reading because the protocol is three rules that
//! disagree on purpose. An operand with no `coerce` is a TypeError to `+`, a
//! plain `nil` to `<=>`, and an ArgumentError to `<`. A `coerce` answering nil
//! is "no opinion" to `<=>` and still a TypeError to `+`. A `coerce` answering
//! the wrong *shape* is a TypeError to all three, because that is a broken
//! object rather than a refusal to compare. And the retry runs on the coerced
//! pair, so `4.2 + obj` can legitimately end in `String#+`.
//!
//! This file runs the whole core library, because the protocol *is*
//! `core/numeric.rb` — the VM's `Insn::BinOp` fast path never reaches it.

//! Skipped under miri: `spinel_parse` calls into Prism, which is C.
#![cfg(not(miri))]

use spinel_vm::Heap;
use spinel_vm::compile;
use spinel_vm::interp;

const TABLE: &str = include_str!("coerce.txt");
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
            panic!("coerce.txt:{}: no `#=>` on a non-comment line", index + 1);
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
                "coerce.txt:{}: {source}\n  ruby:   {want}\n  spinel: {got}",
                index + 1
            )),
            Err(why) => wrong.push(format!("coerce.txt:{}: {source}\n  {why}", index + 1)),
        }
    }
    // Every row, skipped or not: the tripwire is against the table shrinking,
    // and a row moving onto the skip list must not quietly disarm it.
    assert!(rows > 50, "the table lost its cases: {rows} rows left");
    assert!(
        checked > 50,
        "too much of the table is skipped: {checked} run"
    );
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

/// The rows above that are skipped, and the reason each one is.
///
/// A list rather than a comment, because `skipped_rows_still_refuse` runs it: a
/// row that starts answering — rightly or wrongly — fails here instead of
/// sitting skipped for a slice that already fixed it.
///
/// Both entries are the same shape, and neither is the protocol's doing. The
/// coercion path routes the retry onto the coerced pair correctly; the class it
/// lands on is the one that has not been written yet.
const SKIPPED: &[(&str, &str)] = &[];
