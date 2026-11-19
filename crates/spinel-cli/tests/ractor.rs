//! Ractors (#117), on the `spinel` binary: each case under
//! `tests/fixtures/ractor/` runs and writes, byte for byte, what CRuby 4.0.9
//! wrote for it, recorded beside it as `<case>.stdout` and `<case>.stderr`.
//! The one thing normalised is an object's address in a thread's report,
//! `#<Thread:0x...>`, which no two runs share.
//!
//! ruby/spec has no Ractor specs (its `core/` has no `ractor/`), so these
//! are #117's definition of done in their place. Recorded with
//! `ruby -v` = `ruby 4.0.9 (2026-10-23 revision 3e93b23afc) +PRISM
//! [x86_64-linux]`, by running each case from this directory:
//! `ruby <case>.rb > <case>.stdout 2> <case>.stderr`.
//!
//! What each covers:
//! - `experimental_warning`: CRuby's warning, once, at the first `Ractor.new`.
//! - `value_and_join`: arguments, `#value`, `#join` answering the Ractor.
//! - `messages`: `send`/`<<`, `Ractor.receive`/`recv`, replies through
//!   `Ractor.main` and through a `Ractor::Port` passed in.
//! - `copy`: what is sent is a copy: the sender's String, Array, Hash and
//!   Struct are as they were; a frozen one stays frozen.
//! - `moved`: `move: true` leaves the sender a `Ractor::MovedObject`.
//! - `identity`: `current`, `main`, `main?`, `count`, `name:`, `#inspect`
//!   running and terminated.
//! - `remote_error`: the report of an exception that ends a Ractor, and
//!   `Ractor::RemoteError` with it as its cause, from `#value` and `#join`.
//! - `port_close`: a closed port refuses a send, and a receive once empty.
//! - `own_heap`: another Ractor's `ObjectSpace` and `GC.stat` are its heap's.
//! - `isolation`: what a Ractor other than the main one may read and write
//!   of the main one's: shareable constants, class variables, class
//!   instance variables, globals, and the `IsolationError` for the rest.
//! - `concurrency`: Ractors running while the main one defines methods,
//!   a thousand messages each way through a port, a Ractor made in a
//!   Ractor, a thread in one, `Ractor.select` and `#monitor`.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// `#<Thread:0x...` with the address taken out.
fn without_addresses(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("0x") {
        out.push_str(&rest[..at]);
        out.push_str("0x…");
        rest = rest[at + 2..].trim_start_matches(|c: char| c.is_ascii_hexdigit());
    }
    out.push_str(rest);
    out
}

fn case(name: &str, stdout: &str, stderr: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ractor");
    let output = Command::new(SPINEL)
        .current_dir(&dir)
        .arg("run")
        .arg(format!("{name}.rb"))
        .output()
        .expect("spinel runs");
    let (out, err) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.status.success(), "{name}: stderr: {err}");
    assert_eq!(out, stdout, "{name}: stdout");
    assert_eq!(
        without_addresses(&err),
        without_addresses(stderr),
        "{name}: stderr"
    );
}

macro_rules! cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                case(
                    stringify!($name),
                    include_str!(concat!("fixtures/ractor/", stringify!($name), ".stdout")),
                    include_str!(concat!("fixtures/ractor/", stringify!($name), ".stderr")),
                );
            }
        )*
    };
}

cases!(
    experimental_warning,
    value_and_join,
    messages,
    copy,
    moved,
    identity,
    remote_error,
    port_close,
    own_heap,
    isolation,
    concurrency,
);
