//! The backtraces an `at_exit` block and a `trap(:EXIT)` handler see, on
//! the `spinel` binary (#899): only the program's frames, as CRuby's, with
//! none of the code that ends the program — the `<exit>` frames Minitest
//! printed at the end of every failure under `spinel test`.
//!
//! ruby/spec cannot say this: its examples run inside mspec, not at exit.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn an_at_exit_block_sees_only_the_programs_frames() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run");
    let output = Command::new(SPINEL)
        .args(["run", "at_exit_backtrace.rb"])
        .current_dir(&dir)
        .output()
        .expect("spinel runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // CRuby 4.0's output for the same file, run from its directory: the
    // `trap(:EXIT)` handler first, then the blocks, last registered first.
    assert_eq!(
        stdout,
        "[]\n\
         []\n\
         []\n\
         [\"at_exit_backtrace.rb:13:in 'block in <main>'\"]\n\
         [\"at_exit_backtrace.rb:13:in 'block in <main>'\"]\n",
        "stderr: {stderr}"
    );
    // The block that raised, reported as CRuby reports it, and the rest
    // still ran: the status is the uncaught error's.
    assert_eq!(
        stderr,
        "at_exit_backtrace.rb:5:in 'Object#fails': boom (RuntimeError)\n\
         \tfrom at_exit_backtrace.rb:7:in 'block in <main>'\n"
    );
    assert_eq!(output.status.code(), Some(1));
}
