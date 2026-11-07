//! Signals and `ENV` (#44) on the `spinel` binary, for what no ruby/spec
//! example covers: a handler runs on the main thread at a safe point and the
//! wait it interrupted goes on; it runs even when the main thread is almost
//! always in the core library; a signal met during the program's last call
//! still ends it; a handler that raises leaves the other signals waiting; a
//! forked child wakes for its own signals, even one sent the moment it is
//! forked; SIGINT cuts a blocked read short; and `ENV` is the environment a
//! child inherits. Each fixture's output was measured against ruby 4.0.9, but
//! for the one difference `last_call.rb` names.

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// Run a fixture, killing it if it has not ended in a minute: a signal that
/// is never handled would otherwise hang the test.
fn run(fixture: &str) -> (ExitStatus, String, String) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/signals")
        .join(fixture);
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .stdin(Stdio::null())
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
        if let Some(status) = child.try_wait().expect("the child can be waited for") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{fixture} hung; stdout: {}; stderr: {}",
                stdout.join().unwrap_or_default(),
                stderr.join().unwrap_or_default()
            );
        }
        thread::sleep(Duration::from_millis(20));
    };
    (
        status,
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    )
}

#[test]
fn a_handler_runs_on_the_main_thread_and_the_sleep_goes_on() {
    let (status, stdout, stderr) = run("sleep_goes_on.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "[true]\nslept on\n");
}

#[test]
fn a_signal_reaches_a_main_thread_in_the_core_library() {
    let (status, stdout, stderr) = run("core_loop.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "handled\n");
}

#[test]
fn a_signal_met_in_the_last_call_still_ends_the_program() {
    // CRuby's `waitpid` is interrupted and the program ends inside `system`;
    // Spinel's is restarted and the signal is handled when it returns, so
    // "after" is printed first (the `ponytail:` in `shared/signals.rs`).
    // Either way the process ends by SIGTERM, which is what a parent sees.
    let (status, _, stderr) = run("last_call.rb");
    // SIGTERM is 15 on every platform Spinel builds for.
    assert_eq!(status.signal(), Some(15), "stderr: {stderr}");
}

#[test]
fn a_handler_that_raises_leaves_the_other_signals_waiting() {
    let (status, stdout, stderr) = run("raise_keeps_rest.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "[\"interrupt\", \"usr1\"]\n");
}

#[test]
fn a_forked_child_handles_its_own_signals() {
    let (status, stdout, stderr) = run("forked_child.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "child\n0\n");
}

#[test]
fn a_signal_sent_right_after_fork_reaches_the_child() {
    for _ in 0..20 {
        let (status, stdout, stderr) = run("kill_after_fork.rb");
        assert!(status.success(), "stderr: {stderr}");
        assert_eq!(stdout, "child\n0\n");
    }
}

#[test]
fn sigint_cuts_a_blocked_read_short() {
    let (status, stdout, stderr) = run("interrupted_read.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "Interrupt\n");
}

#[test]
fn env_is_the_environment_a_child_inherits() {
    let (status, stdout, stderr) = run("env_live.rb");
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(
        stdout,
        "set\n[unset]\nab\n{\"SPINEL_SIGNALS_A\" => \"a\", \"SPINEL_SIGNALS_B\" => \"b\"}\n"
    );
}
