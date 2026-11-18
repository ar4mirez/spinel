//! `fiddle` on `Spinel::FFI` (#105): the gem's Ruby half, fiddle 1.1.8's
//! `lib/` vendored in `stdlib/`, over its C half in Ruby
//! (`lib/fiddle/spinel.rb`) over libffi (`crates/spinel-vm/src/ffi.rs`).
//!
//! ruby/spec's `library/fiddle` is one example, so these are the check the
//! issue asks for. Each fixture under `fixtures/fiddle/` calls into the C
//! library the `spinel` binary links, and its output must be what ruby 4.0.9
//! with fiddle 1.1.8 prints, which is the `.out` beside it (regenerate one with
//! `ruby fixtures/fiddle/<name>.rb > fixtures/fiddle/<name>.out`):
//!
//! - `struct_layout.rb`: `Fiddle::Importer` structs and unions laid out as the
//!   platform's C ABI lays them out, checked against structs C itself fills.
//! - `closures.rb`: `Fiddle::Closure` called from C — `qsort`, `bsearch`, a
//!   callback making a C call with a closure of its own, a block raising or
//!   throwing out of C.
//! - `pointer.rb`: `Fiddle::Pointer` arithmetic, byte access, dereferencing,
//!   freeing, and C writing through a pointer and into a String.
//! - `functions.rb`: `Fiddle::Handle` and `Fiddle::Function` with every
//!   argument and return type, variadic calls, `errno`, and the errors.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fiddle")
}

/// Run `fixtures/fiddle/<name>.rb` and check its output is `<name>.out`.
fn prints_what_ruby_prints(name: &str) {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join(format!("{name}.rb")))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed on {name}.rb: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = std::fs::read_to_string(fixtures().join(format!("{name}.out")))
        .expect("the expected output is readable");
    let actual = String::from_utf8_lossy(&output.stdout);
    if actual != expected {
        let first = expected
            .lines()
            .zip(actual.lines())
            .position(|(e, a)| e != a)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name}.rb's output differs from ruby 4.0.9's at line {}:\n  ruby:   {:?}\n  spinel: {:?}",
            first + 1,
            expected.lines().nth(first),
            actual.lines().nth(first)
        );
    }
}

#[test]
fn struct_layout_follows_the_c_abi() {
    prints_what_ruby_prints("struct_layout");
}

#[test]
fn closures_are_called_from_c() {
    prints_what_ruby_prints("closures");
}

#[test]
fn pointer_arithmetic_and_memory() {
    prints_what_ruby_prints("pointer");
}

#[test]
fn handles_and_functions() {
    prints_what_ruby_prints("functions");
}

/// What a fixture cannot compare with CRuby, because the order threads run
/// in is not CRuby's: callbacks on several threads at once, each switching
/// threads mid-callback, so that C calls suspended in a closure interleave;
/// a callback that leaves its fiber in the middle of `qsort`; and closures
/// and pointers nothing holds, collected and their C halves freed.
#[test]
fn callbacks_across_threads_fibers_and_collection() {
    let script = r#"
require "fiddle"
include Fiddle
qsort = Function.new(Handle::DEFAULT["qsort"], [TYPE_VOIDP, TYPE_SIZE_T, TYPE_SIZE_T, TYPE_VOIDP], TYPE_VOID)
sorted = 4.times.map do |t|
  Thread.new do
    data = Pointer.malloc(6 * SIZEOF_INT, RUBY_FREE)
    data[0, 24] = [5, 1, 4, 2, 3, 6].map { |x| x * (t + 1) }.pack("i*")
    compare = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
      Thread.pass
      sleep 0.001 if t.odd?
      a[0, 4].unpack1("i") <=> b[0, 4].unpack1("i")
    end
    qsort.call(data, 6, SIZEOF_INT, compare)
    data[0, 24].unpack("i*")
  end
end.map(&:value)
puts sorted.inspect
pairs = Enumerator.new do |y|
  data = Pointer.malloc(4 * SIZEOF_INT, RUBY_FREE)
  data[0, 16] = [3, 1, 2, 0].pack("i*")
  compare = Closure::BlockCaller.new(TYPE_INT, [TYPE_VOIDP, TYPE_VOIDP]) do |a, b|
    x, z = a[0, 4].unpack1("i"), b[0, 4].unpack1("i")
    y << [x, z]
    x <=> z
  end
  qsort.call(data, 4, SIZEOF_INT, compare)
  y << data[0, 16].unpack("i*")
end
puts pairs.next.size, pairs.to_a.last.inspect
300.times do
  Closure::BlockCaller.new(TYPE_INT, [TYPE_INT]) { |x| x }
  Pointer.malloc(64, RUBY_FREE)
end
GC.start
GC.start
puts Spinel::FFI.instance_variable_get(:@closures).size < 300
kept = Closure::BlockCaller.new(TYPE_INT, [TYPE_INT]) { |x| x + 1 }
puts Function.new(kept, [TYPE_INT], TYPE_INT).call(41)
"#;
    let output = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "[[1, 2, 3, 4, 5, 6], [2, 4, 6, 8, 10, 12], [3, 6, 9, 12, 15, 18], [4, 8, 12, 16, 20, 24]]\n\
         2\n[0, 1, 2, 3]\ntrue\n42\n"
    );
}

/// `Spinel::FFI` itself, the layer `fiddle` stands on: a library opened, a
/// function looked up and called, C memory written and read, and a closure.
#[test]
fn spinel_ffi_calls_c_directly() {
    let script = r#"
require "spinel/ffi"
ffi = Spinel::FFI
libc = ffi.open(nil)
strlen = ffi.symbol(libc, "strlen")
name = ffi.malloc(8)
ffi.write(name, "hello\0")
p ffi.call(strlen, [ffi::VOIDP], -ffi::LONG, [name])[0]
p ffi.string(name), ffi.read(name, 3)
id, add = ffi.closure(ffi::INT, [ffi::INT, ffi::INT]) { |a, b| a + b }
p ffi.call(add, [ffi::INT, ffi::INT], ffi::INT, [40, 2])[0]
ffi.free_closure(id)
ffi.free(name)
p ffi.symbol(libc, "no_such_function_here")
begin
  ffi.open("doesnotexist.doesnotexist")
rescue Spinel::FFI::Error => error
  p error.message.include?("doesnotexist")
end
p [ffi.sizeof(ffi::INT), ffi.alignof(ffi::DOUBLE), ffi.type(:size_t)]
"#;
    let output = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "5\n\"hello\"\n\"hel\"\n42\nnil\ntrue\n[4, 8, -5]\n"
    );
}
