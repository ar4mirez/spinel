//! Guards the vendored pure-Ruby stdlib in `stdlib/`.
//!
//! The authoritative check that `stdlib/` matches upstream is the `stdlib drift`
//! CI job, which re-fetches the pinned tag and diffs. It needs the network, so it
//! cannot live here. What this suite covers is everything that can go wrong
//! without the network: the tree missing, the licenses dropped, the pin in
//! `scripts/vendor-stdlib.sh` bumped without re-vendoring, and — the reason the
//! corpus is in the tree at all — a file in it that `spinel-parse` cannot lower.

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

fn stdlib() -> PathBuf {
    repo_root().join("stdlib")
}

#[test]
fn the_stdlib_is_vendored() {
    let stdlib = stdlib();
    assert!(
        stdlib.is_dir(),
        "stdlib/ is missing; run scripts/vendor-stdlib.sh"
    );

    // `stdlib/` is upstream `lib/` flattened, so it is a $LOAD_PATH root: these
    // are the paths `require "erb"` and `require "net/http"` will resolve to.
    for feature in ["erb.rb", "fileutils.rb", "optparse.rb", "net/http.rb"] {
        assert!(
            stdlib.join(feature).is_file(),
            "stdlib/ should be a $LOAD_PATH root, so stdlib/{feature} should exist"
        );
    }

    // Set became a core class in Ruby 4.0 and is no longer shipped as a file.
    // Asserted so that a future upstream bump that reintroduces it is noticed
    // here rather than silently changing what `require "set"` means.
    assert!(
        !stdlib.join("set.rb").is_file(),
        "Set is core in Ruby 4.0; stdlib/set.rb reappearing is a change worth reading"
    );
}

#[test]
fn upstream_licenses_are_preserved() {
    // Ruby's own license, its Japanese text, the 2-clause BSDL it dual-licenses
    // under, and LEGAL, which records the per-file terms for the files in `lib/`
    // that are under neither.
    for file in ["COPYING", "COPYING.ja", "BSDL", "LEGAL"] {
        let path = stdlib().join("LICENSE").join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("stdlib/LICENSE/{file} should be readable: {err}"));
        assert!(
            !text.trim().is_empty(),
            "stdlib/LICENSE/{file} should not be empty"
        );
    }
}

#[test]
fn the_recorded_pin_matches_the_script() {
    let script = std::fs::read_to_string(repo_root().join("scripts/vendor-stdlib.sh"))
        .expect("the vendoring script should be readable");
    let pinned = script
        .lines()
        .find_map(|line| line.strip_prefix("RUBY_TAG=\""))
        .and_then(|rest| rest.split('"').next())
        .expect("scripts/vendor-stdlib.sh should pin RUBY_TAG");

    let upstream = std::fs::read_to_string(stdlib().join("UPSTREAM"))
        .expect("stdlib/UPSTREAM should record the pin");
    let recorded = upstream
        .lines()
        .find_map(|line| line.trim().strip_prefix("tag "))
        .map(str::trim)
        .expect("stdlib/UPSTREAM should name a tag");

    // Bumping the script without re-running it would leave CI's drift job to
    // catch it, but only on a machine with a network. This catches it locally.
    assert_eq!(
        recorded, pinned,
        "stdlib/ was vendored from {recorded} but the script now pins {pinned}; \
         run scripts/vendor-stdlib.sh"
    );
}

#[test]
fn every_vendored_file_lowers_to_spinel_ast() {
    // The reason the corpus is worth having in the tree: it is 600-odd files of
    // real Ruby that the lowering has to keep handling. An unhandled node is a
    // bug in spinel-parse and fails the sweep; a syntax error would be a
    // property of the corpus, and upstream ships none.
    let out = Command::new(SPINEL)
        .args(["parse", "stdlib"])
        .current_dir(repo_root())
        .output()
        .expect("spinel runs");

    let report = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdlib/ should lower with no unhandled nodes:\n{report}"
    );

    let summary = report
        .lines()
        .last()
        .expect("the sweep prints a summary line");
    assert!(
        summary.contains("0 unhandled") && summary.contains("0 syntax errors"),
        "the vendored stdlib should be clean: {summary}"
    );
    // Guards against the sweep passing because it swept nothing.
    let files: usize = summary
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert!(
        files > 500,
        "expected the whole stdlib, swept {files} files"
    );
}
