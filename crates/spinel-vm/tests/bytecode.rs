//! What the compiler emits, and the property that lets it be cached.
//!
//! `eval.rs` checks the answers. This checks the *shape*, because two of the
//! issue's requirements are about the format rather than the result: bytecode
//! is position-independent, and symbols are stored by name and relinked on load.
//! Neither is visible in an answer, and both are expensive to retrofit — a
//! bytecode cache and `core.image` in phase 3 are the callers that depend on
//! them, and they arrive long after the mistake would have been made.

use spinel_vm::bytecode::{Insn, Iseq};
use spinel_vm::{Heap, compile, interp};

fn iseq(source: &str) -> Iseq {
    let parsed = spinel_parse::parse(source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source:?}: {:?}", parsed.errors);
    compile::program(&parsed.program).expect("should compile")
}

fn run(iseq: &Iseq) -> String {
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(iseq.locals.len());
    let mut scope = heap.scope();
    scope.bootstrap();
    let value = interp::eval_in(&mut scope, &mut frame, iseq).expect("should run");
    interp::inspect(&mut scope, value)
}

/// The jumps in an `Iseq`, in order, as displacements.
fn jumps(iseq: &Iseq) -> Vec<i32> {
    iseq.insns
        .iter()
        .filter_map(|insn| match *insn {
            Insn::Jump(d)
            | Insn::JumpIf(d)
            | Insn::JumpUnless(d)
            | Insn::JumpIfKeep(d)
            | Insn::JumpUnlessKeep(d) => Some(d),
            _ => None,
        })
        .collect()
}

#[test]
fn the_same_code_compiles_to_the_same_jumps_wherever_it_sits() {
    // Position independence, stated as the property that actually matters: the
    // displacements do not depend on where in the file the construct is. An
    // absolute target would make these two differ, and would make a cached
    // `Iseq` need relocating before it could be used.
    let alone = iseq("if true then 1 else 2 end");
    let preceded = iseq("x = 1; y = 2; z = 3; if true then 1 else 2 end");

    assert!(!jumps(&alone).is_empty(), "an if should emit jumps");
    assert_eq!(jumps(&alone), jumps(&preceded));
}

#[test]
fn a_loops_back_edge_is_negative_and_its_exit_is_positive() {
    let iseq = iseq("i = 0; while i < 3; i += 1; end");
    let jumps = jumps(&iseq);
    assert!(
        jumps.iter().any(|&d| d < 0),
        "a while must jump backwards: {jumps:?}"
    );
    assert!(
        jumps.iter().any(|&d| d > 0),
        "a while must jump past its body: {jumps:?}"
    );
}

#[test]
fn symbols_are_stored_by_name_and_deduplicated() {
    let iseq = iseq(":a; :b; :a");
    assert_eq!(
        iseq.symbols.iter().map(|s| &**s).collect::<Vec<_>>(),
        vec!["a", "b"],
        "the pool holds names, once each"
    );
    // And nothing in the instruction stream is a `SymbolId`: the indices are
    // into the pool above, which is what makes the pool the only thing that
    // needs relinking.
    let pushes: Vec<u32> = iseq
        .insns
        .iter()
        .filter_map(|insn| match *insn {
            Insn::PushSym(index) => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(pushes, vec![0, 1, 0]);
}

#[test]
fn an_iseq_relinks_against_a_symbol_table_it_has_never_seen() {
    // The "relinked on load" half. An `Iseq` compiled before a pile of
    // unrelated symbols existed must mean the same thing after — otherwise a
    // bytecode file would only be readable by a process that had interned
    // things in the same order, which is the bug position independence exists
    // to prevent.
    let iseq = iseq("[:relink_alpha, :relink_beta]");
    let before = run(&iseq);

    let first = iseq.link();
    // Move the table on underneath it.
    for extra in 0..64 {
        let _ = spinel_vm::shared::symbols::intern(&format!("unrelated_{extra}"));
    }
    let after = iseq.link();

    assert_eq!(first, after, "the same names must relink to the same ids");
    assert_eq!(before, run(&iseq));
    assert_eq!(before, "[:relink_alpha, :relink_beta]");
}

#[test]
fn max_stack_is_what_the_program_actually_needs() {
    // The interpreter sizes its stack from this. Too small is a reallocation;
    // too large is per-frame waste once #11 makes frames plentiful.
    for (source, expected) in [
        ("1", 1),
        ("[1, 2, 3]", 3),
        ("1 + 2", 2),
        ("a = 1", 2),
        ("if true then 1 else 2 end", 1),
    ] {
        assert_eq!(iseq(source).max_stack, expected, "for {source:?}");
    }
}

#[test]
fn every_iseq_ends_by_leaving() {
    // The interpreter loop has no bounds check on the program counter: it runs
    // until `Leave`. That is safe exactly because the compiler always emits one.
    for source in ["", "1", "if true then 1 end", "while false; end", "a = 1"] {
        assert_eq!(
            iseq(source).insns.last(),
            Some(&Insn::Leave),
            "for {source:?}"
        );
    }
}

#[test]
fn a_local_keeps_its_slot_across_separately_compiled_expressions() {
    // What the spec harness depends on: it compiles an example's statements one
    // at a time against a shared slot map, and `a` must be the same slot in the
    // statement that writes it and the one that reads it.
    let parsed = spinel_parse::parse(b"a = 1\nb = 2\na");
    let mut locals: Vec<spinel_ast::Name> = Vec::new();
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(0);
    let mut scope = heap.scope();
    scope.bootstrap();

    let mut last = String::new();
    for statement in &parsed.program.body {
        let iseq = compile::expression("<test>", &locals, statement).expect("should compile");
        locals.clone_from(&iseq.locals);
        let value = interp::eval_in(&mut scope, &mut frame, &iseq).expect("should run");
        last = interp::inspect(&mut scope, value);
    }
    assert_eq!(last, "1", "`a` should still hold 1 in the third statement");
}
