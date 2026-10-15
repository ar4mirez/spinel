//! Threads (#45) on the `spinel` binary: two threads waiting on IO wait at
//! the same time, and a thread that never waits is preempted rather than
//! starving the rest.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn threads_doing_io_overlap() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/threads/overlap.rb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "[\"0\\n\", \"1\\n\"]\noverlapped\npreempted\n"
    );
}
