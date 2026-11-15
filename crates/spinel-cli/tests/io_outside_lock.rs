//! Calls made outside the per-Ractor lock, and interrupted there, on the
//! `spinel` binary (#41). A call that blocks in the kernel — `flock`, a read
//! or write of a blocking pipe — is made outside the lock, so the other
//! threads run meanwhile, and a kill, a raise, a `close` in another thread, a
//! signal and the program's end each cut it short, as CRuby's unblocking
//! function does. The main thread alone is cut short by a signal.
//!
//! ruby/spec cannot say most of this: it is a wait that hung for ever, a
//! signal a parent sends, a lock another process holds, and the standard
//! output a program was given. Each expected output is ruby 4.0.9's on Linux,
//! which ran the same fixture.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/io")
        .join(name)
}

/// A file of this test's own to lock, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("spinel-outside-lock-{}-{name}", std::process::id()));
        File::create(&path).expect("the scratch file is made");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The child's status once it ends, or a panic naming `what` if it has not
/// in `patience`: each of these hung before.
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

/// Run a fixture with `args` to its end, its output collected.
fn run(name: &str, args: &[&std::ffi::OsStr]) -> Output {
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(fixture(name))
        .args(args)
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

/// Each line the child writes to its standard error, as it writes it.
fn lines(child: &mut Child) -> Receiver<String> {
    let stderr = BufReader::new(child.stderr.take().expect("piped stderr"));
    let (send, lines) = channel();
    thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    lines
}

/// The next line, or a panic naming `what` if none comes in time.
fn next_line(lines: &Receiver<String>, what: &str) -> String {
    lines
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("no line from {what}"))
}

fn signal(child: &Child, signo: libc::c_int) {
    let pid = libc::pid_t::try_from(child.id()).expect("a pid");
    // SAFETY: `kill` takes integers; the child is ours and not yet reaped.
    assert_eq!(unsafe { libc::kill(pid, signo) }, 0);
}

/// R2: a thread waits in `flock` for a lock another File holds. The wait
/// used to be made holding the lock, so nothing else ran and the program
/// hung; now a ticker thread ticks, the waiter sleeps, and a kill ends its
/// wait.
#[test]
fn a_thread_waiting_for_a_lock_lets_the_others_run_and_a_kill_ends_it() {
    let scratch = Scratch::new("r2");
    let output = run("flock_waits_outside.rb", &[scratch.0.as_os_str()]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "[:ticks, 10, \"sleep\"]\n:joined\n"
    );
}

/// A thread's `flock` wait is cut short by another thread's raise, which it
/// raises; by its File closed in another thread, `IOError: stream closed in
/// another thread`; and by the program's end, whose kill runs its `ensure`.
#[test]
fn a_lock_wait_is_cut_short_by_a_raise_a_close_and_the_programs_end() {
    let scratch = Scratch::new("interrupted");
    let output = run("flock_interrupted.rb", &[scratch.0.as_os_str()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "[RuntimeError, \"boom\"]\n\
         [IOError, \"stream closed in another thread\"]\n\
         ending\n"
    );
    assert_eq!(stderr, "ensure ran\n");
}

/// A blocking pipe, as an inherited one is: a megabyte written to it while
/// another thread reads it, which deadlocked holding the lock; a writer to
/// it the others run beside and a kill ends; and a writer and a reader its
/// `close` in another thread ends.
#[test]
fn a_blocking_pipe_is_read_and_written_outside_the_lock() {
    let output = run("blocking_pipe_threads.rb", &[]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "1000000\n\
         [:ticks, 10, \"sleep\"]\n\
         :joined\n\
         [IOError, \"stream closed in another thread\"]\n\
         [IOError, \"stream closed in another thread\"]\n"
    );
}

/// R3, inherited: a write to the standard output the program was given, a
/// blocking pipe nothing reads, ends on SIGTERM — alone, and with another
/// thread asleep. Its `write(2)` was restarted under `SA_RESTART` and never
/// reached the safe point where the handler runs.
fn a_write_to_a_full_standard_output_ends_on_sigterm(args: &[&str]) {
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(fixture("stdout_write_full.rb"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    // Held open and never read, so the pipe fills.
    let _unread = child.stdout.take().expect("piped stdout");
    let lines = lines(&mut child);
    assert_eq!(next_line(&lines, "the writer"), "writing");
    // Time for the write to fill the pipe and wait; a signal sent sooner
    // ends the program as surely.
    thread::sleep(Duration::from_millis(200));
    signal(&child, libc::SIGTERM);
    let status = wait_or_kill(
        &mut child,
        Duration::from_secs(10),
        "a write to a full standard output, sent SIGTERM,",
    );
    let rest: Vec<String> = lines.try_iter().collect();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {rest:?}");
    assert!(!rest.iter().any(|line| line == "never"), "stderr: {rest:?}");
}

#[test]
fn a_lone_write_to_a_full_standard_output_ends_on_sigterm() {
    a_write_to_a_full_standard_output_ends_on_sigterm(&[]);
}

#[test]
fn a_write_to_a_full_standard_output_beside_a_thread_ends_on_sigterm() {
    a_write_to_a_full_standard_output_ends_on_sigterm(&["thread"]);
}

/// A lone thread waiting in `flock` for a lock this test holds, which no
/// other thread can unblock: the signals Ruby handles end the wait. Ctrl-C
/// raises an `Interrupt` the fixture rescues; a trapped USR1's handler runs
/// and the wait goes on, until SIGTERM ends the program.
fn lone_lock_wait(mode: &str) -> (Child, Receiver<String>, File) {
    let scratch = Scratch::new(mode);
    let held = File::options()
        .write(true)
        .open(&scratch.0)
        .expect("the scratch file opens");
    // SAFETY: `flock` takes a descriptor of a File alive for the test.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(fixture("flock_signal_alone.rb"))
        .arg(&scratch.0)
        .arg(mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    let lines = lines(&mut child);
    assert_eq!(next_line(&lines, "the waiter"), "locking");
    // Time to enter `flock`; one sent sooner is met as surely.
    thread::sleep(Duration::from_millis(200));
    // The file can go: the lock is the open file description's.
    drop(scratch);
    (child, lines, held)
}

#[test]
fn a_lone_lock_wait_ends_on_ctrl_c() {
    let (mut child, lines, _held) = lone_lock_wait("int");
    signal(&child, libc::SIGINT);
    let status = wait_or_kill(
        &mut child,
        Duration::from_secs(10),
        "a lone flock, sent SIGINT,",
    );
    let rest: Vec<String> = lines.try_iter().collect();
    assert!(status.success(), "{status:?}, stderr: {rest:?}");
    assert_eq!(rest, ["interrupted Interrupt"]);
}

#[test]
fn a_lone_lock_wait_runs_a_trap_and_goes_on_waiting() {
    let (mut child, lines, _held) = lone_lock_wait("trap");
    signal(&child, libc::SIGUSR1);
    assert_eq!(next_line(&lines, "the trap"), "usr1 handled");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(child.try_wait().expect("the child can be waited for"), None);
    signal(&child, libc::SIGTERM);
    let status = wait_or_kill(
        &mut child,
        Duration::from_secs(10),
        "a lone flock, sent SIGTERM,",
    );
    let rest: Vec<String> = lines.try_iter().collect();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {rest:?}");
    assert!(rest.is_empty(), "stderr: {rest:?}");
}
