//! The vendored stdlib at runtime (#47): `require` finds it with no network
//! and no gem install, extracted once to `$SPINEL_HOME/stdlib/`, and first
//! runs that race each other all succeed and leave one copy.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stdlib")
        .join(name)
}

/// A `SPINEL_HOME` no run has used yet.
fn fresh_home(test: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let home = std::env::temp_dir().join(format!(
        "spinel-stdlib-{test}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    home
}

fn run(home: &Path, script: &Path) -> Output {
    Command::new(SPINEL)
        .arg("run")
        .arg(script)
        .env("SPINEL_HOME", home)
        .output()
        .expect("spinel runs")
}

/// The copies under `home`, and anything a run left half written.
fn entries(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home.join("stdlib"))
        .expect("the stdlib directory exists")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

const EXPECTED: &str = "192.168.1.0\n{verbose: true}\ntrue\n";

#[test]
fn require_finds_the_vendored_stdlib() {
    let home = fresh_home("require");
    for _ in 0..2 {
        let output = run(&home, &fixture("uses_stdlib.rb"));
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
    }
    // The second run reused the first one's copy.
    let copies = entries(&home);
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert!(
        copies[0].starts_with(env!("CARGO_PKG_VERSION")),
        "{copies:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn racing_first_runs_all_succeed_and_leave_one_copy() {
    let home = fresh_home("race");
    let children: Vec<_> = (0..8)
        .map(|_| {
            Command::new(SPINEL)
                .arg("run")
                .arg(fixture("uses_stdlib.rb"))
                .env("SPINEL_HOME", &home)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spinel starts")
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().expect("spinel finishes");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
    }
    let copies = entries(&home);
    assert_eq!(copies.len(), 1, "one copy and no staging left: {copies:?}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_home_that_cannot_be_written_only_loses_the_stdlib() {
    // A regular file where the directory would go: nothing can be created
    // under it, and a program that needs no stdlib still runs.
    let home = fresh_home("unwritable");
    std::fs::write(&home, "not a directory").expect("the temp dir is writable");
    let output = Command::new(SPINEL)
        .args([
            "-e",
            "p $LOAD_PATH.size; begin; require 'ipaddr'; rescue LoadError => e; p e.class; end",
        ])
        .env("SPINEL_HOME", &home)
        .output()
        .expect("spinel runs");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0\nLoadError\n");
    let _ = std::fs::remove_file(&home);
}
