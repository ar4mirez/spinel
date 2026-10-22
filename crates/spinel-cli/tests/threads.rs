//! Threads (#45) on the `spinel` binary: two threads waiting on IO wait at
//! the same time, and a thread that never waits is preempted rather than
//! starving the rest. Each runs on an OS thread of its own (#328), so one
//! blocked in the kernel leaves the rest running, and the program's end
//! kills those still alive.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    // A hang fails the test in a minute, with what the program printed.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(Box::new(child.stdout.take().expect("piped stdout")));
    let stderr = drain(Box::new(child.stderr.take().expect("piped stderr")));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("spinel waits") {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    let status = status.unwrap_or_else(|| panic!("timed out\nstdout: {stdout}\nstderr: {stderr}"));
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(
        stdout,
        "own\nopened opened\nran meanwhile\nalive at exit\nkilled at the end\n"
    );
}
