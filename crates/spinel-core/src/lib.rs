//! The Ruby core library, and the loader that puts it into a heap.
//!
//! Core classes are written in Ruby, in `core/*.rb` at the repository root.
//! Rust code lives in `spinel-vm` and only where Ruby cannot express the
//! operation: raw bytes, allocation, syscalls, dispatch. This crate is the seam
//! between the two — it compiles the Ruby and evaluates it into a heap.
//!
//! # Why here and not in `spinel-vm`
//!
//! Compiling Ruby needs a parser, and `spinel-vm` deliberately does not depend
//! on `spinel-parse`: Prism lives in exactly one crate, and a runtime dependency
//! would put it in every VM build. So the VM bootstraps *classes* and this crate
//! bootstraps their *methods*. Every embedder calls both, in that order.
//!
//! # Compiled once, evaluated per heap
//!
//! [`boot`] is called once per [`Heap`], and the spec harness makes one heap per
//! example — 25,000 of them in a corpus run. Evaluating per heap is unavoidable,
//! because a method table belongs to a heap. Parsing per heap is not, so the
//! compile happens on the first call and the resulting [`Iseq`]s are cached.
//!
//! The cache lives in `spinel_vm::shared::core_image`, which is the directory
//! `CLAUDE.md` names as the exception to "no process-global mutable VM state":
//! immutable, append-only tables. An `Iseq` is immutable bytecode and holds no
//! `Value`, so no heap can reach another's objects through it — the same
//! category as `spinel_vm::shared::symbols`.
//!
//! `// ponytail:` this is `docs/engine.md`'s `core.image` minus the
//! serialisation. The ceiling is one parse and compile per *process*. The
//! upgrade path is a `build.rs` that serialises the `Iseq`s into the binary, and
//! it is worth writing when that parse shows up in a benchmark.

use std::sync::Arc;

use spinel_vm::{HandleScope, Iseq, compile, interp, shared};

/// The core library sources, in load order.
///
/// Order is dependency order, not alphabetical: `Comparable` is defined before
/// `Integer` includes it, and `Object` before anything reopens it.
const SOURCES: &[(&str, &str)] = &[
    (
        "core/basic_object.rb",
        include_str!("../../../core/basic_object.rb"),
    ),
    ("core/kernel.rb", include_str!("../../../core/kernel.rb")),
    ("core/object.rb", include_str!("../../../core/object.rb")),
    (
        "core/comparable.rb",
        include_str!("../../../core/comparable.rb"),
    ),
    ("core/module.rb", include_str!("../../../core/module.rb")),
    ("core/class.rb", include_str!("../../../core/class.rb")),
    (
        "core/nil_class.rb",
        include_str!("../../../core/nil_class.rb"),
    ),
    (
        "core/true_class.rb",
        include_str!("../../../core/true_class.rb"),
    ),
    (
        "core/false_class.rb",
        include_str!("../../../core/false_class.rb"),
    ),
    ("core/numeric.rb", include_str!("../../../core/numeric.rb")),
    ("core/float.rb", include_str!("../../../core/float.rb")),
    ("core/integer.rb", include_str!("../../../core/integer.rb")),
    ("core/symbol.rb", include_str!("../../../core/symbol.rb")),
    ("core/string.rb", include_str!("../../../core/string.rb")),
    ("core/array.rb", include_str!("../../../core/array.rb")),
    ("core/hash.rb", include_str!("../../../core/hash.rb")),
    ("core/regexp.rb", include_str!("../../../core/regexp.rb")),
    (
        "core/match_data.rb",
        include_str!("../../../core/match_data.rb"),
    ),
    (
        "core/exception.rb",
        include_str!("../../../core/exception.rb"),
    ),
];

/// The compiled core library, one [`Iseq`] per file.
///
/// The cache itself is `spinel_vm::shared::core_image`, because that directory
/// is where `CLAUDE.md` puts immutable process-wide tables and a compiled
/// `Iseq` is one. Only the compiling is here, because only this crate has a
/// parser.
fn image() -> &'static [Arc<Iseq>] {
    shared::core_image::get_or_compile(|| {
        SOURCES
            .iter()
            .map(|(name, source)| {
                let parsed = spinel_parse::parse(source.as_bytes());
                // A syntax error in `core/*.rb` is a bug in this repository, not
                // in a user's program. Panicking names the file and the message;
                // a `Result` would only move the same panic one frame out, into
                // a caller that has nothing better to do with it.
                assert!(
                    parsed.is_ok(),
                    "{name} does not parse: {}",
                    parsed
                        .errors
                        .iter()
                        .map(|e| e.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                let iseq = compile::program(&parsed.program)
                    .unwrap_or_else(|e| panic!("{name} does not compile: {e:?}"));
                Arc::new(iseq)
            })
            .collect()
    })
}

/// Define the core library's methods in this heap.
///
/// Call once, after [`HandleScope::bootstrap`] has created the classes this
/// fills in. Panics if `core/*.rb` raises, for the same reason the compile does.
pub fn boot(scope: &mut HandleScope<'_>) {
    for (index, iseq) in image().iter().enumerate() {
        let mut frame = interp::Frame::new(0);
        if let Err(err) = interp::eval_in(scope, &mut frame, iseq) {
            let (name, _) = SOURCES[index];
            panic!("{name} raised while loading the core library: {err:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spinel_vm::Heap;

    /// The check that the whole crate exists for: the sources parse, compile,
    /// and run into a heap. A failure here names the file.
    #[test]
    fn the_core_library_loads() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();
        boot(&mut scope);
    }

    /// Compiling is once per process, not once per heap — which is what keeps a
    /// 25,000-example corpus run from paying for the parser 25,000 times.
    #[test]
    fn the_image_is_compiled_once() {
        let first = image().as_ptr();
        let second = image().as_ptr();
        assert_eq!(first, second);
    }
}
