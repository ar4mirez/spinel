//! `TracePoint` (#127) on the `spinel` binary: each case under
//! `tests/fixtures/tracepoint/` runs and writes, byte for byte, what CRuby
//! 4.0.9 wrote for it, recorded beside it as `<case>.stdout` and
//! `<case>.stderr` with `ruby -v` = `ruby 4.0.9 (2026-10-23 revision
//! 3e93b23afc) +PRISM [x86_64-linux]`, by running each case from that
//! directory: `ruby <case>.rb > <case>.stdout 2> <case>.stderr`.
//!
//! `core/tracepoint/` in ruby/spec is #127's definition of done; these are
//! what it does not check:
//! - `deopt`: a loop on every fast path the interpreter has — fused
//!   superinstructions, inline-cached sends and `attr_reader`s, a `yield`,
//!   light stretches — warmed up for thousands of turns and then traced from
//!   a call it makes partway through, which sees every event from the next
//!   instruction on, the `:return` of frames entered before it included.
//!   That is the issue's second box: there is no compiled code yet (the JIT
//!   is phase 6), and an enabled `TracePoint` takes the interpreter off its
//!   fast paths rather than letting them run past an event.
//! - `unwind`: the `:return` of methods an exception, a block's `return`, a
//!   `break` and a `throw` leave, one `:raise` where an exception starts, a
//!   `:return` after an `ensure` on the `return`'s line, and none for an
//!   exception the core library raises and rescues itself.
//! - `hooks`: hooks that disable themselves while an unwind is parked for
//!   them, a hook that raises, one whose `:line` raises before a `begin`
//!   body's first line runs, a `:line`'s `binding` line, a method's
//!   `:return` on the line a hook raised on, two `TracePoint`s' order,
//!   `$!` inside a hook,
//!   the names, classes and bindings an event reports, one thread's events,
//!   `enable(target:, target_line:)`, and the `:line`s of endless defs.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn case(name: &str, stdout: &str, stderr: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tracepoint");
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
    assert_eq!(err, stderr, "{name}: stderr");
}

macro_rules! cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                case(
                    stringify!($name),
                    include_str!(concat!("fixtures/tracepoint/", stringify!($name), ".stdout")),
                    include_str!(concat!("fixtures/tracepoint/", stringify!($name), ".stderr")),
                );
            }
        )*
    };
}

cases!(deopt, unwind, hooks);
