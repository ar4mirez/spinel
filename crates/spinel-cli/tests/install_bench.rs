//! The install benchmark (#91): `bench/README.md`'s install table is
//! reproduced by `bench/install.sh`, never typed.
//!
//! Measuring needs rubygems.org, so it cannot run here. What can is the half
//! that keeps the table honest: that it is exactly what
//! `bench/install/results.tsv` renders to, that the results measured the
//! lockfile committed beside them, and that the warm install they recorded is
//! sub-second.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `crates/spinel-cli` -> repo root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/spinel-cli sits two levels below the root")
        .to_path_buf()
}

#[test]
fn the_table_is_what_the_script_renders() {
    let out = Command::new("bash")
        .arg(repo_root().join("bench/install.sh"))
        .arg("--check")
        .current_dir(repo_root())
        .output()
        .expect("bash runs");
    assert!(
        out.status.success(),
        "bench/install.sh --check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A warm install makes no request only when the lockfile's `CHECKSUMS` name
/// every gem it fetches; a lockfile without them would measure the index too.
#[test]
fn every_locked_gem_has_a_checksum() {
    let lock = std::fs::read_to_string(repo_root().join("bench/install/rails/Gemfile.lock"))
        .expect("bench/install/rails/Gemfile.lock");
    assert!(
        repo_root().join("bench/install/rails/Gemfile").is_file(),
        "bench/install/rails/Gemfile is missing"
    );
    let section = |name: &str| -> Vec<String> {
        lock.lines()
            .skip_while(|l| *l != name)
            .skip(1)
            .take_while(|l| l.starts_with(' ') || l.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let specs: Vec<String> = section("GEM")
        .iter()
        .filter(|l| l.starts_with("    ") && !l.starts_with("     "))
        .map(|l| l.trim().to_owned())
        .collect();
    assert!(specs.len() > 100, "a Rails lockfile, {} specs", specs.len());
    let checksums = section("CHECKSUMS");
    for spec in &specs {
        assert!(
            checksums
                .iter()
                .any(|l| l.trim().starts_with(&format!("{spec} sha256="))),
            "no CHECKSUMS line for {spec}"
        );
    }
}
