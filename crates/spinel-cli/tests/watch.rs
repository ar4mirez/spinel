//! `spinel run --watch` (#69, docs/cli.md): an edit restarts the program as a
//! fresh process, and the old one and its process group are gone; a path
//! `.gitignore` excludes, at the top or in a nested directory, restarts
//! nothing; a burst of saves within the 100 ms debounce restarts it once; a
//! program that ends on its own is started again by the next change; and
//! SIGINT ends the supervisor and its program together.
//!
//! Nothing here sleeps for a guessed time. The supervisor reports each restart
//! on stderr with the paths that caused it, and it handles events in the order
//! the file system delivered them. So a test that must show some change did
//! not restart the program makes that change, then a change that must, and
//! reads the restart line the second one produces: had the first counted, it
//! would be named there or in a line before it. Every wait is bounded.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// The supervisor's debounce. A burst written faster than this is one save.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// The longest any one expected line may take: a cold debug build on a busy
/// CI runner, not a timing the test depends on.
const PATIENCE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stream {
    Out,
    Err,
}

/// `spinel run --watch app.rb` in a fresh copy of `fixtures/watch`.
struct Watch {
    base: PathBuf,
    project: PathBuf,
    supervisor: Child,
    lines: Receiver<(Stream, String)>,
    /// Every line read so far, for assertions over the whole run and for
    /// failure messages.
    seen: Vec<(Stream, String)>,
    /// Every pid the program reported, killed at the end in case the
    /// supervisor failed to.
    pids: Vec<i32>,
}

impl Watch {
    fn start(name: &str, args: &[&str]) -> Watch {
        let base = std::env::temp_dir().join(format!("spinel-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let project = base.join("project");
        copy_dir(&fixture(), &project);
        // The user's global excludes file is read too; keep this machine's
        // out of the test.
        let home = base.join("home");
        fs::create_dir_all(&home).expect("home dir");
        let mut supervisor = Command::new(SPINEL)
            .arg("run")
            .arg("--watch")
            .arg("app.rb")
            .args(args)
            .current_dir(&project)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spinel runs");
        let (send, lines) = mpsc::channel();
        let read = |stream: Stream, pipe: Box<dyn Read + Send>| {
            let send = send.clone();
            thread::spawn(move || {
                for line in BufReader::new(pipe).lines() {
                    let Ok(line) = line else { break };
                    if send.send((stream, line)).is_err() {
                        break;
                    }
                }
            });
        };
        read(
            Stream::Out,
            Box::new(supervisor.stdout.take().expect("stdout")),
        );
        read(
            Stream::Err,
            Box::new(supervisor.stderr.take().expect("stderr")),
        );
        let mut watch = Watch {
            base,
            project,
            supervisor,
            lines,
            seen: Vec::new(),
            pids: Vec::new(),
        };
        watch.expect(Stream::Err, |l| l.starts_with("spinel: watching "));
        watch
    }

    /// Read lines until one on `stream` satisfies `wanted`, and return it.
    fn expect(&mut self, stream: Stream, wanted: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok((from, line)) => {
                    self.seen.push((from, line.clone()));
                    if from == stream && wanted(&line) {
                        return line;
                    }
                }
                Err(_) => panic!(
                    "no expected line on {stream:?} within {PATIENCE:?}; saw {:#?}",
                    self.seen
                ),
            }
        }
    }

    /// Wait for the program to start, and return the pids it printed: its
    /// own, then its forked child's when it has one.
    fn started(&mut self) -> Vec<i32> {
        let line = self.expect(Stream::Out, |l| l.starts_with("started "));
        let pids: Vec<i32> = line
            .split_whitespace()
            .skip(1)
            .map(|pid| pid.parse().expect("a pid"))
            .collect();
        self.pids.extend(&pids);
        pids
    }

    fn restart_lines(&self) -> Vec<&str> {
        self.seen
            .iter()
            .filter(|(stream, line)| {
                *stream == Stream::Err && line.starts_with("spinel: restarting")
            })
            .map(|(_, line)| line.as_str())
            .collect()
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.project.join(path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).expect("parent dir");
        }
        fs::write(&path, contents).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    fn signal(&self, signal: libc::c_int) {
        let pid = libc::pid_t::try_from(self.supervisor.id()).expect("a pid");
        // SAFETY: `kill` reads no memory; `pid` is our unreaped child.
        unsafe {
            libc::kill(pid, signal);
        }
    }

    /// Wait, bounded, for the supervisor to exit.
    fn exited(&mut self) -> ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.supervisor.try_wait().expect("waitable") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the supervisor did not exit; saw {:#?}",
                self.seen
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if self.supervisor.try_wait().ok().flatten().is_none() {
            self.signal(libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.supervisor.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            let _ = self.supervisor.kill();
            let _ = self.supervisor.wait();
        }
        for &pid in &self.pids {
            // SAFETY: as in `signal`. A pid already gone is ESRCH, ignored.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/watch")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("copy target");
    for entry in fs::read_dir(from).expect("fixture dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

/// Whether `pid` has gone, waiting a bounded while for it: a process the
/// supervisor ended is reaped by it, and one it orphaned by init, which may
/// take a moment.
fn gone(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // SAFETY: signal 0 checks for the process and sends nothing.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive || Instant::now() > deadline {
            return !alive;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_edit_restarts_the_program_as_a_fresh_process() {
    let mut watch = Watch::start("edit", &[]);
    let first = watch.started();

    watch.write("config/settings.rb", "SETTINGS = 2\n");
    let line = watch.expect(Stream::Err, |l| l.starts_with("spinel: restarting"));
    assert_eq!(line, "spinel: restarting: config/settings.rb changed");
    let second = watch.started();

    assert_ne!(first[0], second[0], "a new process, not a reload");
    assert!(
        gone(first[0]),
        "the old program {} is still running",
        first[0]
    );
    assert!(
        gone(first[1]),
        "the old program's child {} is still running: its process group was not ended",
        first[1]
    );
}

#[test]
fn ignored_paths_do_not_restart_the_program() {
    let mut watch = Watch::start("ignored", &[]);
    watch.started();

    // `tmp/` and `*.log` from the top `.gitignore`; `local.rb` from the one in
    // `config/`. Then a change that does restart it.
    watch.write("tmp/cache.rb", "1\n");
    watch.write("debug.log", "line\n");
    watch.write("config/debug.log", "line\n");
    watch.write("config/local.rb", "LOCAL = 1\n");
    watch.write("config/settings.rb", "SETTINGS = 2\n");
    watch.expect(Stream::Err, |l| l.contains("config/settings.rb"));
    watch.started();

    for line in watch.restart_lines() {
        assert!(
            !["tmp", "debug.log", "local.rb"]
                .iter()
                .any(|p| line.contains(p)),
            "an ignored path restarted the program: {line}"
        );
    }
}

#[test]
fn a_burst_of_saves_restarts_the_program_once() {
    let mut watch = Watch::start("burst", &[]);
    watch.started();

    // One save as editors make it: several writes to the file in a row.
    let began = Instant::now();
    for n in 0..10 {
        watch.write("burst.rb", &format!("BURST = {n}\n"));
    }
    let took = began.elapsed();
    assert!(
        took < DEBOUNCE,
        "ten small writes took {took:?}, longer than the debounce; nothing can be concluded"
    );
    watch.expect(Stream::Err, |l| l.contains("burst.rb"));
    watch.started();

    // Every event of the burst reached the supervisor before this one did, so
    // a second restart for the burst would be read before this line.
    watch.write("config/settings.rb", "SETTINGS = 2\n");
    watch.expect(Stream::Err, |l| l.contains("config/settings.rb"));
    let burst: Vec<&str> = watch
        .restart_lines()
        .into_iter()
        .filter(|l| l.contains("burst.rb"))
        .collect();
    assert_eq!(burst.len(), 1, "one save, one restart: {burst:#?}");
}

#[test]
fn a_program_that_ends_is_started_again_by_the_next_change() {
    let mut watch = Watch::start("ends", &["once"]);
    watch.started();
    watch.expect(Stream::Err, |l| {
        l == "spinel: the program exited with status 3; waiting for a change"
    });

    watch.write("config/settings.rb", "SETTINGS = 2\n");
    watch.expect(Stream::Err, |l| l.contains("config/settings.rb"));
    watch.started();
}

#[test]
fn sigint_ends_the_supervisor_and_the_program_with_it() {
    let mut watch = Watch::start("sigint", &[]);
    let program = watch.started();

    watch.signal(libc::SIGINT);
    let status = watch.exited();
    assert_eq!(
        status.code(),
        Some(130),
        "128 + SIGINT, as a shell reports it"
    );
    assert!(
        gone(program[0]),
        "the program {} outlived the supervisor",
        program[0]
    );
    assert!(
        gone(program[1]),
        "the program's child {} outlived the supervisor",
        program[1]
    );
}
