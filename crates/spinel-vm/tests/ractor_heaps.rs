//! Two heaps of one program on two OS threads, sharing its classes (#117):
//! what a Ractor will be, ahead of `Ractor` itself.
//!
//! The main heap boots the core library and defines methods. A second heap
//! joins the program (`Heap::new_ractor`) and calls those methods from its own
//! OS thread. Meanwhile the main heap keeps defining methods on the class the
//! second heap's calls are dispatched through, so every definition is a write
//! to the shared table while the other heap reads it, which is what the class
//! lock (`src/vm.rs`) is for. The second heap has an object of its own for
//! each class, and the core library's hidden instance variables on it are its
//! own.
//!
//! Skipped under miri: `spinel_parse` calls into Prism, which is C. The lock's
//! own coverage under Miri is `vm::tests`.
#![cfg(not(miri))]

use std::sync::Arc;

use spinel_vm::class::Builtin;
use spinel_vm::{Heap, Iseq, compile, interp};

fn compile(source: &str) -> Arc<Iseq> {
    let parsed = spinel_parse::parse_file("ractor_heaps.rb", source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source} did not parse");
    let iseq = compile::program_with(&parsed.program, "<main>", None)
        .unwrap_or_else(|e| panic!("{source} did not compile: {e}"));
    Arc::new(iseq)
}

fn eval(heap: &mut Heap, iseq: &Iseq) -> String {
    let mut scope = heap.scope();
    let mut frame = interp::Frame::new(iseq.locals.len());
    let value = interp::eval_in(&mut scope, &mut frame, iseq)
        .unwrap_or_else(|e| panic!("the program raised: {e:?}"));
    interp::inspect(&mut scope, value)
}

/// A heap moved to the OS thread that runs it. A heap is not `Send` in
/// general, since its cells are raw memory; this one is used by one thread at
/// a time, handed over by `spawn` and `join`.
struct Moved(Heap);

// SAFETY: see above. The main heap never touches it while it is away.
unsafe impl Send for Moved {}

#[test]
fn a_joined_heap_calls_what_the_main_heap_defines_while_it_defines_more() {
    let mut main = Heap::new();
    {
        let mut scope = main.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
    }
    let defined = compile(
        "def answer = 6 * 7
         class Integer
           def double = self * 2
         end
         1.class.instance_variable_set(:@__probe__, :main)
         answer.double",
    );
    assert_eq!(eval(&mut main, &defined), "84");

    let child = Moved(main.new_ractor());
    let calls = compile(
        "x = 0
         i = 0
         while i < 100_000
           x += answer.double
           i += 1
         end
         x",
    );
    let worker = std::thread::spawn(move || {
        let mut child = child;
        let answer = eval(&mut child.0, &calls);
        child.0.release_classes();
        (child, answer)
    });

    // Definitions on `Integer` while the other heap dispatches through it:
    // each bumps its serial, so the other heap's inline cache misses and its
    // method cache refills, which is a write of its own.
    for n in 0..200 {
        let iseq = compile(&format!("class Integer; def m{n} = {n}; end"));
        eval(&mut main, &iseq);
    }
    main.release_classes();
    let (mut child, answer) = worker.join().expect("the joined heap's thread finishes");
    assert_eq!(answer, (100_000 * 84).to_string());

    // Every method the main heap defined is the joined heap's too. One OS
    // thread runs one heap at a time, so the main heap stays released while
    // this thread runs the other.
    child.0.reacquire_classes();
    let later =
        compile("[1.m199, answer.double, self.class, 1.class.instance_variable_get(:@__probe__)]");
    assert_eq!(
        eval(&mut child.0, &later),
        "[199, 84, Object, nil]",
        "the methods are shared, and `@__probe__` on `Integer` is the main heap's alone",
    );
    let object = Builtin::Object.id();
    assert_ne!(
        child.0.class_object(object),
        main.class_object(object),
        "each heap has its own `Object`",
    );
    child.0.release_classes();
    main.reacquire_classes();
    let probe = compile("1.class.instance_variable_get(:@__probe__)");
    assert_eq!(eval(&mut main, &probe), ":main");
}

/// A class a joined heap defines, held by the shared tables in a constant, a
/// class variable and a singleton's object, is each heap's own object for
/// that class when either reads it back.
#[test]
fn a_class_a_joined_heap_defines_is_each_heaps_own() {
    let mut main = Heap::new();
    {
        let mut scope = main.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
    }
    let child = Moved(main.new_ractor());
    main.release_classes();

    let define = compile(
        "class Foo
           @@held = Foo
           def self.held = @@held
         end
         Bar = Foo
         Baz = Class.new
         [Foo.new.class == Foo, Bar.equal?(Foo), Foo.held.equal?(Foo), Baz.new.class == Baz]",
    );
    let worker = std::thread::spawn(move || {
        let mut child = child;
        let answer = eval(&mut child.0, &define);
        child.0.release_classes();
        (child, answer)
    });
    let (mut child, answer) = worker.join().expect("the joined heap's thread finishes");
    assert_eq!(answer, "[true, true, true, true]", "in the joined heap");

    main.reacquire_classes();
    let read = compile(
        "class Foo
           def self.singleton_method_added(name) = ($added = name)
         end
         def Foo.later = 1
         [Foo.new.class == Foo, Bar.equal?(Foo), Foo.held.equal?(Foo), Baz.new.class == Baz, $added]",
    );
    assert_eq!(
        eval(&mut main, &read),
        "[true, true, true, true, :later]",
        "in the main heap, `Foo`'s metaclass, made by the joined heap, is attached to its own `Foo`",
    );
    main.release_classes();

    child.0.reacquire_classes();
    let again = compile("[Foo.held.equal?(Foo), Foo.later, $added]");
    assert_eq!(eval(&mut child.0, &again), "[true, 1, nil]");
    child.0.release_classes();
}

/// One heap defines classes and stores each in a constant while the other
/// writes the tables too, so that the other is often parked in its write
/// while a class it has no object for yet goes in. Reading the newest
/// constant right after its write answers that heap's own object for it.
#[test]
fn a_heap_reads_a_class_defined_while_it_waited_to_write() {
    let mut main = Heap::new();
    {
        let mut scope = main.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
    }
    let child = Moved(main.new_ractor());
    let define = compile(
        "i = 0
         while i < 300
           Object.const_set(:\"K#{i}\", Class.new)
           Object.const_set(:LAST, i)
           i += 1
         end
         i",
    );
    let worker = std::thread::spawn(move || {
        let mut child = child;
        let answer = eval(&mut child.0, &define);
        child.0.release_classes();
        (child, answer)
    });

    let read = compile(
        "ok = true
         n = 0
         while n < 300
           def written = 1
           if Object.const_defined?(:LAST)
             k = Object.const_get(:\"K#{LAST}\")
             ok &&= k.new.class.equal?(k)
           end
           n += 1
         end
         ok",
    );
    let answer = eval(&mut main, &read);
    main.release_classes();
    let (_child, defined) = worker.join().expect("the joined heap's thread finishes");
    assert_eq!(defined, "300");
    assert_eq!(answer, "true");
}
