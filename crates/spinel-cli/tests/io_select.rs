//! `IO.select` and the scheduler asking `poll(2)` about one descriptor
//! twice (#41): the same IO in two sets of one `IO.select`, and two threads
//! waiting on one pipe. macOS's `poll` keeps one registration per
//! descriptor, so the second entry replaced the first and the wait never
//! ended; ruby/spec's `core/io/select_spec.rb` hung there on macOS.
//!
//! The expected output is ruby 4.0's, which ran the same fixture. A hang
//! fails the test instead of wedging it.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn one_io_asked_about_twice_is_answered() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/select_one_io_twice.rb");
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(fixture)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the child can be waited for") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("IO.select on one IO asked about twice hung");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, "[1, 0, 0]\n[1, 0, 0]\n[0, 1]\n", "stderr: {stderr}");
}
