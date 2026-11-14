//! `require` from several threads on the `spinel` binary (#39): a thread
//! that finds a file part-way through loading in another waits on a barrier
//! the scheduler sees, so the file runs once and every `require` of it
//! returns after it has finished, and two threads each waiting for the
//! other's load — a `require` or an autoload — end in CRuby's deadlock
//! `fatal` rather than a hang. Each expectation is CRuby 4.0.9's output for
//! the same fixture, but for the thread addresses and the per-thread dump
//! CRuby appends to the message.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// Run a fixture, failing the test after a minute rather than hanging with
/// it; answers the status, stdout and stderr.
fn run(fixture: &str) -> (ExitStatus, String, String) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/require/threads")
        .join(fixture);
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
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
    let status = status.unwrap_or_else(|| panic!("timed out\nstdout: {stdout}\nstderr: {stderr}"));
    (status, stdout, stderr)
}

#[test]
fn threads_requiring_one_file_run_it_once_and_all_return_after_it_finishes() {
    let (status, stdout, stderr) = run("once.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stderr, "");
    assert_eq!(stdout, "{runs: 1, true: 1, all_after: true}\n");
}

/// The main thread, joining threads that wait on each other, raises the
/// deadlock `fatal` from `join` at `line` of `fixture`.
fn assert_deadlock(fixture: &str, line: u32) {
    let (status, stdout, stderr) = run(fixture);
    assert_eq!(status.code(), Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stdout, "");
    let mut lines = stderr.lines();
    let first = lines.next().unwrap_or_default();
    let expected =
        format!("/{fixture}:{line}:in 'Thread#join': No live threads left. Deadlock? (fatal)");
    assert!(first.ends_with(&expected), "stderr: {stderr}");
    let second = lines.next().unwrap_or_default();
    assert!(
        second.starts_with("3 threads, 3 sleeps current:0x") && second.contains(" main thread:0x"),
        "stderr: {stderr}"
    );
}

#[test]
fn two_threads_requiring_each_others_file_end_in_the_deadlock_fatal() {
    assert_deadlock("cycle.rb", 9);
}

#[test]
fn two_threads_autoloading_each_others_constant_end_in_the_deadlock_fatal() {
    assert_deadlock("autoload_cycle.rb", 10);
}
