//! Ractors (#117), what may be shared between them (#118), and their running
//! in parallel (#119), on the `spinel` binary: each case under
//! `tests/fixtures/ractor/` runs and writes, byte for byte, what CRuby 4.0.9
//! wrote for it, recorded beside it as `<case>.stdout` and `<case>.stderr`.
//! The one thing normalised is an object's address in a thread's report,
//! `#<Thread:0x...>`, which no two runs share.
//!
//! ruby/spec has no Ractor specs (its `core/` has no `ractor/`, upstream
//! either), so these are #117's and #118's definition of done in their
//! place, and #119's beside the benchmark in `bench/ractors/`. Recorded with
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
//! - `shareable`: `Ractor.shareable?` over immutable values, frozen
//!   containers of mutable and of frozen objects, core classes' own state,
//!   `Proc`s and `Thread`s.
//! - `make_shareable`: the deep freeze, `copy: true`, the classes it
//!   refuses and what it froze before one, and its checks of a `Proc`: its
//!   `self`, and the variables outside it it reads or writes.
//! - `shareable_proc`: `Ractor.shareable_proc` and `shareable_lambda`,
//!   `self:`, the variables a shareable `Proc` may read (assigned once,
//!   shareable) and may not write, `yield`, and one going to another Ractor
//!   by a message, an argument, a constant and a `define_method` body.
//! - `refusals`: `Ractor.new`'s isolation, a `Proc` sent, unshareable
//!   constants and `define_method` bodies read by another Ractor, a
//!   constant set to a mutable object there: each raises and leaves what it
//!   refused as it was.
//! - `parallel`: four Ractors doing CPU-bound work through method calls,
//!   blocks, attr_readers, Hash literals, `respond_to?` and sends to two
//!   classes from one site, while the main Ractor defines methods on those
//!   classes and each Ractor defines one of its own on a shared module.
//! - `outside_lock`: Ractors blocked in `flock`, a call made outside the
//!   per-Ractor lock — one alone, and one whose two threads both are — while
//!   the main Ractor defines a class and a method, which waited for them for
//!   ever while they kept the class lock.
//! - `env`: `ENV` shareable and not frozen in every Ractor (#865), and four
//!   Ractors writing and reading it in parallel, their own variables and
//!   each other's, with no read that sees what it should not.
//!
//! A case that has not finished in [`DEADLINE`] fails rather than hangs, as
//! `outside_lock` did before #119.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// How long a case may run: minutes more than any takes on a loaded machine.
const DEADLINE: Duration = Duration::from_secs(120);

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

/// Everything `pipe` holds until it closes, read on a thread of its own.
fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).expect("the pipe reads");
        bytes
    })
}

fn case(name: &str, stdout: &str, stderr: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ractor");
    let mut child = Command::new(SPINEL)
        .current_dir(&dir)
        .arg("run")
        .arg(format!("{name}.rb"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    // Both pipes are read on threads of their own, so a case that writes
    // more than a pipe holds is not stopped by it while it is waited for.
    let stdout_reader = drain(child.stdout.take().expect("stdout is piped"));
    let stderr_reader = drain(child.stderr.take().expect("stderr is piped"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the child is waited for") {
            break status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{name}: still running after {DEADLINE:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout_bytes, stderr_bytes) = (
        stdout_reader.join().expect("stdout is read"),
        stderr_reader.join().expect("stderr is read"),
    );
    let (out, err) = (
        String::from_utf8_lossy(&stdout_bytes),
        String::from_utf8_lossy(&stderr_bytes),
    );
    assert!(status.success(), "{name}: stderr: {err}");
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
    shareable,
    make_shareable,
    shareable_proc,
    refusals,
    parallel,
    outside_lock,
    env,
);
