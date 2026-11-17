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
//! `shims/test_worker.rb` registers, one per test or example. The parent
//! appends each line it reads to the file `SPINEL_TEST_EVENTS` names, which
//! is what these tests read. minitest is not installed here either, so the
//! fixtures that run tests carry a stand-in for the part of its API a run
//! goes through, in `stand_ins/minitest.rb`; the real gems' events are
//! checked by hand against CRuby in #93's pull request.
//!
//! The runner prints the report itself (#94): a character per result as it
//! comes, then each failure and error with its `file:line` and backtrace,
//! gem frames folded, a summary line, and a `spinel test file:line` command
//! per failure, which these tests paste into a shell and run. Each
//! framework's own report is left out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/test_runner")
        .join(name)
}

/// A log file of its own for one test, in a fresh directory.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("spinel-test-runner-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// `spinel test` with `args` in `dir`, logging to `log`.
fn spinel_test(dir: &Path, args: &[&str], log: &Path, env: &[(&str, &str)]) -> Output {
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

/// `spinel test` with `args` on the fixture `name`, and every event line the
/// parent read, each parsed: a line that is not one JSON object fails here.
fn events_of(name: &str, args: &[&str]) -> (Output, Vec<Value>) {
    let dir = scratch(&format!("events-{name}"));
    let events = dir.join("events");
    let events_path = events.to_str().expect("a UTF-8 path");
    let output = spinel_test(
        &fixture(name),
        args,
        &dir.join("log"),
        &[("SPINEL_TEST_EVENTS", events_path)],
    );
    let raw = std::fs::read(&events).unwrap_or_default();
    std::fs::remove_dir_all(&dir).ok();
    let text = String::from_utf8(raw).expect("events are UTF-8");
    assert!(
        text.is_empty() || text.ends_with('\n'),
        "every event ends its line"
    );
    let parsed = text
        .lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("not one JSON event ({err}): {line:.200}"));
            assert!(event.is_object(), "{line:.200}");
            event
        })
        .collect();
    (output, parsed)
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
    let (output, events) = events_of("events", &["-j", "2"]);
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

    // The runner's report, in place of each framework's own.
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("\n10 tests, 4 failures, 1 error, 3 skips\n"),
        "{out}"
    );
    assert!(!out.contains("runs,"), "Minitest's left out: {out}");
    assert!(!out.contains("examples,"), "RSpec's left out: {out}");
}

#[test]
fn many_events_from_several_workers_each_arrive_as_a_whole_line() {
    // Six Minitest files and three spec files of a hundred tests each, on
    // three workers at once; every tenth is skipped with a message longer
    // than a pipe holds and than the parent reads at once, which JSON
    // escapes and whose characters are several bytes each.
    let (output, events) = events_of("many", &["-j", "3"]);
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

/// `command` pasted into `sh` in `fixture`, with `spinel` on the PATH: its
/// output, and the results it reported.
fn paste(fixture: &Path, command: &str, events: &Path) -> (Output, Vec<Value>) {
    let bin = Path::new(SPINEL).parent().expect("the binary's directory");
    let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("a PATH");
    std::fs::remove_file(events).ok();
    let pasted = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(fixture)
        .env("PATH", &path)
        .env("SPINEL_TEST_EVENTS", events)
        .output()
        .expect("sh runs");
    let results = std::fs::read_to_string(events)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("an event"))
        .filter(is_result)
        .collect();
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
        let (pasted, results) = paste(&fixture("report"), command, &dir.join("events"));
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
        let (pasted, results) = paste(&fixture("shared"), command, &dir.join("events"));
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
        let (output, events) = events_of("report", &["-j", "1", arg]);
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
fn a_path_that_is_not_one_file_given_once_is_an_error() {
    for (args, error) in [
        (
            &["test/missing_test.rb:3"][..],
            "no such file: test/missing_test.rb",
        ),
        (&["test"][..], "no such file: test"),
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
