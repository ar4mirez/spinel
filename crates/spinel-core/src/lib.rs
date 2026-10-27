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
//! [`boot`] is called once per [`Heap`], and a corpus run under mspec starts a
//! few hundred processes, each with its own. Evaluating per heap is unavoidable,
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

use spinel_vm::{HandleScope, Iseq, ParseFailure, ParsedSource, compile, interp, shared};

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
    (
        "core/enumerable.rb",
        include_str!("../../../core/enumerable.rb"),
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
    ("core/range.rb", include_str!("../../../core/range.rb")),
    ("core/regexp.rb", include_str!("../../../core/regexp.rb")),
    (
        "core/match_data.rb",
        include_str!("../../../core/match_data.rb"),
    ),
    (
        "core/exception.rb",
        include_str!("../../../core/exception.rb"),
    ),
    (
        "core/encoding.rb",
        include_str!("../../../core/encoding.rb"),
    ),
    ("core/binding.rb", include_str!("../../../core/binding.rb")),
    ("core/io.rb", include_str!("../../../core/io.rb")),
    ("core/warning.rb", include_str!("../../../core/warning.rb")),
    ("core/method.rb", include_str!("../../../core/method.rb")),
    (
        "core/ruby_written.rb",
        include_str!("../../../core/ruby_written.rb"),
    ),
    ("core/math.rb", include_str!("../../../core/math.rb")),
    ("core/set.rb", include_str!("../../../core/set.rb")),
    ("core/struct.rb", include_str!("../../../core/struct.rb")),
    (
        "core/rational.rb",
        include_str!("../../../core/rational.rb"),
    ),
    ("core/complex.rb", include_str!("../../../core/complex.rb")),
    ("core/data.rb", include_str!("../../../core/data.rb")),
    ("core/file.rb", include_str!("../../../core/file.rb")),
    ("core/random.rb", include_str!("../../../core/random.rb")),
    ("core/load.rb", include_str!("../../../core/load.rb")),
    ("core/process.rb", include_str!("../../../core/process.rb")),
    ("core/time.rb", include_str!("../../../core/time.rb")),
    ("core/marshal.rb", include_str!("../../../core/marshal.rb")),
    (
        "core/rbconfig.rb",
        include_str!("../../../core/rbconfig.rb"),
    ),
    ("core/pack.rb", include_str!("../../../core/pack.rb")),
    (
        "core/transcode.rb",
        include_str!("../../../core/transcode.rb"),
    ),
    ("core/signal.rb", include_str!("../../../core/signal.rb")),
    ("core/gc.rb", include_str!("../../../core/gc.rb")),
    ("core/thread.rb", include_str!("../../../core/thread.rb")),
    ("core/fiber.rb", include_str!("../../../core/fiber.rb")),
    (
        "core/enumerator.rb",
        include_str!("../../../core/enumerator.rb"),
    ),
    (
        "core/arithmetic_sequence.rb",
        include_str!("../../../core/arithmetic_sequence.rb"),
    ),
    (
        "core/pathname.rb",
        include_str!("../../../core/pathname.rb"),
    ),
    // Last: it re-points names the files above defined twice.
    ("core/aliases.rb", include_str!("../../../core/aliases.rb")),
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
                // CRuby's spelling for a core file written in Ruby, and the
                // mark a backtrace uses to tell a core frame from the program's
                // own (#29): `<internal:array>`, never a path a user file has.
                let path = format!(
                    "<internal:{}>",
                    name.trim_start_matches("core/").trim_end_matches(".rb")
                );
                let parsed = spinel_parse::parse_file(&path, source.as_bytes());
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

/// The libraries `require` finds inside the binary rather than on the load
/// path: the built-in gems whose C half is Spinel's to write, in Ruby where
/// Ruby can say it (`docs/engine.md`, "Built-in gems"). Loaded on `require`,
/// not at boot, as CRuby loads them.
const LIBRARIES: &[(&str, &str)] = &[
    ("stringio", include_str!("../../../lib/stringio.rb")),
    ("fcntl", include_str!("../../../lib/fcntl.rb")),
    ("strscan", include_str!("../../../lib/strscan.rb")),
    ("digest", include_str!("../../../lib/digest.rb")),
    ("digest/md5", include_str!("../../../lib/digest/md5.rb")),
    ("digest/sha1", include_str!("../../../lib/digest/sha1.rb")),
    ("digest/sha2", include_str!("../../../lib/digest/sha2.rb")),
    (
        "digest/rmd160",
        include_str!("../../../lib/digest/rmd160.rb"),
    ),
    (
        "digest/bubblebabble",
        include_str!("../../../lib/digest/bubblebabble.rb"),
    ),
    ("io/nonblock", include_str!("../../../lib/io/nonblock.rb")),
    ("socket", include_str!("../../../lib/socket.rb")),
    ("date", include_str!("../../../lib/date.rb")),
    ("date_core", include_str!("../../../lib/date_core.rb")),
];

fn library(name: &str) -> Option<&'static str> {
    LIBRARIES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, source)| *source)
}

/// Define the core library's methods in this heap.
///
/// Call once, after [`HandleScope::bootstrap`] has created the classes this
/// fills in. Panics if `core/*.rb` raises, for the same reason the compile does.
pub fn boot(scope: &mut HandleScope<'_>) {
    scope.set_parser(parse_eval);
    scope.set_libraries(library);
    for (index, iseq) in image().iter().enumerate() {
        let mut frame = interp::Frame::new(0);
        if let Err(err) = interp::eval_in(scope, &mut frame, iseq) {
            let (name, _) = SOURCES[index];
            panic!("{name} raised while loading the core library: {err:?}");
        }
    }
    interp::watch_optimised(scope);
}

/// End the program: run its `at_exit` blocks and answer its exit status —
/// `SystemExit#status` when `exit` ended it, 1 for another uncaught
/// exception (left in `$!` by the VM), 0 otherwise. `None` when the status
/// cannot be worked out, which is a refusal inside an `at_exit` block.
pub fn exit_status(scope: &mut HandleScope<'_>, raised: bool) -> Option<u8> {
    let source: &[u8] = if raised {
        b"Kernel.__exit__($! || RuntimeError.new)"
    } else {
        b"Kernel.__exit__(nil)"
    };
    let parsed = spinel_parse::parse_file("<exit>", source);
    let iseq = compile::program(&parsed.program).ok()?;
    let mut frame = interp::Frame::new(0);
    let status = interp::eval_in(scope, &mut frame, &iseq).ok()?;
    status.as_fixnum().map(|status| (status & 0xff) as u8)
}

/// Print the exception that ended the program as CRuby's top level prints
/// it — `full_message`, with its cause and backtrace — unless it is one CRuby
/// keeps quiet about. Answers false when there was nothing to print it from,
/// so the caller can say something itself.
pub fn report_uncaught(scope: &mut HandleScope<'_>) -> bool {
    let parsed = spinel_parse::parse_file("<exit>", b"Kernel.__report__");
    let Ok(iseq) = compile::program(&parsed.program) else {
        return false;
    };
    let mut frame = interp::Frame::new(0);
    interp::eval_in(scope, &mut frame, &iseq).is_ok_and(|value| value.is_truthy())
}

/// What string `eval` parses with (#38): the VM has no parser of its own.
fn parse_eval(
    path: &str,
    source: &[u8],
    line: i64,
    in_method: bool,
    encoding: Option<&str>,
) -> Result<ParsedSource, ParseFailure> {
    let parsed = spinel_parse::parse_eval(path, source, line, in_method, encoding);
    if let Some(error) = parsed.syntax_errors().next() {
        let at = parsed.program.source.line(error.span.start);
        return Err(ParseFailure::Syntax(format!(
            "{path}:{at}: {}",
            error.message
        )));
    }
    if parsed.lowering_bugs().next().is_some() {
        return Err(ParseFailure::Unsupported);
    }
    let warnings = parsed
        .warnings
        .iter()
        // ponytail: Ruby says "assigned but unused variable" for a file and
        // never for an `eval` string, and this hook is not told which it is
        // parsing. Dropped for both until it is.
        .filter(|warning| !warning.message.starts_with("assigned but unused variable"))
        .map(|warning| {
            let at = parsed.program.source.line(warning.span.start);
            (
                format!("{path}:{at}: warning: {}\n", warning.message),
                spinel_parse::is_verbose_warning(&warning.message),
            )
        })
        .collect();
    Ok(ParsedSource {
        program: parsed.program,
        warnings,
    })
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
