//! Nonblocking pipes on the `spinel` binary (#41). The descriptors Spinel
//! makes for a pipe are `O_NONBLOCK`, as CRuby's are: a write bigger than
//! the pipe holds takes what fits and waits for the rest in the scheduler,
//! so another thread can drain the pipe and a signal ends the wait. A
//! child's standard streams are made blocking.
//!
//! ruby/spec cannot say this: it is a write that hung for ever, a signal a
//! parent sends, and the flags a child program sees. Each expected output is
//! ruby 4.0.9's on Linux, which ran the same fixture.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/io")
        .join(name)
}

/// The child's status once it ends, or a panic naming `what` if it has not
/// in `patience`: these fixtures hung before pipes were nonblocking.
fn wait_or_kill(child: &mut Child, patience: Duration, what: &str) -> ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("the child can be waited for") {
            return status;
        }
        if started.elapsed() > patience {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what} hung");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Run a fixture to its end, its output collected.
fn run(name: &str) -> Output {
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(fixture(name))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    };
    let stdout = drain(Box::new(child.stdout.take().expect("piped stdout")));
    let stderr = drain(Box::new(child.stderr.take().expect("piped stderr")));
    let status = wait_or_kill(&mut child, Duration::from_secs(60), name);
    Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }
}

/// A megabyte written to a pipe another thread reads. The write used to be
/// one blocking `write(2)` made holding the lock, which the reader then
/// never got to drain. The write buffer's flush waits the same way, and
/// `syswrite` takes what fits.
#[test]
fn a_big_write_to_a_pipe_a_thread_reads_waits_for_it() {
    let output = run("pipe_write_waits.rb");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "1000000\n1000110\n65536\n10\n16384\n"
    );
}

/// A lone thread writing to a full pipe ends on SIGTERM. Its `write(2)` was
/// restarted under `SA_RESTART` and never reached the safe point where the
/// handler runs; now it waits in a `poll(2)` the signal cuts short.
#[test]
fn a_write_to_a_full_pipe_ends_on_sigterm() {
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(fixture("pipe_write_full.rb"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    let mut stderr = BufReader::new(child.stderr.take().expect("piped stderr"));
    let mut line = String::new();
    stderr
        .read_line(&mut line)
        .expect("the fixture says it writes");
    assert_eq!(line, "writing\n");
    // Time for the write to fill the pipe and wait; a signal sent sooner
    // ends the program as surely.
    thread::sleep(Duration::from_millis(200));
    let pid = libc::pid_t::try_from(child.id()).expect("a pid");
    // SAFETY: `kill` takes integers; the child is ours and not yet reaped.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = wait_or_kill(
        &mut child,
        Duration::from_secs(10),
        "a write to a full pipe, sent SIGTERM,",
    );
    let mut rest = String::new();
    let _ = stderr.read_to_string(&mut rest);
    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {rest}");
    assert!(!rest.contains("never"), "stderr: {rest}");
}

/// `nonblock?` on pipes, the standard streams and a file, and what a child
/// sees: blocking standard streams, given or inherited, which leaves the
/// parent's descriptor for the same pipe end blocking for good. A descriptor
/// above 2 and a `fork` keep the flag. The child is Spinel itself, through
/// `RbConfig.ruby`.
#[test]
fn pipes_are_nonblocking_and_a_childs_standard_streams_are_not() {
    let output = run("pipe_nonblock_children.rb");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "[:pipe, true, true, true]\n\
         [:stdio, false, false]\n\
         [:file, false]\n\
         [:popen, true, \"[false, false, false]\\n\"]\n\
         [false, false, false]\n\
         [:system_in, false, true]\n\
         [:spawn_fd3, true]\n\
         [:fork, true]\n\
         [false, false, false]\n\
         [:inherited, false, false]\n\
         [:set, false, true]\n\
         [:cleared, false]\n"
    );
}
