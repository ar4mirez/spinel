//! `Array#flatten` against nesting depth, and against recursive arrays with
//! and without a level (#741).
//!
//! ruby/spec checks that an unlimited flatten of a recursive array raises,
//! but not that a limited one does not, nor that a deep array flattens in
//! time linear in its depth. The answers below were measured on
//! `ruby 4.0.9 (2026-10-23 revision 3e93b23afc)`.
//!
//! This file runs the whole core library, because `flatten` is
//! `core/array.rb`.

//! Skipped under miri: `spinel_parse` calls into Prism, which is C.
#![cfg(not(miri))]

use spinel_vm::Heap;
use spinel_vm::compile;
use spinel_vm::interp;

/// Compile and run `source` against a booted heap: a value inspected, a raise
/// as `!Class: message`. A recursive answer is inspected by the snippet, in
/// Ruby, which knows to print `[...]`.
fn eval(source: &str) -> String {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(
        parsed.errors.is_empty(),
        "{source:?} did not parse: {:?}",
        parsed.errors
    );
    let iseq = compile::program(&parsed.program).expect("compiles");
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(iseq.locals.len());
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);
    match interp::eval_in(&mut scope, &mut frame, &iseq) {
        Ok(value) => interp::inspect(&mut scope, value),
        Err(interp::Error::Uncaught { class, message }) => format!("!{class}: {message}"),
        Err(interp::Error::Raise { class, message }) => format!("!{class}: {message}"),
        Err(other) => panic!("{source:?}: {other}"),
    }
}

/// What CRuby answers for each snippet.
const CASES: &[(&str, &str)] = &[
    (
        "a = [1]; a << a; a.flatten",
        "!ArgumentError: tried to flatten recursive array",
    ),
    (
        "a = [1]; a << a; a.flatten(0).inspect",
        "\"[1, [1, [...]]]\"",
    ),
    (
        "a = [1]; a << a; a.flatten(1).inspect",
        "\"[1, 1, [1, [...]]]\"",
    ),
    (
        "a = [1]; a << a; a.flatten(3).inspect",
        "\"[1, 1, 1, 1, [1, [...]]]\"",
    ),
    (
        "a = [1]; a << a; a.flatten(-2)",
        "!ArgumentError: tried to flatten recursive array",
    ),
    (
        "x = []; y = []; x << y; y << x; [x].flatten(4).inspect",
        "\"[[[[...]]]]\"",
    ),
    ("a = [1]; a << a; a.flatten!(1); a.size", "3"),
    // The same array twice on one side of the walk, and on both: not
    // recursive.
    (
        "b = [1, 2]; [b, [b, [b]], b].flatten",
        "[1, 2, 1, 2, 1, 2, 1, 2]",
    ),
    // A cycle back to each part of a 40-deep path, past the 16 arrays the
    // walk compares one by one as well as within them.
    (
        "(0...40).map { |back| c = [[:leaf]]; 39.times { c << [c.last] }; \
          t = c.last; n = t; 39.times { n = n[0] }; g = t; back.times { g = g[0] }; \
          n << g; begin; t.flatten; :none; rescue ArgumentError; :raised; end }.uniq",
        "[:raised]",
    ),
    (
        "s = [1]; d = s; 40.times { d = [d, s] }; d.flatten.size",
        "41",
    ),
];

#[test]
fn flatten_agrees_with_cruby_on_recursive_arrays_and_levels() {
    let wrong: Vec<String> = CASES
        .iter()
        .filter_map(|(source, want)| {
            let got = eval(source);
            (got != *want).then(|| format!("{source}\n  ruby:   {want}\n  spinel: {got}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Cubic in depth before #741: 2,000 levels took 21 s in a release build. A
/// debug build that is linear takes a few seconds here.
#[test]
fn a_deep_array_flattens() {
    assert_eq!(
        eval("a = [1]; 5_000.times { a = [a] }; [a.flatten, a.flatten(4_999).size]"),
        "[[1], 1]"
    );
}
