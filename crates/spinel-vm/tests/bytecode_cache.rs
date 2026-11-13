//! The bytecode cache's format (#40, `src/sbc.rs`) against real compiles:
//! every file of the core library reads back as exactly what the compiler
//! made, and one read-back `Iseq` runs in two heaps on two threads at once,
//! as the compiler's own output would.
//!
//! Spinel has no `Ractor` yet; a heap is what each Ractor will own (`docs/
//! engine.md`, "One `Heap` per Ractor"), and an `Iseq` is what they share. So
//! "valid across Ractors" is shown here as that sharing: one immutable,
//! `Send + Sync` value, run by two heaps that share nothing else.
//!
//! Skipped under miri: `spinel_parse` calls into Prism, which is C. The
//! format's own Miri coverage is `sbc::tests`, which builds its `Iseq` by
//! hand.
#![cfg(not(miri))]

use std::path::Path;
use std::sync::Arc;

use spinel_vm::{Heap, Iseq, compile, interp, sbc};

fn compile_file(path: &str, source: &[u8]) -> Iseq {
    let parsed = spinel_parse::parse_file(path, source);
    assert!(parsed.errors.is_empty(), "{path} did not parse");
    compile::program_with(&parsed.program, "<top (required)>", None)
        .unwrap_or_else(|e| panic!("{path} did not compile: {e}"))
}

fn round_trip(path: &str, source: &[u8], iseq: &Iseq) -> Arc<Iseq> {
    let key = sbc::key(source);
    let bytes = sbc::encode(iseq, &[], path, &key, None).expect("a compiled file is cacheable");
    sbc::decode(&bytes, path, &key, None)
        .expect("what was written reads back")
        .iseq
}

#[test]
fn every_core_file_reads_back_as_the_compiler_made_it() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core");
    let mut files: Vec<_> = std::fs::read_dir(&core)
        .expect("core/ is there")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rb"))
        .collect();
    files.sort();
    assert!(files.len() > 20, "found only {} core files", files.len());
    for file in files {
        let source = std::fs::read(&file).expect("a core file reads");
        let path = file.to_string_lossy();
        let iseq = compile_file(&path, &source);
        let read = round_trip(&path, &source, &iseq);
        assert!(*read == iseq, "{path} read back differently");
        // `PartialEq` on a float literal cannot see `-0.0` from `0.0`, and
        // the derived `Debug` can.
        assert_eq!(format!("{read:?}"), format!("{iseq:?}"), "{path}");
    }
}

const PROGRAM: &str = r##"
class Tally
  def initialize = @counts = Hash.new(0)
  def add(word) = @counts[word] += 1
  def top(n) = @counts.sort_by { |word, count| [-count, word] }.first(n)
end
tally = Tally.new
"the cat and the hat and the bat".split.each { |word| tally.add(word) }
words = tally.top(2).map { |word, count| "#{word}=#{count}" }
[words, __FILE__, __LINE__, "x".frozen?, /a(t)/ =~ "hat" && $1, 2**70, -0.0]
"##;

fn run(iseq: &Iseq) -> String {
    let mut heap = Heap::new();
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);
    let mut frame = interp::Frame::new(iseq.locals.len());
    let value = interp::eval_in(&mut scope, &mut frame, iseq).expect("the program runs");
    interp::inspect(&mut scope, value)
}

#[test]
fn one_read_back_iseq_runs_in_two_heaps_on_two_threads() {
    let path = "/project/lib/tally.rb";
    let fresh = compile_file(path, PROGRAM.as_bytes());
    let expected = run(&fresh);
    assert_eq!(
        expected,
        r#"[["the=3", "and=2"], "/project/lib/tally.rb", 10, false, "t", 1180591620717411303424, -0.0]"#
    );
    let shared = round_trip(path, PROGRAM.as_bytes(), &fresh);
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let iseq = Arc::clone(&shared);
            std::thread::spawn(move || run(&iseq))
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().expect("the thread finishes"), expected);
    }
}
