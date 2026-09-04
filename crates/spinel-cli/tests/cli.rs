//! Drives the built `spinel` binary the way a user would: spawn it, read stdout.
//!
//! `CLAUDE.md`: tooling work is done when an integration test shells out to the built
//! binary. Phase 0 has no `spinel run` yet, so these cover the only surface that
//! exists — the version banner, help, and exit codes.
//!
//! These live in `spinel-cli` rather than a top-level `tests/` package on purpose.
//! Cargo only sets `CARGO_BIN_EXE_*`, and only guarantees the binary is rebuilt
//! before the test runs, for tests inside the binary's own package. A separate
//! package has no dependency edge to `spinel-cli`, so `cargo test` happily runs the
//! suite against a stale binary from an earlier build. `docs/architecture.md` was
//! updated to match.

use std::process::{Command, Output};

/// The binary this test was built alongside. Cargo rebuilds it first and points this
/// at the exact artifact, so the suite can never test a stale `spinel`.
const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn spinel(args: &[&str]) -> Output {
    Command::new(SPINEL)
        .args(args)
        .output()
        .expect("spinel runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is utf-8")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("stderr is utf-8")
}

#[test]
fn version_prints_engine_language_and_platform() {
    let out = spinel(&["--version"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let banner = stdout(&out);
    let banner = banner.trim();
    assert_eq!(banner.lines().count(), 1, "banner is one line: {banner:?}");

    // `spinel <version> (ruby <version>) [<platform>]`
    let (head, platform) = banner
        .rsplit_once(' ')
        .unwrap_or_else(|| panic!("banner should end with a platform: {banner:?}"));
    let engine_version = head
        .strip_prefix("spinel ")
        .and_then(|rest| rest.split_once(" (ruby "))
        .map(|(version, language)| {
            assert!(
                language.ends_with(')') && language.len() > 1,
                "banner should name the Ruby language version: {banner:?}"
            );
            version
        })
        .unwrap_or_else(|| panic!("banner should read `spinel <ver> (ruby <ver>): {banner:?}"));

    assert_eq!(
        engine_version,
        env!("CARGO_PKG_VERSION"),
        "binary version should match the workspace version"
    );
    assert_eq!(engine_version.split('.').count(), 3, "semver: {banner:?}");
    assert!(
        platform.starts_with('[') && platform.ends_with(']') && platform.len() > 2,
        "banner should name the platform: {banner:?}"
    );
}

#[test]
fn ruby_spelling_of_version_matches() {
    // `ruby -v` is muscle memory; `-V` is the Rust CLI convention. Both must agree.
    let long = spinel(&["--version"]);
    for flag in ["-v", "-V"] {
        let short = spinel(&[flag]);
        assert!(short.status.success(), "{flag} should exit 0");
        assert_eq!(
            stdout(&short),
            stdout(&long),
            "{flag} should match --version"
        );
    }
}

#[test]
fn help_explains_that_nothing_runs_ruby_yet() {
    let out = spinel(&["--help"]);
    assert!(out.status.success());

    let help = stdout(&out);
    assert!(help.contains("Usage:"), "help should show usage: {help}");
    assert!(
        help.to_lowercase().contains("does not run ruby yet"),
        "help should say the surface is not built yet: {help}"
    );
    assert!(
        help.contains("[OPTIONS]"),
        "usage line should show that options exist: {help}"
    );
}

#[test]
fn bare_invocation_is_not_silent_success() {
    // A typo that exits 0 with no output is the worst possible CLI behaviour.
    let out = spinel(&[]);
    assert!(!out.status.success(), "bare `spinel` should not exit 0");
    assert!(
        stderr(&out).contains("Usage:"),
        "bare `spinel` should print help to stderr"
    );
}

#[test]
fn running_a_file_explains_why_it_cannot_yet() {
    // The likeliest first thing a Ruby developer types. It must not read as a
    // parser complaint about an "unexpected argument".
    let out = spinel(&["app.rb"]);
    assert!(!out.status.success(), "should not exit 0");

    let err = stderr(&out);
    assert!(err.contains("app.rb"), "error should name the file: {err}");
    assert!(
        !err.contains("unexpected argument"),
        "error should answer the user, not describe the parser: {err}"
    );
    assert!(
        err.contains("phase 1"),
        "error should say when it will work: {err}"
    );
    assert!(stdout(&out).is_empty(), "errors belong on stderr");
}

#[test]
fn unknown_flag_names_the_flag_and_fails() {
    let out = spinel(&["--frobnicate"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("--frobnicate"), "error should quote it: {err}");
    assert!(
        err.contains("--help"),
        "error should point at --help: {err}"
    );
}
