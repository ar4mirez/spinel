//! The Ractor benchmark (#119): `bench/README.md`'s ractors table is
//! reproduced by `bench/ractors.sh`, never typed.
//!
//! Measuring needs CRuby and idle CPUs, so it cannot run here. What can is
//! the half that keeps the table honest — that it is exactly what
//! `bench/ractors/results.tsv` renders to, that it covers no Ractor and 1, 2
//! and 4, and that Spinel's recorded time falls from 1 Ractor to 2 to 4 —
//! and the program itself, run small: cut any way, it answers one checksum.

use std::path::{Path, PathBuf};
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

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
        .arg(repo_root().join("bench/ractors.sh"))
        .arg("--check")
        .current_dir(repo_root())
        .output()
        .expect("bash runs");
    assert!(
        out.status.success(),
        "bench/ractors.sh --check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// No Ractor, and the same work cut into 1, 2 and 4 Ractors, answer the
/// same checksum, which is CRuby 4.0.9's for these 8 units.
#[test]
fn every_cut_of_the_work_answers_one_checksum() {
    for ractors in ["0", "1", "2", "4"] {
        let out = Command::new(SPINEL)
            .arg("run")
            .arg(repo_root().join("bench/ractors/benchmark.rb"))
            .args([ractors, "8"])
            .output()
            .expect("spinel runs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{ractors} Ractors: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.starts_with("checksum: 388364\nms: "),
            "{ractors} Ractors answered {stdout:?}"
        );
    }
}
