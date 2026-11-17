//! `spinel test --watch` (#97, docs/test-runner.md, "Watch"): `--watch`'s
//! supervisor (#69) running `spinel test`, each change mapped to the tests
//! to run again. A changed test file runs itself; a changed `lib/user.rb` or
//! `app/models/account.rb` runs the tests named for it, under `test/` and
//! `spec/` at any depth; any other change runs what failed last, by the
//! location the runner recorded for it, and nothing when nothing did; Enter
//! runs every test; and Ctrl-C ends the supervisor, the run and its workers,
//! and leaves no file of the supervisor's behind.
//!
//! Each test runs `spinel test --watch --reporter json` in a fresh copy of
//! `fixtures/test_watch`, with the test runner's minitest and rspec-core
//! stand-ins beside it (`tests/test_runner.rs` says why they stand in), and
//! reads what each run ran from its `done` events, one per file, up to its
//! `summary`. The supervisor reports on stderr what each change runs and
//! when each run has ended, so a test waits for a run to end before making
//! the next change, and a change that runs nothing says so. Every wait is
//! bounded.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// The longest any one expected line may take: a cold debug build on a busy
/// CI runner, not a timing the test depends on.
const PATIENCE: Duration = Duration::from_secs(60);

/// Every test file and spec file in the fixture.
const EVERY: [&str; 4] = [
    "spec/models/user_spec.rb",
    "test/answer_test.rb",
    "test/models/account_test.rb",
    "test/user_test.rb",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stream {
    Out,
    Err,
}

/// One run: the files it loaded, and the line the supervisor printed once
/// it had ended.
#[derive(Debug)]
struct Run {
    files: BTreeSet<String>,
    ended: String,
}

/// `spinel test --watch` in a fresh copy of `fixtures/test_watch`.
struct Watch {
    base: PathBuf,
    project: PathBuf,
    supervisor: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<(Stream, String)>,
    /// Every line read so far, for failure messages.
    seen: Vec<(Stream, String)>,
}

impl Watch {
    fn start(name: &str) -> Watch {
        let base =
            std::env::temp_dir().join(format!("spinel-test-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let project = base.join("project");
        copy_dir(&fixtures().join("test_watch"), &project);
        copy_dir(
            &fixtures().join("test_runner/stand_ins"),
            &base.join("stand_ins"),
        );
        // The user's global excludes file is read too; keep this machine's
        // out of the test.
        let home = base.join("home");
        fs::create_dir_all(&home).expect("home dir");
        // The supervisor's own files go here, to be seen gone at the end.
        fs::create_dir_all(base.join("tmp")).expect("tmp dir");
        let mut supervisor = Command::new(SPINEL)
            .args(["test", "--watch", "-j", "2", "--reporter", "json"])
            .current_dir(&project)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("TMPDIR", base.join("tmp"))
            .env("SPINEL_TEST_LOG", base.join("log"))
            .stdin(Stdio::piped())
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
        let stdin = supervisor.stdin.take();
        let mut watch = Watch {
            base,
            project,
            supervisor,
            stdin,
            lines,
            seen: Vec::new(),
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

    /// The next run, read to both its `summary` on stdout and the line the
    /// supervisor prints once it has ended.
    fn run(&mut self) -> Run {
        let mut files = BTreeSet::new();
        let mut summary = false;
        let mut ended = None;
        let deadline = Instant::now() + PATIENCE;
        while !summary || ended.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok((stream, line)) = self.lines.recv_timeout(left) else {
                panic!("no whole run within {PATIENCE:?}; saw {:#?}", self.seen);
            };
            self.seen.push((stream, line.clone()));
            match stream {
                Stream::Out => {
                    let Ok(event) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    match event["event"].as_str() {
                        Some("done") => {
                            files.insert(event["file"].as_str().expect("a file").to_owned());
                        }
                        Some("summary") => summary = true,
                        _ => {}
                    }
                }
                Stream::Err if line.starts_with("spinel: the tests ") => ended = Some(line),
                Stream::Err => {}
            }
        }
        Run {
            files,
            ended: ended.expect("an ended line"),
        }
    }

    /// The line naming `changed` and what it runs.
    fn changed(&mut self, changed: &str) -> String {
        let prefix = format!("spinel: {changed} changed; ");
        self.expect(Stream::Err, |l| l.starts_with(&prefix))
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.project.join(path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).expect("parent dir");
        }
        fs::write(&path, contents).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    /// Save `path` again with what it holds.
    fn touch(&self, path: &str) {
        let contents = fs::read_to_string(self.project.join(path)).expect("a fixture file");
        self.write(path, &contents);
    }

    fn enter(&mut self) {
        let stdin = self.stdin.as_mut().expect("stdin");
        stdin.write_all(b"\n").expect("a line to the supervisor");
        stdin.flush().expect("flushed");
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

    /// No line so far has named the runner's own writes under `.spinel/`.
    fn spinel_dir_never_counted(&self) {
        for (stream, line) in &self.seen {
            assert!(
                !(*stream == Stream::Err && line.contains(".spinel")),
                "a write under .spinel/ counted as a change: {line}"
            );
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
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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

fn set(files: &[&str]) -> BTreeSet<String> {
    files.iter().map(|&file| file.to_owned()).collect()
}

/// Whether `pid` has gone, waiting a bounded while for it.
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
fn the_first_run_is_every_test_and_a_changed_test_file_runs_itself() {
    let mut watch = Watch::start("itself");
    let first = watch.run();
    assert_eq!(first.files, set(&EVERY));
    assert_eq!(
        first.ended,
        "spinel: the tests exited with status 1; waiting for a change, or Enter to run every test",
        "test/answer_test.rb fails"
    );

    watch.touch("test/user_test.rb");
    assert_eq!(
        watch.changed("test/user_test.rb"),
        "spinel: test/user_test.rb changed; running test/user_test.rb"
    );
    let run = watch.run();
    assert_eq!(run.files, set(&["test/user_test.rb"]));
    assert!(run.ended.contains("exited with status 0"), "{run:?}");

    // A spec file, nested, the same.
    watch.touch("spec/models/user_spec.rb");
    watch.changed("spec/models/user_spec.rb");
    assert_eq!(watch.run().files, set(&["spec/models/user_spec.rb"]));
    watch.spinel_dir_never_counted();
}

#[test]
fn a_changed_source_file_runs_the_tests_named_for_it() {
    let mut watch = Watch::start("named");
    watch.run();

    // `lib/user.rb`: `test/**/user_test.rb` and `spec/**/user_spec.rb`.
    watch.write(
        "lib/user.rb",
        "class User\n  def self.name = \"ada\"\nend\n",
    );
    assert_eq!(
        watch.changed("lib/user.rb"),
        "spinel: lib/user.rb changed; running spec/models/user_spec.rb, test/user_test.rb"
    );
    assert_eq!(
        watch.run().files,
        set(&["spec/models/user_spec.rb", "test/user_test.rb"])
    );

    // `app/models/account.rb`: `test/models/account_test.rb`, found at its
    // own depth by name alone.
    watch.touch("app/models/account.rb");
    watch.changed("app/models/account.rb");
    assert_eq!(watch.run().files, set(&["test/models/account_test.rb"]));
    watch.spinel_dir_never_counted();
}

#[test]
fn any_other_change_runs_what_failed_last() {
    let mut watch = Watch::start("failed");
    let first = watch.run();
    assert!(first.ended.contains("status 1"), "{first:?}");

    // `test/answer_test.rb` failed at its test's line, so that line runs.
    watch.touch("config/answer.rb");
    assert_eq!(
        watch.changed("config/answer.rb"),
        "spinel: config/answer.rb changed; running test/answer_test.rb:7"
    );
    let run = watch.run();
    assert_eq!(run.files, set(&["test/answer_test.rb"]));
    assert!(run.ended.contains("status 1"), "{run:?}");

    // A source file no test is named for is any other change too. The run
    // of `lib/user.rb`'s tests between leaves what failed as it was.
    watch.touch("lib/user.rb");
    watch.changed("lib/user.rb");
    watch.run();
    watch.touch("lib/helper.rb");
    assert_eq!(
        watch.changed("lib/helper.rb"),
        "spinel: lib/helper.rb changed; running test/answer_test.rb:7"
    );
    watch.run();

    // Fixed, it passes, and then nothing failed last.
    watch.write("config/answer.rb", "ANSWER = 2\n");
    watch.changed("config/answer.rb");
    let run = watch.run();
    assert_eq!(run.files, set(&["test/answer_test.rb"]));
    assert!(run.ended.contains("status 0"), "{run:?}");
    watch.touch("config/answer.rb");
    assert_eq!(
        watch.changed("config/answer.rb"),
        "spinel: config/answer.rb changed; nothing failed last time, so no test runs \
         (Enter runs every test)"
    );
    // Nothing ran for it: the next run is the next change's alone.
    watch.touch("test/user_test.rb");
    watch.changed("test/user_test.rb");
    assert_eq!(watch.run().files, set(&["test/user_test.rb"]));
    watch.spinel_dir_never_counted();
}

#[test]
fn enter_runs_every_test() {
    let mut watch = Watch::start("enter");
    watch.run();
    watch.touch("test/user_test.rb");
    watch.changed("test/user_test.rb");
    assert_eq!(watch.run().files, set(&["test/user_test.rb"]));

    watch.enter();
    watch.expect(Stream::Err, |l| l == "spinel: running every test");
    assert_eq!(watch.run().files, set(&EVERY));
}

#[test]
fn ctrl_c_ends_the_run_and_its_workers_and_leaves_nothing_behind() {
    let mut watch = Watch::start("sigint");
    watch.run();

    // A test file that never finishes loading, which runs itself.
    watch.write(
        "test/hang_test.rb",
        "require \"test_helper\"\nloaded(__FILE__)\nsleep\n",
    );
    watch.changed("test/hang_test.rb");
    let log = watch.base.join("log");
    let deadline = Instant::now() + PATIENCE;
    let worker = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        if let Some(pid) = text
            .lines()
            .find_map(|line| line.strip_prefix("hang_test.rb "))
        {
            break pid.trim().parse::<i32>().expect("a pid");
        }
        assert!(
            Instant::now() < deadline,
            "hang_test.rb never loaded; saw {:#?}",
            watch.seen
        );
        thread::sleep(Duration::from_millis(20));
    };

    watch.signal(libc::SIGINT);
    let status = watch.exited();
    assert_eq!(
        status.code(),
        Some(130),
        "128 + SIGINT, as a shell reports it"
    );
    assert!(gone(worker), "the worker {worker} outlived the supervisor");
    let left: Vec<_> = fs::read_dir(watch.base.join("tmp"))
        .expect("tmp dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(left.is_empty(), "the supervisor left {left:?} behind");
}
