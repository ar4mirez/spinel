//! The yjit-bench subset (#63): `bench/README.md`'s table is reproduced by
//! `scripts/bench.sh`, never typed.
//!
//! Measuring needs CRuby and minutes of quiet CPU, so it cannot run here. What
//! can is the half that keeps the table honest: that it is exactly what
//! `bench/yjit-bench/results.tsv` renders to, that the results cover every
//! vendored benchmark and nothing else, and that the vendored programs and
//! their licences are present and parse.

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

const BENCHMARKS: [&str; 6] = [
    "fib.rb",
    "getivar.rb",
    "keyword_args.rb",
    "str_concat.rb",
    "nbody/benchmark.rb",
    "binarytrees/benchmark.rb",
];

#[test]
fn the_table_is_what_the_script_renders() {
    let out = Command::new("bash")
        .arg(repo_root().join("scripts/bench.sh"))
        .arg("--yjit-bench-check")
        .current_dir(repo_root())
        .output()
        .expect("bash runs");
    assert!(
        out.status.success(),
        "scripts/bench.sh --yjit-bench-check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_vendored_benchmarks_and_licences_are_present_and_parse() {
    let dir = repo_root().join("bench/yjit-bench");
    for licence in [
        "LICENSE.md",
        "benchmarks/nbody/LICENSE",
        "benchmarks/binarytrees/LICENSE",
    ] {
        assert!(
            dir.join(licence).is_file(),
            "bench/yjit-bench/{licence} is missing"
        );
    }
    for bench in BENCHMARKS {
        let path = dir.join("benchmarks").join(bench);
        let out = Command::new(SPINEL)
            .arg("-c")
            .arg(&path)
            .output()
            .expect("spinel runs");
        assert!(
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("Syntax OK"),
            "spinel -c {}: {}{}",
            path.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
