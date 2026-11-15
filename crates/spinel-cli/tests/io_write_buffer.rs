//! The write buffer as another process sees it, on the `spinel` binary
//! (#41). An IO that is neither `sync` nor a terminal holds what it writes,
//! up to 8 KiB, as CRuby's `wbuf` does, so the order its bytes reach a file
//! in is the order of the flushes, not of the writes.
//!
//! ruby/spec cannot say this: it is what a file shared by a parent, its
//! children and two streams holds afterwards.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// stdout and stderr share one open file, as `> f 2>&1` makes them. stderr
/// writes at once; stdout's lines wait for `fork`, `system` and `spawn`, which
/// flush it first, and for the end of the program, and a child's wait for the
/// child's end. The expected bytes are ruby 4.0.9's on Linux, which ran the
/// same fixture: `ruby fork_output_order.rb > f 2>&1`.
#[test]
fn stdout_and_stderr_to_one_file_keep_rubys_order_across_fork() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/fork_output_order.rb");
    let path = std::env::temp_dir().join(format!("spinel-fork-order-{}", std::process::id()));
    let file = std::fs::File::create(&path).expect("a scratch file");
    // One file description for both, so they share its offset.
    let both = file.try_clone().expect("a second descriptor for the file");
    let status = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(both)
        .status()
        .expect("spinel runs");
    let written = std::fs::read_to_string(&path).expect("the file was written");
    std::fs::remove_file(&path).ok();
    assert!(status.success(), "output: {written}");
    assert_eq!(
        written,
        "parent: stderr before fork\n\
         parent: before fork\n\
         child: stderr\n\
         child: stdout\n\
         parent: after fork\n\
         system: child\n\
         parent: before spawn\n\
         spawn: child\n\
         parent: stderr at end\n\
         parent: last line\n"
    );
}

/// A pipe's reader gone, a write to a writer that is not `sync` is held, so
/// the broken pipe is found by the `flush`, and again by the `close`, which
/// still has the bytes to send: CRuby keeps what a failed flush did not
/// write. ruby/spec's `IO#flush` example asks only that each raise.
#[test]
fn a_broken_pipe_surfaces_at_flush_and_again_at_close() {
    let output = Command::new(SPINEL)
        .arg("-e")
        .arg(
            "r, w = IO.pipe; w.sync = false; w.write('foo'); r.close; \
             a = (w.flush rescue $!.class); b = (w.close rescue $!.class); \
             p [a, b, w.closed?]",
        )
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "[Errno::EPIPE, Errno::EPIPE, true]\n"
    );
}
