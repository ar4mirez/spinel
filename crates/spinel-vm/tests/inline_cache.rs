//! Inline caches, through the interpreter rather than through the table's own
//! API — [#169](https://github.com/ar4mirez/spinel/issues/169).
//!
//! `callcache.rs`'s unit tests drive `get` and `fill` directly, which proves
//! the guard and proves nothing about whether `Insn::Send` consults it. These
//! run Ruby: one call site, called twice, with the world changed in between.
//! A cache that failed to invalidate would answer the first call's method the
//! second time, and every assertion here is that pair of answers.
//!
//! Skipped under miri: `spinel_parse` calls into Prism, which is C.
#![cfg(not(miri))]

use spinel_vm::Heap;
use spinel_vm::compile;
use spinel_vm::interp;

/// Compile and run `source` in one booted heap, and answer both the result and
/// how many call sites the run left memoised.
fn run(source: &str) -> (String, usize) {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(
        parsed.errors.is_empty(),
        "{source:?} did not parse: {:?}",
        parsed.errors
    );
    let iseq = compile::program(&parsed.program).expect("the source compiles");
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(iseq.locals.len());
    let rendered = {
        let mut scope = heap.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
        let value = interp::eval_in(&mut scope, &mut frame, &iseq).expect("the source runs");
        interp::inspect(&mut scope, value)
    };
    let filled = heap.call_caches().filled();
    (rendered, filled)
}

/// `Insn::Send` fills the table at all. Without this the rest of the file
/// would pass against a cache nothing ever writes to.
#[test]
fn a_send_memoises_its_call_site() {
    let (value, filled) = run("class C; def m; 7; end; end; C.new.m");
    assert_eq!(value, "7");
    assert!(filled > 0, "sends left entries behind");
}

/// The serial guard, end to end. One call site, two calls, a redefinition in
/// between — the receiver class never changes, so a cache keyed on the class
/// alone would answer `1` twice.
#[test]
fn a_redefinition_between_two_calls_through_one_site_is_seen() {
    let source = "
      class C
        def m; 1; end
      end
      def through(c); c.m; end
      c = C.new
      first = through(c)
      class C
        def m; 2; end
      end
      second = through(c)
      first * 10 + second
    ";
    let (value, _) = run(source);
    assert_eq!(value, "12", "the second call saw the new body");
}

/// The same, with the definition on an ancestor rather than on the receiver's
/// own class. `C`'s serial moves because the change is in `C`'s chain, which is
/// the invalidation #9 bought and this cache is guarding on.
#[test]
fn a_definition_on_a_superclass_between_two_calls_is_seen() {
    let source = "
      class Base
        def m; 1; end
      end
      class C < Base
      end
      def through(c); c.m; end
      c = C.new
      first = through(c)
      class Base
        def m; 2; end
      end
      second = through(c)
      first * 10 + second
    ";
    let (value, _) = run(source);
    assert_eq!(value, "12");
}

/// The class guard, end to end: one site, two receiver classes. A monomorphic
/// entry filled by the first must not answer the second.
#[test]
fn one_site_with_two_receiver_classes_answers_both() {
    let source = "
      class A
        def m; 1; end
      end
      class B
        def m; 2; end
      end
      def through(x); x.m; end
      a1 = through(A.new)
      b1 = through(B.new)
      a2 = through(A.new)
      a1 * 100 + b1 * 10 + a2
    ";
    let (value, _) = run(source);
    assert_eq!(value, "121", "A, B, then A again — each got its own");
}

/// A singleton method defined after the site was memoised. This is the case
/// that needs no serial: `def obj.m` *replaces* the object's class with its
/// singleton, so the class half of the guard catches it.
#[test]
fn a_singleton_definition_after_a_call_is_seen() {
    let source = "
      class C
        def m; 1; end
      end
      def through(c); c.m; end
      c = C.new
      first = through(c)
      def c.m; 2; end
      second = through(c)
      other = through(C.new)
      first * 100 + second * 10 + other
    ";
    let (value, _) = run(source);
    assert_eq!(value, "121", "the singleton got its own, and C kept C's");
}

/// A loop through one site: the shape the cache exists for, and the shape that
/// would break if a base were handed out per frame push rather than per `Iseq`.
#[test]
fn a_repeated_call_reuses_one_entry() {
    let source = "
      class C
        def m; 1; end
      end
      def through(c); c.m; end
      c = C.new
      total = 0
      i = 0
      while i < 50
        total = total + through(c)
        i = i + 1
      end
      total
    ";
    let (value, filled) = run(source);
    assert_eq!(value, "50");
    let (_, once) = run("class C; def m; 1; end; end; def through(c); c.m; end; through(C.new)");
    assert!(
        filled < once + 20,
        "50 calls through one site did not grow the table per call ({filled} vs {once})"
    );
}
