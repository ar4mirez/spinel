//! `src/exceptions.txt` — CRuby's exception hierarchy — run against the heap
//! `bootstrap` builds.
//!
//! The table is a measurement: `scripts/exceptions-oracle.rb` writes it from a
//! real Ruby's `ObjectSpace` and CI re-runs it with `--check`. This test is the
//! other half — that the bootstrap actually *builds* what the table describes,
//! with each class reachable by name and each superclass chain intact. Without
//! it, a parse bug would silently produce a heap where `rescue TypeError` never
//! matches and every exception spec fails for a reason nothing names.

use spinel_vm::class::Builtin;
use spinel_vm::{Heap, Value};

const TABLE: &str = include_str!("../src/exceptions.txt");

/// `(name, superclass)` for every line of the table, comments and blanks gone.
fn rows() -> Vec<(&'static str, &'static str)> {
    TABLE
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, superclass) = line.split_once('<')?;
            Some((name.trim(), superclass.trim()))
        })
        .collect()
}

/// The class object bound to a top-level constant, or `None`.
fn class_named(heap: &mut Heap, name: &str) -> Option<Value> {
    let symbol = spinel_vm::shared::symbols::intern(name);
    let scope = heap.scope();
    scope.classes().const_get_here(Builtin::Object.id(), symbol)
}

#[test]
fn every_measured_class_is_reachable_by_name() {
    let mut heap = Heap::new();
    heap.scope().bootstrap();
    for (name, _) in rows() {
        assert!(
            class_named(&mut heap, name).is_some(),
            "{name} is in the oracle table but not a constant after bootstrap"
        );
    }
}

#[test]
fn every_measured_superclass_is_the_one_ruby_reports() {
    let mut heap = Heap::new();
    heap.scope().bootstrap();
    for (name, expected) in rows() {
        let mut scope = heap.scope();
        let object = scope
            .classes()
            .const_get_here(
                Builtin::Object.id(),
                spinel_vm::shared::symbols::intern(name),
            )
            .unwrap_or_else(|| panic!("{name} should be a constant"));
        let handle = scope.root(object);
        let id = scope
            .class_id_of(handle)
            .unwrap_or_else(|| panic!("{name} should be a class"));
        let superclass = scope
            .classes()
            .superclass(id)
            .unwrap_or_else(|| panic!("{name} should have a superclass"));
        let actual = scope.classes().name(superclass).map(str::to_owned);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "{name}'s superclass disagrees with what CRuby reports"
        );
    }
}

#[test]
fn rescue_matching_walks_to_standard_error() {
    // What `rescue StandardError` has to be able to answer. A `ZeroDivisionError`
    // is one; a `NoMemoryError` is deliberately not, which is why `rescue` with
    // no class list does not catch it.
    let mut heap = Heap::new();
    heap.scope().bootstrap();
    let mut scope = heap.scope();
    let id_of = |scope: &mut spinel_vm::HandleScope<'_>, name: &str| {
        let object = scope
            .classes()
            .const_get_here(
                Builtin::Object.id(),
                spinel_vm::shared::symbols::intern(name),
            )
            .expect("a bootstrapped class");
        let handle = scope.root(object);
        scope.class_id_of(handle).expect("a class")
    };
    let standard = id_of(&mut scope, "StandardError");
    let zero_div = id_of(&mut scope, "ZeroDivisionError");
    let no_memory = id_of(&mut scope, "NoMemoryError");
    assert!(scope.classes().ancestors(zero_div).contains(&standard));
    assert!(!scope.classes().ancestors(no_memory).contains(&standard));
}
