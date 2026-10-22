//! Threads (#45) on the `spinel` binary: two threads waiting on IO wait at
//! the same time, and a thread that never waits is preempted rather than
//! starving the rest. Each runs on an OS thread of its own (#328), so one
//! blocked in the kernel leaves the rest running, and the program's end
//! kills those still alive.

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

#[test]
fn threads_run_on_os_threads_of_their_own() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/threads/os_threads.rb");
    let dir = std::env::temp_dir().join(format!("spinel-os-threads-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .output()
        .expect("spinel runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "own\nthrough\nran meanwhile\nalive at exit\nkilled at the end\n"
    );
}
