//! `spinel test`'s parent and workers (#92, docs/test-runner.md,
//! "Execution"), on the built `spinel`: the helpers are required once, in
//! the parent, the workers are forked after them, `-j` sets how many, and a
//! worker that dies is reported with the file it was running.
//!
//! Each fixture under `tests/fixtures/test_runner/` logs to the file
//! `SPINEL_TEST_LOG` names, one line per helper load and per test file, with
//! the pid that did it. Worker counts stay at 3 or under.
//!
//! Spec files run after the Minitest files (#837), each phase in a process
//! of its own forked from the runner, the RSpec one forking its workers
//! after `rspec/core` and `spec/spec_helper.rb`, each worker running
//! its files' examples through `RSpec::Core::Runner.run`. rspec-core is not
//! installed here, so the RSpec fixtures carry a stand-in for the calls the
//! runner makes, in their `lib/rspec/core.rb`, which logs each run; the real
//! gem's run is checked by hand against CRuby in #837's pull request.
//!
//! Each worker writes one JSON line per event (#93): `done` for each file,
//! and, from the Minitest reporter and the RSpec formatter
//! `shims/test_worker.rb` registers, one per test or example. With
//! `--reporter json` (#95) the parent prints each line it reads as one JSON
//! object on stdout, which is what these tests read. minitest is not
//! installed here either, so the
//! fixtures that run tests carry a stand-in for the part of its API a run
//! goes through, in `stand_ins/minitest.rb`; the real gems' events are
//! checked by hand against CRuby in #93's pull request.
//!
//! The runner prints the report itself (#94): a character per result as it
//! comes, then each failure and error with its `file:line` and backtrace,
//! gem frames folded, a summary line, and a `spinel test file:line` command
//! per failure, which these tests paste into a shell and run. Each
//! framework's own report is left out. `--reporter junit` (#95) prints JUnit
//! XML instead, checked against the schema CI systems read
//! (`tests/fixtures/test_runner/junit.xsd`).
//!
//! A run keeps each file's time in `.spinel/test_timings.json` where it
//! runs and hands the files out longest first next time (#96), so each run
//! of a fixture here is in a copy of its own, which starts with no times
//! kept and is removed after it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test_runner")
}

fn fixture(name: &str) -> PathBuf {
    fixtures().join(name)
}

/// A copy of a fixture, with the stand-ins its `lib/` requires beside it,
/// in a fresh directory that is removed when the copy is dropped.
struct Copy {
    root: PathBuf,
    dir: PathBuf,
}

impl Drop for Copy {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

/// `dir` copied, when it is a fixture; a directory of a test's own is run
/// in as it is.
fn copy_of(dir: &Path) -> Option<Copy> {
    let name = dir.strip_prefix(fixtures()).ok()?;
    // The first free name is claimed by creating it, which only one caller
    // can do, so copies made at once by tests on other threads stay apart.
    let root = (0..)
        .map(|n| {
            std::env::temp_dir().join(format!(
                "spinel-test-runner-copy-{}-{n}",
                std::process::id()
            ))
        })
        .find(|root| match std::fs::create_dir(root) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => panic!("a fresh directory to copy into: {e}"),
        })
        .expect("a free name");
    copy_tree(&fixtures().join("stand_ins"), &root.join("stand_ins"));
    copy_tree(dir, &root.join(name));
    Some(Copy {
        dir: root.join(name),
        root,
    })
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory to copy into");
    for entry in std::fs::read_dir(from).expect("a fixture directory") {
        let entry = entry.expect("a fixture entry");
        let path = entry.path();
        if path.is_dir() {
            copy_tree(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).expect("a fixture file copied");
        }
    }
}

/// A log file of its own for one test, in a fresh directory.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("spinel-test-runner-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// `spinel test` with `args` in `dir`, or in a copy of it when it is a
/// fixture, logging to `log`.
fn spinel_test(dir: &Path, args: &[&str], log: &Path, env: &[(&str, &str)]) -> Output {
    let copy = copy_of(dir);
    let dir = copy.as_ref().map_or(dir, |copy| copy.dir.as_path());
    Command::new(SPINEL)
        .arg("test")
        .args(args)
        .current_dir(dir)
        .env("SPINEL_TEST_LOG", log)
        .envs(env.iter().copied())
        .output()
        .expect("spinel runs")
}

fn lines(log: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| line.split(' ').map(str::to_owned).collect())
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Runs the `boot` fixture on `jobs` workers, with every file waiting until
/// that many workers have each started one, and checks what the log shows:
/// the helper loaded once, in the parent, and every file run in a worker
/// forked from it. Returns the workers' pids.
fn boot_on(jobs: usize) -> BTreeSet<String> {
    let dir = scratch(&format!("boot-{jobs}"));
    let log = dir.join("log");
    let jobs_arg = jobs.to_string();
    let output = spinel_test(
        &fixture("boot"),
        &["-j", &jobs_arg],
        &log,
        &[("SPINEL_TEST_BARRIER", &jobs_arg)],
    );
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let helpers: Vec<&Vec<String>> = lines.iter().filter(|l| l[0].ends_with("_helper")).collect();
    let loaded: Vec<&str> = helpers.iter().map(|l| l[0].as_str()).collect();
    assert_eq!(loaded, ["test_helper"], "loaded once: {lines:?}");
    let parent = &helpers[0][1];

    let files: Vec<&Vec<String>> = lines.iter().filter(|l| l[0] == "file").collect();
    let names: BTreeSet<&str> = files.iter().map(|l| l[4].as_str()).collect();
    assert_eq!(
        names,
        BTreeSet::from(["a_test.rb", "b_test.rb", "c_test.rb", "d_test.rb"]),
        "every file ran once: {lines:?}"
    );
    assert_eq!(files.len(), 4, "and only once: {lines:?}");
    for file in &files {
        assert_ne!(
            &file[1], parent,
            "files run in workers, not the parent: {lines:?}"
        );
        // What the helper defined in the parent, as the worker saw it: the
        // worker inherited it through `fork` rather than booting again.
        assert_eq!(&file[2], parent, "booted by the parent: {lines:?}");
        assert_eq!(file[3], "hello", "`lib/` is on the load path: {lines:?}");
    }
    files.iter().map(|l| l[1].clone()).collect()
}

#[test]
fn the_helpers_are_loaded_once_and_two_workers_share_the_files() {
    let workers = boot_on(2);
    assert_eq!(workers.len(), 2, "-j 2 runs two workers: {workers:?}");
}

#[test]
fn dash_j_sets_how_many_workers_run_at_once() {
    // Every file waits for three workers to have started one, so the run
    // finishes only if three run at once; three distinct pids, and no more,
    // is `-j 3`.
    let workers = boot_on(3);
    assert_eq!(workers.len(), 3, "-j 3 runs three workers: {workers:?}");
}

#[test]
fn one_worker_runs_every_file_in_turn() {
    let workers = boot_on(1);
    assert_eq!(workers.len(), 1, "-j 1 runs one worker: {workers:?}");
}

#[test]
fn a_worker_that_dies_is_reported_with_its_file_and_the_rest_still_run() {
    let dir = scratch("crash");
    let log = dir.join("log");
    let output = spinel_test(&fixture("crash"), &["-j", "2"], &log, &[]);
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(1), "stderr: {err}");
    assert!(
        err.contains("was killed by SIGKILL while running test/b_killed_test.rb"),
        "a worker killed mid-file is reported with the file: {err}"
    );
    assert!(
        err.contains("exited with status 3 while running test/d_exit_test.rb"),
        "a worker that exits mid-file is reported with the file: {err}"
    );
    assert!(
        err.contains("test/f_raise_test.rb: ArgumentError: broken on load"),
        "a file whose loading raised is reported: {err}"
    );
    // Each file that did not finish is printed to run again.
    let out = String::from_utf8_lossy(&output.stdout);
    for file in [
        "test/b_killed_test.rb",
        "test/d_exit_test.rb",
        "test/f_raise_test.rb",
    ] {
        assert!(
            out.lines()
                .any(|line| line == format!("spinel test {file}")),
            "{out}"
        );
    }
    assert!(out.contains(", 3 problems\n"), "{out}");
    // The workers forked in place of the dead ones run what was waiting.
    let ran: BTreeSet<&str> = lines.iter().map(|l| l[2].as_str()).collect();
    assert_eq!(
        ran,
        BTreeSet::from(["a_test.rb", "c_test.rb", "e_test.rb", "g_test.rb"]),
        "stderr: {err}"
    );
}

#[test]
fn a_helpers_at_exit_runs_in_each_worker_and_not_in_the_parent() {
    let dir = scratch("at-exit");
    let log = dir.join("log");
    let output = spinel_test(&fixture("at_exit"), &["-j", "1"], &log, &[]);
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let worker = &lines[0][1];
    let at_exit: Vec<&Vec<String>> = lines.iter().filter(|l| l[0] == "at_exit").collect();
    assert_eq!(at_exit.len(), 1, "once, in the one worker: {lines:?}");
    assert_eq!(&at_exit[0][1], worker, "{lines:?}");
    assert_eq!(
        lines.last(),
        Some(at_exit[0]),
        "after its last file: {lines:?}"
    );
}

#[test]
fn a_worker_failing_after_its_last_file_fails_the_run() {
    for (mode, ending) in [
        ("fail", "exited with status 1"),
        ("kill", "was killed by SIGKILL"),
    ] {
        let dir = scratch(&format!("at-exit-{mode}"));
        let log = dir.join("log");
        let output = spinel_test(
            &fixture("at_exit"),
            &["-j", "1"],
            &log,
            &[("SPINEL_TEST_AT_EXIT", mode)],
        );
        std::fs::remove_dir_all(&dir).ok();
        let err = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{mode}: {err}");
        assert!(
            err.contains(&format!(
                "{ending} after its last file; it ran test/a_test.rb, test/b_test.rb"
            )),
            "{mode}: {err}"
        );
    }
}

/// The pids of the log lines that start with `kind`, by their third word.
fn by_name<'a>(lines: &'a [Vec<String>], kind: &str) -> Vec<(&'a str, &'a str)> {
    lines
        .iter()
        .filter(|l| l[0] == kind)
        .map(|l| (l[2].as_str(), l[1].as_str()))
        .collect()
}

#[test]
fn spec_files_run_through_rspec_in_workers_after_the_tests() {
    let dir = scratch("rspec");
    let log = dir.join("log");
    let output = spinel_test(&fixture("rspec"), &["-j", "2"], &log, &[]);
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);

    // `d_spec.rb`'s example fails, so its worker exits 1, and so does the run.
    assert_eq!(output.status.code(), Some(1), "stderr: {err}");
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains(
            "  1) Failure: d fails [spec/models/d_spec.rb:4]\n     RuntimeError: a failing example fails the run\n"
        ),
        "{out}"
    );
    assert!(
        !out.contains("examples,") && !err.contains("examples,"),
        "RSpec's own report is left out: {out}{err}"
    );
    let helpers: Vec<&Vec<String>> = lines.iter().filter(|l| l[0].ends_with("_helper")).collect();
    let loaded: Vec<&str> = helpers.iter().map(|l| l[0].as_str()).collect();
    assert_eq!(
        loaded,
        ["test_helper", "spec_helper"],
        "each once: {lines:?}"
    );
    let tests_parent = helpers[0][1].as_str();
    let parent = helpers[1][1].as_str();
    assert_ne!(
        tests_parent, parent,
        "each phase in a process of its own: {lines:?}"
    );

    let tests = by_name(&lines, "file");
    let test_workers: BTreeSet<&str> = tests.iter().map(|(_, pid)| *pid).collect();
    for pid in &test_workers {
        assert_ne!(*pid, tests_parent, "in a worker: {lines:?}");
        assert_ne!(*pid, parent, "in a worker: {lines:?}");
    }
    let names: BTreeSet<&str> = tests.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        BTreeSet::from(["a_test.rb", "b_test.rb"]),
        "{lines:?}"
    );

    // The Minitest helper's `at_exit` runs in each Minitest worker, and in no
    // process that ran spec files or examples, nor in either phase's process.
    let at_exit: BTreeSet<&str> = lines
        .iter()
        .filter(|l| l[0] == "at_exit")
        .map(|l| l[1].as_str())
        .collect();
    assert_eq!(
        at_exit, test_workers,
        "only the Minitest workers: {lines:?}"
    );

    let specs = by_name(&lines, "spec");
    let names: BTreeSet<&str> = specs.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        BTreeSet::from(["c_spec.rb", "d_spec.rb"]),
        "{lines:?}"
    );
    assert_eq!(specs.len(), 2, "each once: {lines:?}");
    for line in lines.iter().filter(|l| l[0] == "spec") {
        assert_ne!(line[1], parent, "in a worker: {lines:?}");
        // What the helper defined in the parent, as the worker saw it.
        assert_eq!(
            line[3], parent,
            "the helper booted in the parent: {lines:?}"
        );
        assert!(!at_exit.contains(line[1].as_str()), "{lines:?}");
    }

    // Each worker that loaded spec files ran their examples, and only theirs.
    let runs: Vec<&Vec<String>> = lines.iter().filter(|l| l[0] == "run").collect();
    let mut ran = BTreeSet::new();
    for run in &runs {
        let pid = run[1].as_str();
        let mine: BTreeSet<&str> = specs
            .iter()
            .filter(|(_, p)| *p == pid)
            .map(|(name, _)| *name)
            .collect();
        assert!(
            !mine.is_empty(),
            "a run in a worker that loaded specs: {lines:?}"
        );
        assert!(!at_exit.contains(pid), "{lines:?}");
        let examples: BTreeSet<&str> = run[2].split(',').collect();
        let expected: BTreeSet<&str> = mine
            .iter()
            .map(|name| {
                if *name == "c_spec.rb" {
                    "c_passes"
                } else {
                    "d_fails"
                }
            })
            .collect();
        assert_eq!(examples, expected, "{lines:?}");
        ran.extend(examples);
    }
    assert_eq!(ran, BTreeSet::from(["c_passes", "d_fails"]), "{lines:?}");
}

#[test]
fn a_passing_spec_only_project_passes() {
    let dir = scratch("rspec-only");
    let log = dir.join("log");
    let output = spinel_test(&fixture("rspec_only"), &["-j", "1"], &log, &[]);
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);

    assert!(output.status.success(), "stderr: {err}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "..\n\n2 tests, 0 failures, 0 errors, 0 skips\n",
        "one worker, one run of both files' examples, reported by the runner"
    );
    let parent = &lines[0];
    assert_eq!(parent[0], "spec_helper", "{lines:?}");
    let runs: Vec<&Vec<String>> = lines.iter().filter(|l| l[0] == "run").collect();
    assert_eq!(runs.len(), 1, "{lines:?}");
    assert_ne!(runs[0][1], parent[1], "in a worker: {lines:?}");
    assert_eq!(runs[0][2], "a_passes,b_passes", "in file order: {lines:?}");
    assert_eq!(runs[0][3], "0", "{lines:?}");
}

#[test]
fn spec_files_without_rspec_installed_fail_the_run_unloaded() {
    let dir = scratch("rspec-missing");
    let log = dir.join("log");
    // An empty gem home, so no rspec-core installed elsewhere is found.
    let gems = dir.join("gems");
    std::fs::create_dir_all(&gems).expect("a gem home");
    let gems = gems.to_str().expect("a UTF-8 path");
    let output = spinel_test(
        &fixture("rspec_missing"),
        &["-j", "1"],
        &log,
        &[("GEM_HOME", gems), ("GEM_PATH", gems)],
    );
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(1), "stderr: {err}");
    assert!(
        err.contains("1 RSpec file under spec/ not run: cannot load such file -- rspec/core"),
        "{err}"
    );
    assert!(lines.is_empty(), "nothing is loaded: {lines:?}");
}

#[test]
fn no_test_files_is_an_error() {
    let dir = scratch("empty");
    let output = spinel_test(&dir, &["-j", "1"], &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no test files"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn dash_j_must_be_at_least_one() {
    let dir = scratch("zero");
    let output = spinel_test(&dir, &["-j", "0"], &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

/// Each line of `--reporter json`'s stdout, parsed: a line that is not one
/// JSON object fails here.
fn json_lines(output: &Output) -> Vec<Value> {
    let text = String::from_utf8(output.stdout.clone()).expect("events are UTF-8");
    assert!(
        text.is_empty() || text.ends_with('\n'),
        "every event ends its line"
    );
    text.lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("not one JSON event ({err}): {line:.200}"));
            assert!(event.is_object(), "{line:.200}");
            event
        })
        .collect()
}

/// `spinel test --reporter json` with `args` on the fixture `name`, every
/// event it printed but the last, and the last, the `summary`.
fn events_of(name: &str, args: &[&str]) -> (Output, Vec<Value>, Value) {
    let dir = scratch(&format!("events-{name}"));
    let args: Vec<&str> = ["--reporter", "json"].iter().chain(args).copied().collect();
    let output = spinel_test(&fixture(name), &args, &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    let mut events = json_lines(&output);
    let summary = events.pop().unwrap_or_default();
    assert_eq!(summary["event"], "summary", "last: {summary}");
    (output, events, summary)
}

/// A result event without its time, which varies, after checking it is a
/// time and the event has exactly the documented fields.
fn without_ms(event: &Value) -> Value {
    let keys: BTreeSet<&str> = event
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "backtrace",
            "event",
            "file",
            "line",
            "message",
            "ms",
            "name",
            "rerun"
        ]),
        "{event}"
    );
    let mut event = event.clone();
    let ms = event
        .as_object_mut()
        .and_then(|fields| fields.remove("ms"))
        .expect("ms");
    assert!(ms.as_f64().is_some_and(|ms| ms >= 0.0), "ms: {ms}");
    event
}

fn is_result(event: &Value) -> bool {
    matches!(
        event["event"].as_str(),
        Some("pass" | "fail" | "error" | "skip")
    )
}

#[test]
fn minitest_and_rspec_each_report_every_test_as_one_event_line() {
    let (output, events, summary) = events_of("events", &["-j", "2"]);
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "a test failed: {err}");

    let done: Vec<&Value> = events.iter().filter(|e| e["event"] == "done").collect();
    assert_eq!(
        done,
        [
            &json!({"event": "done", "file": "test/results_test.rb", "error": null}),
            &json!({"event": "done", "file": "spec/results_spec.rb", "error": null}),
        ],
        "{err}"
    );
    let results: Vec<&Value> = events.iter().filter(|e| is_result(e)).collect();
    assert_eq!(results.len() + done.len(), events.len(), "{events:?}");

    // Minitest's, in the order it ran them: each by its first failure, as
    // its summary counts it.
    let mut by_name: BTreeMap<String, Value> = results
        .iter()
        .map(|e| {
            (
                e["name"].as_str().expect("a name").to_owned(),
                without_ms(e),
            )
        })
        .collect();
    let mut take = |name: &str| {
        by_name
            .remove(name)
            .unwrap_or_else(|| panic!("{name}: {events:?}"))
    };
    let file = "test/results_test.rb";
    assert_eq!(
        take("ResultsTest#test_passes"),
        json!({"event": "pass", "file": file, "line": 4, "name": "ResultsTest#test_passes",
               "message": null, "backtrace": [], "rerun": format!("{file}:4")})
    );
    let fails = take("ResultsTest#test_fails");
    assert_eq!(fails["event"], "fail", "{fails}");
    assert_eq!(fails["line"], 8, "{fails}");
    assert_eq!(fails["rerun"], "test/results_test.rb:8", "{fails}");
    assert_eq!(fails["message"], "Expected: 3\n  Actual: 2", "{fails}");
    // Minitest's own frames filtered out, the test's first, relative.
    assert_eq!(
        fails["backtrace"][0], "test/results_test.rb:9:in 'ResultsTest#test_fails'",
        "{fails}"
    );
    let errors = take("ResultsTest#test_errors");
    assert_eq!(errors["event"], "error", "{errors}");
    assert_eq!(errors["line"], 12, "{errors}");
    assert_eq!(errors["rerun"], "test/results_test.rb:12", "{errors}");
    assert_eq!(errors["message"], "ArgumentError: broken", "{errors}");
    assert_eq!(
        errors["backtrace"][0], "test/results_test.rb:13:in 'ResultsTest#test_errors'",
        "{errors}"
    );
    assert_eq!(
        take("ResultsTest#test_skips"),
        json!({"event": "skip", "file": file, "line": 16, "name": "ResultsTest#test_skips",
               "message": "not yet", "backtrace": [], "rerun": format!("{file}:16")})
    );

    // RSpec's: an exception in an example is a failure, as `rspec` counts
    // it, and a pending or skipped example a skip.
    let file = "spec/results_spec.rb";
    let frame = |line: u32| format!("{file}:{line}:in 'block (2 levels) in <top (required)>'");
    assert_eq!(
        take("results passes"),
        json!({"event": "pass", "file": file, "line": 4, "name": "results passes",
               "message": null, "backtrace": [], "rerun": format!("{file}:4")})
    );
    assert_eq!(
        take("results fails"),
        json!({"event": "fail", "file": file, "line": 5, "name": "results fails",
               "message": "\nexpected: 3\n     got: 2\n", "backtrace": [frame(5)],
               "rerun": format!("{file}:5")})
    );
    assert_eq!(
        take("results raises"),
        json!({"event": "fail", "file": file, "line": 6, "name": "results raises",
               "message": "ArgumentError: broken", "backtrace": [frame(6)],
               "rerun": format!("{file}:6")})
    );
    assert_eq!(
        take("results is pending"),
        json!({"event": "skip", "file": file, "line": 7, "name": "results is pending",
               "message": "not yet", "backtrace": [], "rerun": format!("{file}:7")})
    );
    assert_eq!(
        take("results is skipped"),
        json!({"event": "skip", "file": file, "line": 11, "name": "results is skipped",
               "message": "Temporarily skipped with xit", "backtrace": [],
               "rerun": format!("{file}:11")})
    );
    assert_eq!(
        take("results passes while pending"),
        json!({"event": "fail", "file": file, "line": 12, "name": "results passes while pending",
               "message": "Expected pending 'not yet' to fail. No error was raised.",
               "backtrace": [], "rerun": format!("{file}:12")})
    );
    assert!(by_name.is_empty(), "no other results: {by_name:?}");

    assert_eq!(
        summary,
        json!({"event": "summary", "tests": 10, "failures": 4, "errors": 1, "skips": 3,
               "problems": 0})
    );

    // The runner's report, in place of each framework's own.
    let dir = scratch("events-report");
    let output = spinel_test(&fixture("events"), &["-j", "2"], &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    let out = stdout(&output);
    assert!(
        out.contains("\n10 tests, 4 failures, 1 error, 3 skips\n"),
        "{out}"
    );
    for output in [out, err] {
        assert!(!output.contains("runs,"), "Minitest's left out: {output}");
        assert!(!output.contains("examples,"), "RSpec's left out: {output}");
    }
}

#[test]
fn many_events_from_several_workers_each_arrive_as_a_whole_line() {
    // Six Minitest files and three spec files of a hundred tests each, on
    // three workers at once; every tenth is skipped with a message longer
    // than a pipe holds and than the parent reads at once, which JSON
    // escapes and whose characters are several bytes each.
    let (output, events, _) = events_of("many", &["-j", "3"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "", "no line the parent could not read");

    let long = "é\"\\\n\t".repeat(20_000);
    let mut names = BTreeSet::new();
    let mut per_file: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for event in events.iter().filter(|e| is_result(e)) {
        let event = without_ms(event);
        let name = event["name"].as_str().expect("a name").to_owned();
        assert!(names.insert(name.clone()), "{name} reported once");
        let file = event["file"].as_str().expect("a file").to_owned();
        let counts = per_file.entry(file).or_default();
        match event["event"].as_str() {
            Some("pass") => counts.0 += 1,
            Some("skip") => {
                assert!(
                    event["message"] == long.as_str(),
                    "{name}: the long message whole"
                );
                counts.1 += 1;
            }
            _ => panic!("{event:.200}"),
        }
    }
    let files: Vec<&str> = per_file.keys().map(String::as_str).collect();
    assert_eq!(
        files,
        [
            "spec/a_spec.rb",
            "spec/b_spec.rb",
            "spec/c_spec.rb",
            "test/a_test.rb",
            "test/b_test.rb",
            "test/c_test.rb",
            "test/d_test.rb",
            "test/e_test.rb",
            "test/f_test.rb",
        ]
    );
    for (file, counts) in &per_file {
        assert_eq!(*counts, (90, 10), "{file}");
    }
    let done = events.iter().filter(|e| e["event"] == "done").count();
    assert_eq!(done, 9, "a `done` per file");
}

/// What `spinel test` printed on stdout.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The commands under "Re-run each failure:", as printed.
fn reruns(out: &str) -> Vec<String> {
    let (_, after) = out
        .split_once("\nRe-run each failure:\n\n")
        .unwrap_or_else(|| panic!("no re-run commands: {out}"));
    after.lines().map(str::to_owned).collect()
}

#[test]
fn the_report_prints_progress_then_each_failure_then_a_summary() {
    let dir = scratch("report");
    let output = spinel_test(&fixture("report"), &["-j", "2"], &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "a test failed: {out}");
    assert_eq!(stderr(&output), "", "no problems");

    // A character per result: the Minitest file's, then the RSpec file's,
    // each as its framework's progress prints it.
    let (progress, rest) = out.split_once('\n').expect("a progress line");
    let mut minitest: Vec<char> = progress[..5].chars().collect();
    minitest.sort_unstable();
    assert_eq!(minitest, ['.', 'E', 'F', 'F', 'S'], "{out}");
    let mut rspec: Vec<char> = progress[5..].chars().collect();
    rspec.sort_unstable();
    assert_eq!(rspec, ['*', '.', 'F'], "{out}");

    // Each failure and error, in order of where its test is, with the
    // `file:line` its test is defined at and its backtrace.
    assert!(
        rest.starts_with(
            "\n  1) Failure: ReportTest#test_fails [test/report_test.rb:8]\n\
             \x20    Expected: 3\n\
             \x20      Actual: 2\n\
             \x20    # test/report_test.rb:9:in 'ReportTest#test_fails'\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "\n  2) Failure: ReportTest#test_fails_through_a_gem [test/report_test.rb:12]\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "\n  3) Error: ReportTest#test_errors [test/report_test.rb:18]\n\
             \x20    ArgumentError: broken\n\
             \x20    # test/report_test.rb:19:in 'ReportTest#test_errors'\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "\n  4) Failure: report fails [spec/report_spec.rb:6]\n\
             \x20    expected: 3\n\
             \x20         got: 2\n\
             \x20    # spec/report_spec.rb:7:in 'block (2 levels) in <top (required)>'\n"
        ),
        "{out}"
    );
    assert!(!out.contains("  5) "), "no other failures: {out}");
    assert!(
        out.contains("\n8 tests, 3 failures, 1 error, 2 skips\n"),
        "{out}"
    );
    assert_eq!(
        reruns(&out),
        [
            "spinel test test/report_test.rb:8 # ReportTest#test_fails",
            "spinel test test/report_test.rb:12 # ReportTest#test_fails_through_a_gem",
            "spinel test test/report_test.rb:18 # ReportTest#test_errors",
            "spinel test spec/report_spec.rb:6 # report fails",
        ],
        "{out}"
    );
}

#[test]
fn gem_frames_are_folded_out_of_a_backtrace() {
    let dir = scratch("report-gem");
    let output = spinel_test(
        &fixture("report"),
        &["-j", "1", "test/report_test.rb:12"],
        &dir.join("log"),
        &[],
    );
    std::fs::remove_dir_all(&dir).ok();
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}");
    // The test's block, called back from three frames of the gem, then the
    // test's own frame: the gem's three folded to one line between them.
    assert!(
        out.contains(
            "     # test/report_test.rb:14:in 'block in ReportTest#test_fails_through_a_gem'\n\
             \x20    # (3 gem frames)\n\
             \x20    # test/report_test.rb:13:in 'ReportTest#test_fails_through_a_gem'\n"
        ),
        "{out}"
    );
    assert!(!out.contains("checker"), "no gem frame printed: {out}");
}

#[test]
fn the_exit_status_is_zero_when_nothing_failed() {
    let dir = scratch("report-pass");
    // The skipped test and the passing example only.
    let output = spinel_test(
        &fixture("report"),
        &["-j", "2", "test/report_test.rb:22", "spec/report_spec.rb:4"],
        &dir.join("log"),
        &[],
    );
    std::fs::remove_dir_all(&dir).ok();
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{out}{}", stderr(&output));
    assert!(
        out.ends_with("\n\n2 tests, 0 failures, 0 errors, 1 skip\n"),
        "{out}"
    );
    assert!(!out.contains("Re-run"), "nothing to re-run: {out}");
}

/// `command` pasted into `sh` in a copy of `fixture`, with `spinel` on the
/// PATH: its output, and the results it reports, from the same command
/// pasted again with `spinel` a shell function that adds `--reporter json`
/// after `test`.
fn paste(fixture: &Path, command: &str) -> (Output, Vec<Value>) {
    let copy = copy_of(fixture);
    let fixture = copy.as_ref().map_or(fixture, |copy| copy.dir.as_path());
    let bin = Path::new(SPINEL).parent().expect("the binary's directory");
    let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("a PATH");
    let sh = |script: &str| {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(fixture)
            .env("PATH", &path)
            .output()
            .expect("sh runs")
    };
    let pasted = sh(command);
    let json = sh(&format!(
        "spinel() {{ subcommand=\"$1\"; shift; command spinel \"$subcommand\" --reporter json \"$@\"; }}\n{command}"
    ));
    let results = json_lines(&json).into_iter().filter(is_result).collect();
    (pasted, results)
}

/// A re-run command's location and the name of the test it is for.
fn location_and_name(command: &str) -> (&str, &str) {
    command
        .strip_prefix("spinel test ")
        .and_then(|rest| rest.split_once(" # "))
        .expect("a command and the test's name")
}

#[test]
fn each_printed_re_run_command_runs_its_failure_alone_when_pasted() {
    let dir = scratch("report-rerun");
    let output = spinel_test(&fixture("report"), &["-j", "2"], &dir.join("log"), &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}");
    let commands = reruns(&out);
    assert_eq!(commands.len(), 4, "{out}");

    for command in &commands {
        let (pasted, results) = paste(&fixture("report"), command);
        let again = stdout(&pasted);
        assert_eq!(pasted.status.code(), Some(1), "{command}: {again}");
        assert_eq!(results.len(), 1, "{command} runs one test: {results:?}");
        let (location, name) = location_and_name(command);
        let result = &results[0];
        assert_eq!(result["name"], name, "{command}: {result}");
        assert!(
            matches!(result["event"].as_str(), Some("fail" | "error")),
            "{command}: {result}"
        );
        assert_eq!(
            format!(
                "{}:{}",
                result["file"].as_str().expect("a file"),
                result["line"]
            ),
            location,
            "{command}: {result}"
        );
        // And it prints itself to run again.
        assert_eq!(reruns(&again), std::slice::from_ref(command), "{again}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_test_defined_outside_the_file_that_runs_it_re_runs_from_that_file() {
    // A Minitest test from a module a test class includes, and an RSpec
    // example from shared examples a group includes, each defined in a file
    // under `support/` that `spinel test` cannot run.
    let dir = scratch("report-shared");
    let output = spinel_test(&fixture("shared"), &["-j", "1"], &dir.join("log"), &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}{}", stderr(&output));
    assert!(
        out.contains(
            "  1) Failure: SharedTest#test_shared_fails [test/support/shared_tests.rb:3]\n"
        ),
        "the failure names where its test is defined: {out}"
    );
    assert!(
        out.contains(
            "  2) Failure: shared behaves like a sum adds up [spec/support/shared_examples.rb:3]\n"
        ),
        "{out}"
    );
    // The Minitest one runs again from the file that defined its class, all
    // of it; the RSpec one from the line that included it, as `rspec`'s own
    // summary prints it.
    let commands = reruns(&out);
    assert_eq!(
        commands,
        [
            "spinel test test/shared_test.rb # SharedTest#test_shared_fails",
            "spinel test spec/shared_spec.rb:6 # shared behaves like a sum adds up",
        ],
        "{out}"
    );
    for command in &commands {
        let (pasted, results) = paste(&fixture("shared"), command);
        let again = stdout(&pasted);
        assert_eq!(pasted.status.code(), Some(1), "{command}: {again}");
        let (_, name) = location_and_name(command);
        let failed: Vec<&Value> = results
            .iter()
            .filter(|result| result["event"] == "fail")
            .collect();
        assert_eq!(failed.len(), 1, "{command}: {results:?}");
        assert_eq!(failed[0]["name"], name, "{command}: {results:?}");
        assert_eq!(reruns(&again), std::slice::from_ref(command), "{again}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_line_runs_the_test_or_example_defined_nearest_above_it() {
    for (arg, expected) in [
        // Inside a test's body, and on the line its method starts.
        ("test/report_test.rb:20", vec!["ReportTest#test_errors"]),
        ("test/report_test.rb:4", vec!["ReportTest#test_passes"]),
        // Above every test: none.
        ("test/report_test.rb:2", vec![]),
        // Inside an example's body; on a group's line, all of the group.
        ("spec/report_spec.rb:12", vec!["report is pending"]),
        (
            "spec/report_spec.rb:3",
            vec!["report fails", "report is pending", "report passes"],
        ),
    ] {
        let (output, events, _) = events_of("report", &["-j", "1", arg]);
        let mut names: Vec<&str> = events
            .iter()
            .filter(|e| is_result(e))
            .map(|e| e["name"].as_str().expect("a name"))
            .collect();
        names.sort_unstable();
        assert_eq!(names, expected, "{arg}: {}", stdout(&output));
    }
}

#[test]
fn a_test_in_a_class_reopened_by_a_second_file_re_runs_from_that_file() {
    // Both files on one worker: the class appears with the first, and the
    // failing test is defined in the second.
    let dir = scratch("report-reopened");
    let output = spinel_test(&fixture("reopened"), &["-j", "1"], &dir.join("log"), &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}{}", stderr(&output));
    let commands = reruns(&out);
    assert_eq!(
        commands,
        ["spinel test test/b_test.rb:5 # SplitTest#test_b"],
        "{out}"
    );
    let (pasted, results) = paste(&fixture("reopened"), &commands[0]);
    let again = stdout(&pasted);
    assert_eq!(pasted.status.code(), Some(1), "{again}");
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0]["name"], "SplitTest#test_b", "{results:?}");
    assert_eq!(results[0]["event"], "fail", "{results:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_inherited_test_re_runs_from_the_file_of_the_class_that_failed() {
    // `SqliteTest` inherits `test_roundtrip` from `AdapterTest`, defined in
    // another file given, and fails it; both files on one worker.
    let dir = scratch("report-inherited");
    let output = spinel_test(&fixture("inherited"), &["-j", "1"], &dir.join("log"), &[]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{out}{}", stderr(&output));
    let commands = reruns(&out);
    assert_eq!(
        commands,
        ["spinel test test/sqlite_test.rb # SqliteTest#test_roundtrip"],
        "{out}"
    );
    let (pasted, results) = paste(&fixture("inherited"), &commands[0]);
    let again = stdout(&pasted);
    assert_eq!(pasted.status.code(), Some(1), "{again}");
    let failed: Vec<&Value> = results
        .iter()
        .filter(|result| result["event"] != "pass")
        .collect();
    assert_eq!(failed.len(), 1, "{results:?}");
    assert_eq!(
        failed[0]["name"], "SqliteTest#test_roundtrip",
        "{results:?}"
    );
    assert_eq!(failed[0]["event"], "fail", "{results:?}");
    assert_eq!(
        reruns(&again),
        std::slice::from_ref(&commands[0]),
        "{again}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_path_that_is_not_one_file_given_once_is_an_error() {
    for (args, error) in [
        (
            &["test/missing_test.rb:3"][..],
            "no such file: test/missing_test.rb",
        ),
        (&["test/nowhere"][..], "no such file: test/nowhere"),
        (
            &["test:3"][..],
            "test is a directory; a line number needs a file",
        ),
        (
            &["lib"][..],
            "no test files in lib: nothing matches **/*_test.rb or **/*_spec.rb under it",
        ),
        (
            &["test/report_test.rb:8", "test/report_test.rb"][..],
            "test/report_test.rb is given more than once",
        ),
    ] {
        let dir = scratch("report-missing");
        let output = spinel_test(&fixture("report"), args, &dir.join("log"), &[]);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(stdout(&output), "", "nothing ran: {args:?}");
        assert!(stderr(&output).contains(error), "{}", stderr(&output));
    }
}

/// `spinel test` with `args` in the `rspec` fixture, a project with both
/// suites, on one worker: what exited, and the test and spec files that
/// ran, each in the order it ran.
fn ran_in_rspec_fixture(args: &[&str]) -> (Output, Vec<String>, Vec<String>) {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = scratch(&format!("directories-{call}"));
    let log = dir.join("log");
    let args: Vec<&str> = ["-j", "1"].iter().chain(args).copied().collect();
    let output = spinel_test(&fixture("rspec"), &args, &log, &[]);
    let lines = lines(&log);
    std::fs::remove_dir_all(&dir).ok();
    let named = |kind: &str| -> Vec<String> {
        by_name(&lines, kind)
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect()
    };
    (output, named("file"), named("spec"))
}

#[test]
fn a_directory_given_runs_the_test_and_spec_files_under_it() {
    // #910: `spinel test spec` runs only the specs of a project with both,
    // as `rspec spec` does, and `spinel test test` only the tests, as
    // `rails test test` does; nested directories are searched, in sorted
    // order, with or without a trailing slash.
    for (args, tests, specs) in [
        (&["spec"][..], &[][..], &["c_spec.rb", "d_spec.rb"][..]),
        (&["spec/"][..], &[][..], &["c_spec.rb", "d_spec.rb"][..]),
        (&["test"][..], &["a_test.rb", "b_test.rb"][..], &[][..]),
        (&["test/"][..], &["a_test.rb", "b_test.rb"][..], &[][..]),
        (&["spec/models"][..], &[][..], &["d_spec.rb"][..]),
        (&["./spec/models/"][..], &[][..], &["d_spec.rb"][..]),
        // A directory and a file mix, and a file a directory also brings
        // in runs once.
        (
            &["spec/models", "test/b_test.rb"][..],
            &["b_test.rb"][..],
            &["d_spec.rb"][..],
        ),
        (
            &["spec", "spec/models"][..],
            &[][..],
            &["c_spec.rb", "d_spec.rb"][..],
        ),
        (
            &["spec/models/d_spec.rb", "spec"][..],
            &[][..],
            &["d_spec.rb", "c_spec.rb"][..],
        ),
    ] {
        let (output, ran_tests, ran_specs) = ran_in_rspec_fixture(args);
        let err = stderr(&output);
        // `d_spec.rb`'s example fails; every other file passes.
        let failed = specs.contains(&"d_spec.rb");
        assert_eq!(output.status.success(), !failed, "{args:?}: {err}");
        assert_eq!(ran_tests, tests, "{args:?}: {err}");
        assert_eq!(ran_specs, specs, "{args:?}: {err}");
    }
}

#[test]
fn a_file_given_with_a_directory_that_has_it_is_not_given_twice() {
    // A file a directory also brings in, given with or without a line,
    // runs once; the same file given twice is still an error.
    for args in [
        &["spec", "spec/c_spec.rb:4"][..],
        &["spec/c_spec.rb", "spec"][..],
    ] {
        let (output, ran_tests, ran_specs) = ran_in_rspec_fixture(args);
        let err = stderr(&output);
        assert!(!err.contains("more than once"), "{args:?}: {err}");
        assert!(ran_tests.is_empty(), "{args:?}: {err}");
        assert_eq!(ran_specs, ["c_spec.rb", "d_spec.rb"], "{args:?}: {err}");
    }
    let (output, ran_tests, ran_specs) =
        ran_in_rspec_fixture(&["spec/c_spec.rb", "spec/c_spec.rb:4"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(ran_tests.is_empty() && ran_specs.is_empty());
    assert!(
        stderr(&output).contains("spec/c_spec.rb is given more than once"),
        "{}",
        stderr(&output)
    );
}

/// Each discovery error `reporter` is given: the directory to run in, the
/// arguments, and the message.
fn discovery_errors(reporter: &str) -> [(PathBuf, Vec<&'static str>, &'static str); 2] {
    [
        (
            fixture("report"),
            vec!["test/missing_test.rb"],
            "no such file: test/missing_test.rb",
        ),
        (
            scratch(&format!("discovery-empty-{reporter}")),
            vec![],
            "no test files: nothing matches test/**/*_test.rb or spec/**/*_spec.rb",
        ),
    ]
}

#[test]
fn the_json_reporter_reports_a_discovery_error_as_a_problem_then_the_summary() {
    for (dir, args, message) in discovery_errors("json") {
        let log = scratch("json-discovery");
        let args: Vec<&str> = ["--reporter", "json"]
            .iter()
            .chain(&args)
            .copied()
            .collect();
        let output = spinel_test(&dir, &args, &log.join("log"), &[]);
        std::fs::remove_dir_all(&log).ok();
        if args.len() == 2 {
            std::fs::remove_dir_all(&dir).ok();
        }
        let err = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {err}");
        assert_eq!(
            json_lines(&output),
            vec![
                json!({"event": "problem", "message": message, "rerun": null}),
                json!({"event": "summary", "tests": 0, "failures": 0, "errors": 0, "skips": 0,
                       "problems": 1}),
            ],
            "{args:?}"
        );
        assert_eq!(
            err.matches(&format!("spinel test: {message}\n")).count(),
            1,
            "told once on stderr: {err}"
        );
    }
}

#[test]
fn the_junit_reporter_reports_a_discovery_error_as_an_error() {
    for (dir, args, message) in discovery_errors("junit") {
        let log = scratch("junit-discovery");
        let args: Vec<&str> = ["--reporter", "junit"]
            .iter()
            .chain(&args)
            .copied()
            .collect();
        let output = spinel_test(&dir, &args, &log.join("log"), &[]);
        std::fs::remove_dir_all(&log).ok();
        if args.len() == 2 {
            std::fs::remove_dir_all(&dir).ok();
        }
        let err = stderr(&output);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {err}");
        let xml = String::from_utf8(output.stdout.clone()).expect("UTF-8");
        junit_valid(&xml);
        assert_eq!(
            junit_cases(&xml),
            vec![(
                "spinel test".to_owned(),
                vec![case(["spinel test", message, "", "error", message, ""])],
            )],
            "{xml}"
        );
        assert_eq!(
            err.matches(&format!("spinel test: {message}\n")).count(),
            1,
            "told once on stderr: {err}"
        );
    }
}

/// What every helper and test in the `noisy` fixture prints to stdout.
const NOISY_PRINTS: [&str; 6] = [
    "test_helper prints",
    "a test prints",
    "and writes",
    "and prints to STDOUT",
    "spec_helper prints",
    "an example prints",
];

#[test]
fn the_json_reporter_prints_only_events_on_stdout_and_what_tests_print_on_stderr() {
    let (output, events, summary) = events_of("noisy", &["-j", "2"]);
    let err = stderr(&output);
    assert_eq!(
        output.status.code(),
        Some(1),
        "an error and a failure: {err}"
    );
    let out = stdout(&output);
    for printed in NOISY_PRINTS {
        assert!(!out.contains(printed), "{printed:?} on stdout: {out}");
        assert!(
            err.lines().any(|line| line == printed),
            "{printed:?} on stderr: {err}"
        );
    }
    // Each event as the worker wrote it; the markup and the escape character
    // in a message come through as they were raised.
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().expect("a kind"))
        .collect();
    assert_eq!(
        kinds.iter().filter(|kind| **kind == "done").count(),
        2,
        "{kinds:?}"
    );
    let error = events
        .iter()
        .find(|e| e["event"] == "error")
        .expect("the error");
    assert_eq!(
        error["message"], "ArgumentError: <b> & \"quoted\" 'single'\u{1b}[31m\tred",
        "{error}"
    );
    assert_eq!(
        summary,
        json!({"event": "summary", "tests": 5, "failures": 1, "errors": 1, "skips": 1,
               "problems": 0})
    );
}

#[test]
fn the_json_reporter_ends_with_each_problem_then_the_summary() {
    let dir = scratch("json-crash");
    let output = spinel_test(
        &fixture("crash"),
        &["-j", "2", "--reporter", "json"],
        &dir.join("log"),
        &[],
    );
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{err}");
    let events = json_lines(&output);
    let (summary, rest) = events.split_last().expect("a summary");
    assert_eq!(
        summary,
        &json!({"event": "summary", "tests": 0, "failures": 0, "errors": 0, "skips": 0,
                "problems": 3})
    );
    let problems: Vec<&Value> = rest.iter().filter(|e| e["event"] == "problem").collect();
    assert_eq!(
        problems.len(),
        3,
        "the problems come last, after the files' events: {events:?}"
    );
    assert!(
        rest.ends_with(&problems.iter().copied().cloned().collect::<Vec<_>>()),
        "{events:?}"
    );
    let reruns: BTreeSet<&str> = problems
        .iter()
        .map(|p| p["rerun"].as_str().expect("a file to run again"))
        .collect();
    assert_eq!(
        reruns,
        BTreeSet::from([
            "test/b_killed_test.rb",
            "test/d_exit_test.rb",
            "test/f_raise_test.rb"
        ])
    );
    for problem in &problems {
        let keys: BTreeSet<&str> = problem
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(["event", "message", "rerun"]));
        // Told on stderr too, as with every reporter.
        let message = problem["message"].as_str().expect("a message");
        assert!(
            err.contains(&format!("spinel test: {message}\n")),
            "{message}: {err}"
        );
    }
    assert!(
        problems
            .iter()
            .any(|p| p["message"] == "test/f_raise_test.rb: ArgumentError: broken on load"),
        "{problems:?}"
    );
}

/// `spinel test --reporter junit` with `args` on the fixture `name`: its
/// output, and its stdout, checked to be one JUnit XML document as CI
/// systems read it (`junit_valid`).
fn junit_of(name: &str, args: &[&str]) -> (Output, String) {
    let dir = scratch(&format!("junit-{name}"));
    let args: Vec<&str> = ["--reporter", "junit"]
        .iter()
        .chain(args)
        .copied()
        .collect();
    let output = spinel_test(&fixture(name), &args, &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    let xml = String::from_utf8(output.stdout.clone()).expect("UTF-8, as it says");
    assert!(
        xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"),
        "{xml}"
    );
    assert!(xml.ends_with("</testsuites>\n"), "nothing after it: {xml}");
    junit_valid(&xml);
    (output, xml)
}

/// The attributes `node` has, after checking each is one `allowed` and each
/// `required` is there.
fn attributes<'a>(
    node: roxmltree::Node<'a, '_>,
    allowed: &[&str],
    required: &[&str],
) -> BTreeMap<&'a str, &'a str> {
    let found: BTreeMap<&str, &str> = node
        .attributes()
        .map(|attribute| (attribute.name(), attribute.value()))
        .collect();
    for name in found.keys() {
        assert!(
            allowed.contains(name),
            "<{}> has no attribute {name}",
            node.tag_name().name()
        );
    }
    for name in required {
        assert!(
            found.contains_key(name),
            "<{}> needs {name}",
            node.tag_name().name()
        );
    }
    found
}

/// A count attribute: `xs:nonNegativeInteger`.
fn count_of(attributes: &BTreeMap<&str, &str>, name: &str) -> usize {
    let value = attributes.get(name).copied().unwrap_or("0");
    assert!(
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
        "{name}={value:?}"
    );
    value.parse().expect("a count")
}

/// A `time`: seconds, an `xs:decimal` of at least 0.
fn check_seconds(attributes: &BTreeMap<&str, &str>) {
    if let Some(time) = attributes.get("time") {
        let (whole, fraction) = time.split_once('.').unwrap_or((time, "0"));
        assert!(
            !whole.is_empty()
                && !fraction.is_empty()
                && whole
                    .bytes()
                    .chain(fraction.bytes())
                    .all(|b| b.is_ascii_digit()),
            "time={time:?}"
        );
    }
}

/// The element children of `node`.
fn elements<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> {
    node.children().filter(roxmltree::Node::is_element)
}

/// Checks `xml` against the schema CI systems read JUnit XML by, Jenkins'
/// (`tests/fixtures/test_runner/junit.xsd`, which has its references):
/// with `xmllint` against the XSD itself, when `xmllint` is installed, and
/// always against the same rules here, element by element and attribute by
/// attribute, with the counts that an XSD cannot check (each `tests`,
/// `failures`, `errors` and `skipped` is what the cases in it add up to).
fn junit_valid(xml: &str) {
    let schema = fixture("junit.xsd");
    match Command::new("xmllint")
        .args(["--noout", "--schema"])
        .arg(&schema)
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(xml.as_bytes())
                .expect("xmllint reads it");
            let checked = child.wait_with_output().expect("xmllint runs");
            assert!(
                checked.status.success(),
                "xmllint: {}{xml}",
                String::from_utf8_lossy(&checked.stderr)
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("xmllint is not installed: checked by the rules here only");
        }
        Err(err) => panic!("xmllint: {err}"),
    }

    let document = roxmltree::Document::parse(xml).unwrap_or_else(|err| panic!("{err}: {xml}"));
    let root = document.root_element();
    assert_eq!(root.tag_name().name(), "testsuites", "{xml}");
    let totals = attributes(
        root,
        &["name", "tests", "failures", "errors", "disabled", "time"],
        &[],
    );
    check_seconds(&totals);
    let mut sums = [0; 3];
    for suite in elements(root) {
        assert_eq!(suite.tag_name().name(), "testsuite", "{xml}");
        let attrs = attributes(
            suite,
            &[
                "name",
                "tests",
                "failures",
                "errors",
                "skipped",
                "disabled",
                "time",
                "timestamp",
                "hostname",
                "id",
                "package",
                "file",
            ],
            &["name", "tests"],
        );
        check_seconds(&attrs);
        if let Some(timestamp) = attrs.get("timestamp") {
            // `xs:dateTime`, as written: `YYYY-MM-DDTHH:MM:SS`.
            let shape: String = timestamp
                .chars()
                .map(|c| if c.is_ascii_digit() { '0' } else { c })
                .collect();
            assert_eq!(shape, "0000-00-00T00:00:00", "timestamp={timestamp}");
        }
        let mut counts = [0; 4];
        for case in elements(suite) {
            assert_eq!(case.tag_name().name(), "testcase", "{xml}");
            let case_attrs = attributes(
                case,
                &[
                    "name",
                    "classname",
                    "time",
                    "file",
                    "line",
                    "assertions",
                    "status",
                ],
                &["name"],
            );
            check_seconds(&case_attrs);
            if case_attrs.contains_key("line") {
                assert!(count_of(&case_attrs, "line") > 0, "{case_attrs:?}");
            }
            counts[0] += 1;
            let mut outcomes = 0;
            for child in elements(case) {
                let index = match child.tag_name().name() {
                    "failure" => 1,
                    "error" => 2,
                    "skipped" => 3,
                    "system-out" | "system-err" => continue,
                    other => panic!("<testcase> has no <{other}>"),
                };
                let allowed: &[&str] = if index == 3 {
                    &["message"]
                } else {
                    &["message", "type"]
                };
                attributes(child, allowed, &[]);
                assert!(
                    elements(child).next().is_none(),
                    "text only in <{}>",
                    child.tag_name().name()
                );
                counts[index] += 1;
                outcomes += 1;
            }
            assert!(outcomes <= 1, "one outcome per test: {case_attrs:?}");
        }
        assert_eq!(
            [
                count_of(&attrs, "tests"),
                count_of(&attrs, "failures"),
                count_of(&attrs, "errors"),
                count_of(&attrs, "skipped"),
            ],
            counts,
            "{attrs:?}"
        );
        sums[0] += counts[0];
        sums[1] += counts[1];
        sums[2] += counts[2];
    }
    assert_eq!(
        [
            count_of(&totals, "tests"),
            count_of(&totals, "failures"),
            count_of(&totals, "errors"),
        ],
        sums,
        "{totals:?}"
    );
}

/// Each `testsuite`'s name, and each of its cases as
/// `[classname, name, file:line, outcome, its message, its text]`.
fn junit_cases(xml: &str) -> Vec<(String, Vec<[String; 6]>)> {
    let document = roxmltree::Document::parse(xml).expect("parsed");
    elements(document.root_element())
        .map(|suite| {
            let cases = elements(suite)
                .map(|case| {
                    let outcome = elements(case).next();
                    [
                        case.attribute("classname").unwrap_or_default().to_owned(),
                        case.attribute("name").unwrap_or_default().to_owned(),
                        [case.attribute("file"), case.attribute("line")]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(":"),
                        outcome.map_or("pass", |o| o.tag_name().name()).to_owned(),
                        outcome
                            .and_then(|o| o.attribute("message"))
                            .unwrap_or_default()
                            .to_owned(),
                        outcome
                            .and_then(|o| o.text())
                            .unwrap_or_default()
                            .to_owned(),
                    ]
                })
                .collect();
            (
                suite.attribute("name").unwrap_or_default().to_owned(),
                cases,
            )
        })
        .collect()
}

fn case(fields: [&str; 6]) -> [String; 6] {
    fields.map(str::to_owned)
}

#[test]
fn the_junit_reporter_prints_a_testsuite_per_file_as_ci_systems_read_it() {
    let (output, xml) = junit_of("report", &["-j", "2"]);
    assert_eq!(output.status.code(), Some(1), "a test failed: {xml}");
    assert_eq!(stderr(&output), "", "no problems");
    let frame = "spec/report_spec.rb:7:in 'block (2 levels) in <top (required)>'";
    let failure_text = |message: &str, frames: &[&str]| {
        std::iter::once(message)
            .chain(frames.iter().copied())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut suites = junit_cases(&xml);
    // The Minitest suite first, then the RSpec one, each case by its line.
    let names: Vec<&str> = suites.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["test/report_test.rb", "spec/report_spec.rb"],
        "{xml}"
    );
    let (_, rspec) = suites.pop().expect("the RSpec suite");
    let (_, minitest) = suites.pop().expect("the Minitest suite");

    // Minitest's: the test's class, and its method.
    let file = "test/report_test.rb";
    assert_eq!(minitest.len(), 5, "{xml}");
    assert_eq!(
        minitest[0],
        case([
            "ReportTest",
            "test_passes",
            &format!("{file}:4"),
            "pass",
            "",
            ""
        ])
    );
    let [classname, name, at, outcome, message, text] = &minitest[1];
    assert_eq!(
        [classname, name, at, outcome, message],
        [
            "ReportTest",
            "test_fails",
            &format!("{file}:8"),
            "failure",
            "Expected: 3\n  Actual: 2"
        ],
        "{xml}"
    );
    // The message, then the backtrace, the framework's, whole.
    assert!(
        text.starts_with(&failure_text(
            "Expected: 3\n  Actual: 2",
            &["test/report_test.rb:9:in 'ReportTest#test_fails'"]
        )),
        "{text}"
    );
    assert_eq!(minitest[2][3], "failure", "{xml}");
    assert!(
        minitest[2][5].contains("vendor/gems/checker-1.0/lib/checker.rb:13"),
        "nothing folded: {xml}"
    );
    let [classname, name, at, outcome, message, text] = &minitest[3];
    assert_eq!(
        [classname, name, at, outcome, message],
        [
            "ReportTest",
            "test_errors",
            &format!("{file}:18"),
            "error",
            "ArgumentError: broken"
        ],
        "{xml}"
    );
    assert_eq!(
        text,
        &failure_text(
            "ArgumentError: broken",
            &["test/report_test.rb:19:in 'ReportTest#test_errors'"]
        )
    );
    assert_eq!(
        minitest[4],
        case([
            "ReportTest",
            "test_skips",
            &format!("{file}:22"),
            "skipped",
            "not yet",
            ""
        ])
    );

    // RSpec's: the file's path with dots, and the example's full description.
    let file = "spec/report_spec.rb";
    assert_eq!(
        rspec,
        [
            case([
                "spec.report_spec",
                "report passes",
                &format!("{file}:4"),
                "pass",
                "",
                ""
            ]),
            case([
                "spec.report_spec",
                "report fails",
                &format!("{file}:6"),
                "failure",
                "expected: 3\n     got: 2",
                &failure_text("expected: 3\n     got: 2", &[frame]),
            ]),
            case([
                "spec.report_spec",
                "report is pending",
                &format!("{file}:10"),
                "skipped",
                "not yet",
                ""
            ]),
        ],
        "{xml}"
    );

    let document = roxmltree::Document::parse(&xml).expect("parsed");
    let root = document.root_element();
    assert_eq!(
        ["tests", "failures", "errors"].map(|name| root.attribute(name)),
        [Some("8"), Some("3"), Some("1")],
        "{xml}"
    );
}

#[test]
fn the_junit_reporter_escapes_what_xml_cannot_hold_and_tests_print_on_stderr() {
    let (output, xml) = junit_of("noisy", &["-j", "2"]);
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{err}");
    for printed in NOISY_PRINTS {
        assert!(!xml.contains(printed), "{printed:?} in the XML: {xml}");
        assert!(
            err.lines().any(|line| line == printed),
            "{printed:?} on stderr: {err}"
        );
    }
    let cases: BTreeMap<String, [String; 6]> = junit_cases(&xml)
        .into_iter()
        .flat_map(|(_, cases)| cases)
        .map(|case| (case[1].clone(), case))
        .collect();
    // Markup read back as it was raised, and the escape character, which
    // XML 1.0 cannot hold, as `\u001B`.
    let error = &cases["test_errors_with_markup"];
    assert_eq!(error[3], "error", "{xml}");
    assert_eq!(
        error[4], "ArgumentError: <b> & \"quoted\" 'single'\\u001B[31m\tred",
        "{xml}"
    );
    assert!(error[5].starts_with(&error[4]), "{xml}");
    assert_eq!(cases["test_skips_with_markup"][4], "<later> & \"then\"");
    let failure = &cases["noisy fails with <markup> & more"];
    assert_eq!(failure[3], "failure", "{xml}");
    assert_eq!(failure[4], "ArgumentError: <i> & \"quoted\"\\u001B[0m");
    assert_eq!(cases["test_prints"][3], "pass", "{xml}");
    assert_eq!(cases["noisy prints"][3], "pass", "{xml}");
}

#[test]
fn the_junit_reporter_reports_each_problem_as_an_error() {
    let dir = scratch("junit-crash");
    let output = spinel_test(
        &fixture("crash"),
        &["-j", "2", "--reporter", "junit"],
        &dir.join("log"),
        &[],
    );
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{err}");
    let xml = String::from_utf8(output.stdout.clone()).expect("UTF-8");
    junit_valid(&xml);
    let suites = junit_cases(&xml);
    assert_eq!(suites.len(), 1, "no test ran: {xml}");
    let (name, cases) = &suites[0];
    assert_eq!(name, "spinel test", "{xml}");
    let files: BTreeSet<&str> = cases.iter().map(|case| case[1].as_str()).collect();
    assert_eq!(
        files,
        BTreeSet::from([
            "test/b_killed_test.rb",
            "test/d_exit_test.rb",
            "test/f_raise_test.rb"
        ]),
        "{xml}"
    );
    for case in cases {
        assert_eq!(case[3], "error", "{xml}");
        // Told on stderr too.
        assert!(
            err.contains(&format!("spinel test: {}\n", case[4])),
            "{}: {err}",
            case[4]
        );
    }
    assert!(
        cases
            .iter()
            .any(|case| case[4] == "test/f_raise_test.rb: ArgumentError: broken on load"),
        "{xml}"
    );
}

#[test]
fn each_reporter_exits_zero_when_nothing_failed_and_one_otherwise() {
    for reporter in ["progress", "json", "junit"] {
        for (args, code) in [
            (&["test/report_test.rb:22", "spec/report_spec.rb:4"][..], 0),
            (&["test/report_test.rb:8"][..], 1),
        ] {
            let dir = scratch(&format!("status-{reporter}"));
            let args: Vec<&str> = ["-j", "1", "--reporter", reporter]
                .iter()
                .chain(args)
                .copied()
                .collect();
            let output = spinel_test(&fixture("report"), &args, &dir.join("log"), &[]);
            std::fs::remove_dir_all(&dir).ok();
            assert_eq!(
                output.status.code(),
                Some(code),
                "{reporter} {args:?}: {}{}",
                stdout(&output),
                stderr(&output)
            );
        }
    }
}

#[test]
fn an_unknown_reporter_is_an_error() {
    let dir = scratch("reporter-unknown");
    let output = spinel_test(
        &fixture("report"),
        &["--reporter", "tap"],
        &dir.join("log"),
        &[],
    );
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(2), "a usage error");
    let err = stderr(&output);
    assert!(err.contains("invalid value 'tap'"), "{err}");
    assert!(err.contains("progress, json, junit"), "{err}");
}

/// The times a run kept in `dir`, by path, as `.spinel/test_timings.json`
/// holds them; checks that file's shape, and that nothing else was left
/// beside it, such as the file it was written to before being renamed.
fn timings(dir: &Path) -> BTreeMap<String, f64> {
    let kept = dir.join(".spinel");
    let names: Vec<String> = std::fs::read_dir(&kept)
        .expect(".spinel/ exists")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        ["test_timings.json"],
        "nothing else is left in .spinel/"
    );
    let text = std::fs::read_to_string(kept.join("test_timings.json")).expect("the timings");
    let json: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(json["version"], 1, "{text}");
    json["files"]
        .as_object()
        .unwrap_or_else(|| panic!("a file per path: {text}"))
        .iter()
        .map(|(path, ms)| {
            let ms = ms
                .as_f64()
                .unwrap_or_else(|| panic!("milliseconds: {text}"));
            (path.clone(), ms)
        })
        .collect()
}

/// The files each worker was handed, by its pid, from a `timings` log.
fn handed(log: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut workers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in lines(log) {
        workers
            .entry(line[1].clone())
            .or_default()
            .insert(line[2].clone());
    }
    workers
}

/// The files of the worker that ran the slow file in the `timings` fixture.
fn with_the_slow_file(log: &Path) -> BTreeSet<String> {
    let workers = handed(log);
    let files = workers
        .values()
        .find(|files| files.contains("z_slow_test.rb"));
    files
        .cloned()
        .unwrap_or_else(|| panic!("the slow file ran: {workers:?}"))
}

#[test]
fn each_files_time_is_kept_and_the_next_run_hands_out_the_slowest_first() {
    // Six quick files, then one slow one, last in file order, on two
    // workers. The first run hands them out in file order, the slow file
    // to a worker with three quick ones: the run takes the slow file and
    // three quick ones. The second, with the times kept, hands the slow file
    // out first, to a worker of its own, and the six quick ones, which take
    // less than it together, to the other: the run takes the slow file.
    let copy = copy_of(&fixture("timings")).expect("a copy");
    let dir = scratch("timings");

    let started = std::time::Instant::now();
    let output = spinel_test(&copy.dir, &["-j", "2"], &dir.join("first"), &[]);
    let first = started.elapsed();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let kept = timings(&copy.dir);
    let quick = ["a", "b", "c", "d", "e", "f"].map(|name| format!("test/{name}_test.rb"));
    let names: BTreeSet<&str> = kept.keys().map(String::as_str).collect();
    let mut expected: BTreeSet<&str> = quick.iter().map(String::as_str).collect();
    expected.insert("test/z_slow_test.rb");
    assert_eq!(names, expected, "every file's time is kept: {kept:?}");
    let slow = kept["test/z_slow_test.rb"];
    assert!(slow >= 2000.0, "the slow file's test sleeps 2s: {kept:?}");
    for file in &quick {
        assert!(kept[file] >= 250.0 && kept[file] < slow, "{file}: {kept:?}");
    }
    let shared = with_the_slow_file(&dir.join("first"));
    assert_eq!(
        shared.len(),
        4,
        "in file order, with three quick files: {shared:?}"
    );

    let started = std::time::Instant::now();
    let output = spinel_test(&copy.dir, &["-j", "2"], &dir.join("second"), &[]);
    let second = started.elapsed();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let alone = with_the_slow_file(&dir.join("second"));
    assert_eq!(
        alone,
        BTreeSet::from(["z_slow_test.rb".to_owned()]),
        "longest first, on a worker of its own: {:?}",
        handed(&dir.join("second"))
    );
    assert_eq!(handed(&dir.join("second")).len(), 2, "on two workers");
    std::fs::remove_dir_all(&dir).ok();
    // Three quick files' 0.75s fewer, with room for a slow machine.
    assert!(
        second < first,
        "longest first finishes sooner: {second:?}, in file order {first:?}"
    );
    assert_eq!(timings(&copy.dir).len(), 7, "and keeps the times again");
}

#[test]
fn times_missing_unreadable_or_stale_are_left_out_and_every_file_still_runs() {
    let dir = scratch("timings-bad");
    let stale = r#"{"version": 1, "files": {"test/gone_test.rb": 99999, "test/a_test.rb": "slow", "test/b_test.rb": -1}}"#;
    for (case, kept) in [
        ("missing", None),
        ("not JSON", Some("{\"version\": 1, \"files\": {")),
        ("not an object", Some("[1, 2]")),
        (
            "another version",
            Some(r#"{"version": 2, "files": {"test/a_test.rb": 5}}"#),
        ),
        ("stale and wrong", Some(stale)),
    ] {
        let copy = copy_of(&fixture("reopened")).expect("a copy");
        if let Some(kept) = kept {
            std::fs::create_dir_all(copy.dir.join(".spinel")).expect(".spinel/");
            std::fs::write(copy.dir.join(".spinel/test_timings.json"), kept).expect("written");
        }
        let output = spinel_test(&copy.dir, &["-j", "2"], &dir.join("log"), &[]);
        let out = stdout(&output);
        // `reopened` has a test that fails, and its time is still kept.
        assert_eq!(output.status.code(), Some(1), "{case}: {}", stderr(&output));
        assert!(
            out.contains("2 tests, 1 failure, 0 errors, 0 skips"),
            "{case}: {out}"
        );
        assert!(
            !stderr(&output).contains("spinel test:"),
            "{case}: {}",
            stderr(&output)
        );
        let kept = timings(&copy.dir);
        let names: Vec<&str> = kept.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["test/a_test.rb", "test/b_test.rb"],
            "{case}: the files that ran, and no file that is gone: {kept:?}"
        );
    }

    // A directory where the file goes: told, and nothing else changes.
    let copy = copy_of(&fixture("reopened")).expect("a copy");
    std::fs::create_dir_all(copy.dir.join(".spinel/test_timings.json")).expect("a directory");
    let output = spinel_test(&copy.dir, &["-j", "2"], &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{err}");
    assert!(
        err.contains("spinel test: test times not kept in .spinel/test_timings.json"),
        "{err}"
    );
    assert!(!err.contains("problem"), "{err}");
    assert!(
        stdout(&output).contains("2 tests, 1 failure, 0 errors, 0 skips\n"),
        "{err}"
    );
    let left: Vec<_> = std::fs::read_dir(copy.dir.join(".spinel"))
        .expect(".spinel/")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert_eq!(
        left,
        ["test_timings.json"],
        "no file written to rename is left"
    );
}

#[test]
fn a_file_run_in_part_or_ending_in_a_problem_keeps_the_time_it_had() {
    let dir = scratch("timings-partial");
    let copy = copy_of(&fixture("reopened")).expect("a copy");
    let kept_at = |a: f64, b: f64| {
        std::fs::create_dir_all(copy.dir.join(".spinel")).expect(".spinel/");
        let json = json!({"version": 1, "files": {"test/a_test.rb": a, "test/b_test.rb": b}});
        std::fs::write(copy.dir.join(".spinel/test_timings.json"), json.to_string())
            .expect("written");
    };

    // One test of a file, by its line: not the file's time.
    kept_at(55555.0, 66666.0);
    let output = spinel_test(&copy.dir, &["test/a_test.rb:5"], &dir.join("log"), &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let kept = timings(&copy.dir);
    assert_eq!(kept["test/a_test.rb"], 55555.0, "{kept:?}");
    assert_eq!(kept["test/b_test.rb"], 66666.0, "{kept:?}");

    // A file given whole is timed again; the others keep theirs.
    let output = spinel_test(&copy.dir, &["test/b_test.rb"], &dir.join("log"), &[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let kept = timings(&copy.dir);
    assert_eq!(kept["test/a_test.rb"], 55555.0, "{kept:?}");
    assert!(kept["test/b_test.rb"] < 66666.0, "{kept:?}");

    // A file whose worker died, exited or whose loading raised keeps no new
    // time; one whose worker ended well is timed.
    let copy = copy_of(&fixture("crash")).expect("a copy");
    let output = spinel_test(&copy.dir, &["-j", "2"], &dir.join("log"), &[]);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let kept = timings(&copy.dir);
    for file in kept.keys() {
        assert!(
            ["a", "c", "e", "g"]
                .map(|name| format!("test/{name}_test.rb"))
                .contains(file),
            "only files whose worker ended well: {kept:?}"
        );
    }
    assert!(!kept.is_empty(), "the replacement workers' files: {kept:?}");
}
