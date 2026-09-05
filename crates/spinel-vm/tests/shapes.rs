//! Hidden-class shapes: the layout rule the rest of the VM is built on.
//!
//! `shape.rs`'s own unit tests hold the tree honest in isolation. What is
//! checked here is the tree *as objects reach it* — that two objects built the
//! same way actually meet at one node, that two built in the opposite order
//! actually do not, and that the collector traces what a shape points at.
//!
//! The last of those is the one worth spelling out. #151's definition of done
//! asked for `Heap::mark` to trace ivar slots and predicted it would be a test
//! rather than a change, because instance variables live behind a slot of a
//! `Payload::Slots` object and the collector already descends into those. The
//! prediction held; this file is what makes it a fact rather than a claim.

//! Skipped under miri: `spinel_parse` calls into Prism, which is C.
#![cfg(not(miri))]

use spinel_vm::heap::HandleScope;
use spinel_vm::shape::ShapeId;
use spinel_vm::{Heap, Value, compile, interp};

/// Run `source` on a booted heap and hand the answer, plus the live scope, to
/// `check` — so a test can ask the heap about the object it just built.
fn with_result<T>(source: &str, check: impl FnOnce(&mut HandleScope<'_>, Value) -> T) -> T {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(
        parsed.errors.is_empty(),
        "{source:?} did not parse: {:?}",
        parsed.errors
    );
    let iseq = compile::program(&parsed.program)
        .unwrap_or_else(|e| panic!("{source:?} did not compile: {e}"));
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(iseq.locals.len());
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);
    let value = interp::eval_in(&mut scope, &mut frame, &iseq)
        .unwrap_or_else(|e| panic!("{source:?} did not run: {e}"));
    check(&mut scope, value)
}

/// The shapes of the two objects an `[a, b]` answer holds.
fn shapes_of_pair(source: &str) -> (ShapeId, ShapeId) {
    with_result(source, |scope, value| {
        let array = scope.root(value);
        // An `Array` is `[storage, length]`; the two elements are in storage.
        let storage = scope.slot(array, 0);
        let storage = scope.root(storage);
        let (first, second) = (scope.slot(storage, 0), scope.slot(storage, 1));
        let (first, second) = (scope.root(first), scope.root(second));
        (scope.shape(first), scope.shape(second))
    })
}

#[test]
fn two_objects_given_the_same_names_in_the_same_order_share_a_shape() {
    let (a, b) = shapes_of_pair(
        r#"
        class C
          def initialize(x, y)
            @a = x
            @b = y
          end
        end
        [C.new(1, 2), C.new(3, 4)]
        "#,
    );
    assert_eq!(a, b, "same names, same order, different shapes");
    assert_ne!(a, ShapeId::ROOT);
}

#[test]
fn two_objects_given_the_same_names_in_a_different_order_do_not() {
    let (a, b) = shapes_of_pair(
        r#"
        class C
          def initialize(which)
            if which
              @a = 1
              @b = 2
            else
              @b = 2
              @a = 1
            end
          end
        end
        [C.new(true), C.new(false)]
        "#,
    );
    assert_ne!(
        a, b,
        "`@a` then `@b` and `@b` then `@a` cannot be one layout: \
         the index each name lands at differs"
    );
}

#[test]
fn a_name_assigned_twice_does_not_transition_twice() {
    let (a, b) = shapes_of_pair(
        r#"
        class C
          def initialize(twice)
            @a = 1
            @a = 2 if twice
          end
        end
        [C.new(false), C.new(true)]
        "#,
    );
    assert_eq!(a, b, "re-assigning `@a` grew the object");
}

#[test]
fn an_object_that_holds_none_is_not_the_same_as_one_that_cannot() {
    with_result("[Object.new, [1, 2]]", |scope, value| {
        let array = scope.root(value);
        let storage = scope.slot(array, 0);
        let storage = scope.root(storage);
        let (plain, native) = (scope.slot(storage, 0), scope.slot(storage, 1));
        let (plain, native) = (scope.root(plain), scope.root(native));
        assert_eq!(
            scope.shape(plain),
            ShapeId::ROOT,
            "a plain object holds no ivars and can hold one"
        );
        assert_eq!(
            scope.shape(native),
            ShapeId::NONE,
            "an Array's slot 0 is its elements, so it must not be read as ivar storage"
        );
    });
}

#[test]
fn a_class_object_carries_its_table_id_as_an_ordinary_ivar() {
    with_result("class Marker; end; Marker", |scope, value| {
        let class = scope.root(value);
        assert_ne!(
            scope.shape(class),
            ShapeId::NONE,
            "a class object holds instance variables like any other"
        );
        assert_ne!(
            scope.shape(class),
            ShapeId::ROOT,
            "and holds at least one: its own table id"
        );
        // The round-trip the dispatch path takes: header class pointer to table
        // entry, through the hidden ivar rather than a fixed slot.
        let id = scope.class_id_of(class).expect("Marker is a class object");
        assert_eq!(scope.classes().name(id), Some("Marker"));
    });
}

#[test]
fn instance_variables_survive_a_collection() {
    // Enough garbage to force several collections between the writes, so the
    // storage object a shape points at is reached only through the object's own
    // slot — which is the path `Heap::mark` has to descend.
    let source = r#"
      class C
        def initialize
          @a = "first"
          i = 0
          while i < 400
            junk = [i, i + 1, i + 2]
            i = i + 1
          end
          @b = "second"
          i = 0
          while i < 400
            junk = [i, i + 1, i + 2]
            i = i + 1
          end
          @c = "third"
        end
        def all = [@a, @b, @c]
      end
      C.new.all
    "#;
    with_result(source, |scope, value| {
        // A collection *after* the object is built too, so nothing survives
        // only because it happened to be young.
        scope.collect();
        assert_eq!(
            interp::inspect(scope, value),
            r#"["first", "second", "third"]"#
        );
    });
}

#[test]
fn growing_past_the_first_storage_object_keeps_what_was_there() {
    // The first storage holds two. The fifth write has replaced it twice, and
    // every earlier value has to have been copied across both times.
    let source = r#"
      class C
        def initialize
          @a = 1
          @b = 2
          @c = 3
          @d = 4
          @e = 5
        end
        def all = [@a, @b, @c, @d, @e]
      end
      C.new.all
    "#;
    with_result(source, |scope, value| {
        assert_eq!(interp::inspect(scope, value), "[1, 2, 3, 4, 5]");
    });
}

#[test]
fn a_dup_does_not_share_its_originals_storage() {
    let source = r#"
      class C
        def initialize = @a = 1
        def a = @a
        def a=(v)
          @a = v
        end
      end
      first = C.new
      second = first.dup
      second.a = 99
      [first.a, second.a]
    "#;
    with_result(source, |scope, value| {
        assert_eq!(interp::inspect(scope, value), "[1, 99]");
    });
}
