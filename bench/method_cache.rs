//! What the method cache is worth, and what invalidation costs.
//!
//! Three numbers, because [#9] asks for one and the other two are what make it
//! honest:
//!
//! 1. **Dispatch**, cached against uncached. `Classes::lookup` in front of
//!    `Classes::lookup_uncached`, on the chain `core/*.rb` actually builds.
//!    This is the issue's "measurable dispatch improvement over uncached
//!    lookup".
//! 2. **Invalidation**, the descendant walk a definition pays for. Per-class
//!    serials buy precision on the read side by doing work on the write side,
//!    and a benchmark that only reported the read would be selling half a
//!    trade.
//! 3. **Boot**, which is the write side at its worst: `spinel_core::boot`
//!    defines several hundred methods, many of them on `Object` and `Kernel`
//!    where every class in the heap is downstream.
//!
//! Not criterion: `std::time::Instant` and a fixed iteration count are enough
//! to separate "an order of magnitude" from "noise", which is the only question
//! being asked. Numbers go in the PR, not in the repo — see `CLAUDE.md`.
//!
//! [#9]: https://github.com/ar4mirez/spinel/issues/9

use std::time::{Duration, Instant};

use spinel_vm::bytecode::{BlockRef, CallSite, Iseq};
use spinel_vm::callcache::CallCaches;
use spinel_vm::shared::symbols;
use spinel_vm::value::Value;
use spinel_vm::{Builtin, ClassId, Heap, SymbolId, compile, interp};

/// Min of five runs of `iterations`, rather than a mean.
///
/// The quantity being estimated is how long the work takes when nothing else
/// interferes; a mean folds in every scheduler interruption, and at ten
/// nanoseconds a call that noise is larger than the effect.
fn time(iterations: u32, mut body: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..iterations {
            body();
        }
        best = best.min(start.elapsed() / iterations);
    }
    best
}

const N: u32 = 1_000_000;

/// An `Iseq` with exactly one call site and nothing else. `CallCaches` counts
/// sites and never reads one, so this is the whole of what it needs.
fn one_call_site() -> std::sync::Arc<Iseq> {
    std::sync::Arc::new(Iseq {
        call_sites: vec![CallSite {
            name: 0,
            argc: 0,
            splats: Vec::new(),
            keywords: Vec::new(),
            block: BlockRef::None,
            implicit_self: false,
        }],
        ..Iseq::default()
    })
}

/// Parse and compile once, outside the timed loop: this measures running Ruby,
/// not compiling it.
fn compiled(source: &str) -> Iseq {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(parsed.errors.is_empty(), "the benchmark source parses");
    compile::program(&parsed.program).expect("the benchmark source compiles")
}

/// One dispatch measurement: the cache in front of the walk, and the walk.
fn dispatch(heap: &mut Heap, what: &str, id: ClassId, name: SymbolId) {
    let mut scope = heap.scope();
    let classes = scope.classes_mut();
    let hops = classes.ancestors(id).len();
    classes.lookup(id, name);

    let cached = time(N, || {
        std::hint::black_box(classes.lookup(std::hint::black_box(id), name));
    });
    let uncached = time(N, || {
        std::hint::black_box(classes.lookup_uncached(std::hint::black_box(id), name));
    });
    println!(
        "  {what:<28} {hops:>3} deep   {cached:>7.1?}   {uncached:>8.1?}   {:>5.1}x",
        uncached.as_secs_f64() / cached.as_secs_f64()
    );
}

fn main() {
    let mut heap = Heap::new();
    {
        let mut scope = heap.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
    }

    // A hierarchy deep enough to show the curve. Real Ruby is not usually 24
    // classes deep, but `method_missing` and `respond_to?` walk to the end of
    // whatever chain they are on, and that is the same measurement.
    let (shallow, deep, mixed) = {
        let mut scope = heap.scope();
        let mut parent = Builtin::Object.id();
        for level in 0..24 {
            parent = scope.define_class(Some(&format!("Deep{level}")), Some(parent));
        }
        let deep = parent;
        let shallow = scope.define_class(Some("Shallow"), Some(Builtin::Object.id()));
        // Modules widen a chain without deepening the superclass list, which is
        // the shape `Comparable` and `Enumerable` give real classes.
        let mixed = scope.define_class(Some("Mixed"), Some(Builtin::Object.id()));
        for level in 0..8 {
            let m = scope.define_module(Some(&format!("Mix{level}")));
            scope.classes_mut().include(mixed, m).unwrap();
        }
        (shallow, deep, mixed)
    };

    let own = symbols::intern("__bench_own__");
    let far = symbols::intern("__bench_far__");
    let absent = symbols::intern("__bench_absent__");
    {
        let mut scope = heap.scope();
        let body = Value::fixnum(1).unwrap();
        let classes = scope.classes_mut();
        classes.define_method(shallow, own, body);
        classes.define_method(deep, own, body);
        classes.define_method(mixed, own, body);
        // Answered by `Object`, so every chain has to be crossed to reach it.
        classes.define_method(Builtin::Object.id(), far, body);
    }

    println!("dispatch                                   cached   uncached   speedup");
    dispatch(&mut heap, "hit on the class itself", shallow, own);
    dispatch(&mut heap, "hit on the class itself", deep, own);
    dispatch(&mut heap, "hit on Object, 1 away", shallow, far);
    dispatch(&mut heap, "hit on Object, 24 away", deep, far);
    dispatch(&mut heap, "hit on Object, 8 modules", mixed, far);
    dispatch(&mut heap, "miss (method_missing)", deep, absent);

    // What per-class serials actually buy: a definition somewhere unrelated no
    // longer evicts this class's cached answer. One serial for the whole table
    // made every one of these dispatches a full chain walk again.
    {
        let mut scope = heap.scope();
        let classes = scope.classes_mut();
        let churn = symbols::intern("__bench_churn__");
        let body = Value::fixnum(1).unwrap();
        classes.lookup(deep, far);
        const K: u32 = 200_000;
        let steady = time(K, || {
            std::hint::black_box(classes.lookup(std::hint::black_box(deep), far));
        });
        // `Shallow` is not in `Deep23`'s chain, so nothing here can change what
        // `Deep23` resolves — which is exactly the claim being measured.
        let churned = time(K, || {
            classes.define_method(shallow, churn, body);
            std::hint::black_box(classes.lookup(std::hint::black_box(deep), far));
        });
        let definition = time(K, || {
            classes.define_method(shallow, churn, body);
        });
        println!("\ndispatch while an unrelated class is being defined into");
        println!("  undisturbed lookup           {steady:>8.1?}");
        println!("  definition alone             {definition:>8.1?}");
        // What one serial per table forced: the definition evicted `Deep23`'s
        // entry too, so the next lookup walked the chain again. The walk is
        // `lookup_uncached`; the re-insert it also paid is not counted here,
        // which makes this the *generous* reading of the old design.
        let coarse = time(K, || {
            classes.define_method(shallow, churn, body);
            std::hint::black_box(classes.lookup_uncached(std::hint::black_box(deep), far));
        });
        println!(
            "  definition + lookup          {churned:>8.1?}   (lookup share {:.1?})",
            churned.saturating_sub(definition)
        );
        println!(
            "  ... one serial per table     {coarse:>8.1?}   (lookup share {:.1?}, {:.1}x worse)",
            coarse.saturating_sub(definition),
            coarse.as_secs_f64() / churned.as_secs_f64()
        );
    }

    // The write side. Per-class serials buy the read side by paying here.
    {
        let mut scope = heap.scope();
        let classes = scope.classes_mut();
        let victim = symbols::intern("__bench_invalidation__");
        let body = Value::fixnum(1).unwrap();
        let count = classes.len();
        const M: u32 = 100_000;
        let root = time(M, || {
            classes.define_method(Builtin::Object.id(), victim, body)
        });
        let leaf = time(M, || classes.define_method(deep, victim, body));
        println!("\ninvalidation, {count} classes in the heap");
        println!("  define on Object   {root:>8.1?}   (every class downstream)");
        println!("  define on a leaf   {leaf:>8.1?}   (nothing downstream)");
    }

    // The inline cache, in front of the method cache — #169. The number to beat
    // is the *cached* lookup above, not the walk: the per-class cache already
    // turns every chain depth into one hash probe, so an inline cache is only
    // worth its guard if two integer compares beat that probe.

    {
        let mut scope = heap.scope();
        let serial = scope.classes().serial(deep);
        let method = scope
            .classes_mut()
            .lookup(deep, far)
            .expect("`far` is on Object");

        let mut caches = CallCaches::new();
        let slot = caches.base(&one_call_site());
        caches.fill(slot, deep, serial, method);

        let probe = {
            let classes = scope.classes_mut();
            time(N, || {
                std::hint::black_box(classes.lookup(std::hint::black_box(deep), far));
            })
        };
        // The serial read is included because `dispatch` pays it: the guard is
        // "this class, and this class has not changed", and the second half has
        // to be fetched before it can be compared.
        let guard = {
            let classes = scope.classes();
            time(N, || {
                let now = classes.serial(std::hint::black_box(deep));
                std::hint::black_box(caches.get(slot, std::hint::black_box(deep), now));
            })
        };
        // A guard that fails costs the compare *and* the probe, which is what a
        // polymorphic site pays on every call.
        let missed = {
            let classes = scope.classes();
            time(N, || {
                let now = classes.serial(std::hint::black_box(shallow));
                std::hint::black_box(caches.get(slot, std::hint::black_box(shallow), now));
            })
        };

        println!("\ninline cache vs the method cache it fronts");
        println!("  cached lookup (hash probe)   {probe:>8.1?}");
        println!(
            "  inline cache hit             {guard:>8.1?}   ({:.1}x)",
            probe.as_secs_f64() / guard.as_secs_f64().max(f64::MIN_POSITIVE)
        );
        println!("  inline cache miss (guard)    {missed:>8.1?}   (then pays the probe too)");
    }

    // End to end: the same send, through the interpreter, in a Ruby loop. The
    // micro numbers above are the two paths; this is what the difference is
    // worth once frame push, argument binding and the loop itself are in the
    // way. Compare against the same figure on the parent commit.

    {
        const E: u32 = 200;
        let iseq = compiled(
            "
            class C
              def m; 1; end
            end
            def through(c); c.m; end
            c = C.new
            total = 0
            i = 0
            while i < 2000
              total = total + through(c)
              i = i + 1
            end
            total
            ",
        );
        let elapsed = time(E, || {
            let mut heap = Heap::new();
            let mut frame = interp::Frame::new(iseq.locals.len());
            let mut scope = heap.scope();
            scope.bootstrap();
            spinel_core::boot(&mut scope);
            std::hint::black_box(
                interp::eval_in(&mut scope, &mut frame, &iseq).expect("the loop runs"),
            );
        });
        // Boot dominates a run this short, so the loop is reported net of it.
        println!("\n2000 sends through one call site, end to end");
        println!("  run (boot included)  {elapsed:>8.1?}");
    }

    // Boot: several hundred definitions, most of them on `Object` or `Kernel`.
    // The spec harness pays this once per example, 25,624 times.
    const B: u32 = 200;
    let boot = time(B, || {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        spinel_core::boot(&mut scope);
        std::hint::black_box(scope.classes().len());
    });
    println!("\nboot (bootstrap + core/*.rb)");
    println!("  per heap           {boot:>8.1?}");
}
