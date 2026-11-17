//! `spinel test`'s parent and workers (#92, docs/test-runner.md,
//! "Execution"), on the built `spinel`: the helpers are required once, in
//! the parent, the workers are forked after them, `-j` sets how many, and a
//! worker that dies is reported with the file it was running.
//!
//! Each fixture under `tests/fixtures/test_runner/` logs to the file
//! `SPINEL_TEST_LOG` names, one line per helper load and per test file, with
//! the pid that did it. Worker counts stay at 3 or under.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
/// each helper loaded once, in the parent, and every file run in a worker
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
    assert_eq!(
        loaded,
        ["test_helper", "spec_helper"],
        "each helper is loaded once: {lines:?}"
    );
    let parent = &helpers[0][1];
    assert_eq!(&helpers[1][1], parent, "both in the parent: {lines:?}");

    let files: Vec<&Vec<String>> = lines.iter().filter(|l| l[0] == "file").collect();
    let names: BTreeSet<&str> = files.iter().map(|l| l[4].as_str()).collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "a_test.rb",
            "b_test.rb",
            "c_test.rb",
            "d_test.rb",
            "e_spec.rb"
        ]),
        "every file ran once: {lines:?}"
    );
    assert_eq!(files.len(), 5, "and only once: {lines:?}");
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
