//! The tables every Ractor of a program shares, written and read from several
//! OS threads at once (#119): bounded, offline, and over in seconds.
//!
//! - The symbol table (`src/shared/symbols.rs`) is append-only: threads that
//!   intern overlapping names at once each get one id per name, and the name
//!   back for it. No Prism, so Miri runs it too.
//! - The class tables (`src/vm.rs`) are written under the class lock: heaps
//!   joined to one program, each on an OS thread of its own as a Ractor's is,
//!   define methods on one class, make blocks (a `Proc` interns its body), send
//!   to several classes from one site and ask `respond_to?` (a method-cache
//!   fill), while the main heap defines methods on the class their sends go
//!   through. Every method any of them defined is there for every one of them
//!   at the end, and each answers what it computed alone.

use std::collections::HashMap;

use spinel_vm::shared::symbols;

#[test]
fn threads_interning_the_same_names_get_one_id_each() {
    const THREADS: usize = 4;
    let names: usize = if cfg!(miri) { 12 } else { 400 };
    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            std::thread::spawn(move || {
                let mut ids = HashMap::new();
                for round in 0..2 {
                    for i in 0..names {
                        // Every thread interns every shared name, starting
                        // at a place of its own, and as many its alone.
                        let n = (i + t * 7 + round) % names;
                        let shared = format!("stress_shared_{n}");
                        let own = format!("stress_own_{t}_{i}");
                        for name in [shared, own] {
                            let id = symbols::intern(&name);
                            assert_eq!(symbols::name(id).as_deref(), Some(name.as_str()));
                            if let Some(&before) = ids.get(&name) {
                                assert_eq!(id, before, "{name} interned twice");
                            }
                            ids.insert(name, id);
                        }
                    }
                }
                ids
            })
        })
        .collect();
    let maps: Vec<HashMap<String, spinel_vm::SymbolId>> = workers
        .into_iter()
        .map(|w| w.join().expect("the interning thread finishes"))
        .collect();
    for n in 0..names {
        let name = format!("stress_shared_{n}");
        let id = maps[0][&name];
        for map in &maps[1..] {
            assert_eq!(map[&name], id, "{name} has one id for every thread");
        }
        assert_eq!(symbols::intern(&name), id);
    }
}

/// Skipped under miri: `spinel_parse` calls into Prism, which is C.
#[cfg(not(miri))]
mod heaps {
    use std::sync::Arc;

    use spinel_vm::{Heap, Iseq, compile, interp};

    fn compile(source: &str) -> Arc<Iseq> {
        let parsed = spinel_parse::parse_file("shared_tables_stress.rb", source.as_bytes());
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

    /// A heap moved to the OS thread that runs it, used by one thread at a
    /// time, handed over by `spawn` and `join`.
    struct Moved(Heap);

    // SAFETY: see above. The main heap never touches a heap while it is away.
    unsafe impl Send for Moved {}

    const HEAPS: usize = 3;
    const ROUNDS: usize = 150;

    /// What heap `k` runs: its own methods on `Shared`, blocks, a site that
    /// sends to two classes, `respond_to?` of names others may be defining,
    /// and symbols made from strings; the answer is a sum it alone decides.
    fn work(k: usize) -> String {
        format!(
            "k = {k}
             total = 0
             i = 0
             while i < {ROUNDS}
               Shared.class_eval(\"def m_#{{k}}_#{{i}} = #{{i}}\")
               total += Shared.new.public_send(:\"m_#{{k}}_#{{i}}\")
               [Circle.new(i), Square.new(i)].each {{ |shape| total += shape.area }}
               other = Shared.new.respond_to?(:\"m_#{{(k + 1) % {HEAPS}}}_#{{i}}\")
               total += 1 if other
               total -= 1 if other
               total += \"stress_#{{k}}_#{{i}}\".to_sym.to_s.size
               i += 1
             end
             total"
        )
    }

    /// The same sum, worked out here.
    fn expected(k: usize) -> usize {
        (0..ROUNDS)
            .map(|i| i + 3 * i + 2 * i + format!("stress_{k}_{i}").len())
            .sum()
    }

    #[test]
    fn joined_heaps_write_and_read_the_class_tables_at_once() {
        let mut main = Heap::new();
        {
            let mut scope = main.scope();
            scope.bootstrap();
            spinel_core::boot(&mut scope);
        }
        let setup = compile(
            "class Shared; end
             class Circle
               def initialize(r) = @r = r
               def area = 3 * @r
             end
             class Square < Circle
               def area = 2 * @r
             end
             :ok",
        );
        assert_eq!(eval(&mut main, &setup), ":ok");

        let workers: Vec<_> = (0..HEAPS)
            .map(|k| {
                let child = Moved(main.new_ractor());
                let program = compile(&work(k));
                std::thread::spawn(move || {
                    let mut child = child;
                    let answer = eval(&mut child.0, &program);
                    child.0.release_classes();
                    (child, answer)
                })
            })
            .collect();

        // Definitions on the classes the others send to, while they run:
        // each moves the serials their caches are guarded by.
        for n in 0..100 {
            let iseq = compile(&format!(
                "class Circle; def extra_{n} = {n}; end; Shared.new.respond_to?(:m_0_0)"
            ));
            eval(&mut main, &iseq);
        }
        main.release_classes();

        let mut children = Vec::new();
        for (k, worker) in workers.into_iter().enumerate() {
            let (child, answer) = worker.join().expect("a joined heap's thread finishes");
            assert_eq!(answer, expected(k).to_string(), "heap {k}'s sum");
            children.push(child);
        }

        // Every heap sees every method any of them defined.
        let count = compile(
            "[Shared.instance_methods(false).size, Square.new(1).extra_99, Shared.new.m_2_149]",
        );
        let want = format!("[{}, 99, 149]", HEAPS * ROUNDS);
        for child in &mut children {
            child.0.reacquire_classes();
            assert_eq!(eval(&mut child.0, &count), want);
            child.0.release_classes();
        }
        main.reacquire_classes();
        assert_eq!(eval(&mut main, &count), want);
    }
}
