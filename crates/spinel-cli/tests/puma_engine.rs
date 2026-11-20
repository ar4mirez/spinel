//! What Puma's suite found the engine answering otherwise than CRuby (#116),
//! each a program and what ruby 4.0.9 printed for it, in
//! `fixtures/puma_engine/`:
//!
//! - `super` in a block whose method's frame is not on the stack it runs on
//!   (a fiber's or a thread's first frame, a Proc called after its method
//!   returned) resolves against the method it was written in, which Puma's
//!   `FiberPerRequest` needs; it raised "super called outside of method".
//! - Byte escapes past ASCII in a UTF-8 pattern spell one character, as
//!   Onigmo reads them (Rack::Builder's BOM pattern); a match that walks a
//!   long subject once is answered rather than refused.
//! - `raise exception, message` raises `exception.exception(message)`.
//! - `IO#read` and its kin raise `TypeError` for a buffer that is not a
//!   String.
//! - A forked child of a process that had threads holds no more descriptors
//!   than its parent (Puma's mold workers fork generation after generation).
//! - `A::X` reads `A`'s own constants before a module prepended to `A`
//!   (minitest-retry prepends to `Minitest`).
//! - On Linux, a UNIX socket name starting with a NUL is in the abstract
//!   namespace, as Puma's `unix://@name` binds it.
//! - `String#empty?` is whether there are no bytes: it neither counts
//!   characters, which made writing a large body through MiniSSL quadratic,
//!   nor asks `length`.
//!
//! No ruby/spec example covers these. Everything runs offline.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/puma_engine")
}

/// Runs `name.rb` from the fixtures directory, where `__FILE__` is the bare
/// name, as it was when ruby wrote `name.out`, and compares.
fn check(name: &str) {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(format!("{name}.rb"))
        .current_dir(fixtures())
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = std::fs::read_to_string(fixtures().join(format!("{name}.out")))
        .unwrap_or_else(|error| panic!("{name}.out is readable: {error}"));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn super_in_a_block_away_from_its_method() {
    check("super_in_blocks");
}

#[test]
fn utf8_byte_escapes_and_long_matches() {
    check("regexp_byte_escapes");
}

#[test]
fn raise_with_an_exception_and_a_message() {
    check("raise_with_message");
}

#[test]
fn read_into_a_buffer_that_is_not_a_string() {
    check("io_read_buffer");
}

#[test]
fn empty_is_whether_there_are_no_bytes() {
    check("string_empty");
}

#[test]
fn a_prepended_module_does_not_shadow_a_constant() {
    check("prepended_constant");
}

#[test]
#[cfg(unix)]
fn forked_children_keep_their_parents_descriptors() {
    if !std::path::Path::new("/dev/fd").is_dir() {
        eprintln!("skipped: no /dev/fd here");
        return;
    }
    check("fork_descriptors");
}

#[test]
#[cfg(any(target_os = "linux", target_os = "android"))]
fn unix_sockets_in_the_abstract_namespace() {
    check("abstract_unix_socket");
}
