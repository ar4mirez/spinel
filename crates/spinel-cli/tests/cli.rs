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

// ---------------------------------------------------------------------------
// spinel parse
// ---------------------------------------------------------------------------

/// A path under `tests/fixtures/`, as a string the CLI can take.
fn fixture(relative: &str) -> String {
    format!("{}/tests/fixtures/{relative}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn parse_prints_a_readable_tree() {
    let out = spinel(&["parse", &fixture("parse/greeter.rb")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let tree = stdout(&out);
    // The shape a reader is looking for: one line per node, nesting drawn, and
    // the class and method named rather than spelled as `Debug` field soup.
    assert!(
        tree.starts_with("program"),
        "tree starts at the root: {tree}"
    );
    assert!(tree.contains("class"), "{tree}");
    assert!(tree.contains("def greet"), "{tree}");
    assert!(tree.contains("├─") && tree.contains("└─"), "{tree}");
    assert!(
        tree.contains("assign ||="),
        "the operator survives the assignment fold: {tree}"
    );
    assert!(
        tree.contains("str \"hi\" frozen"),
        "the magic comment reaches the literal: {tree}"
    );
    assert!(
        !tree.contains("ExprKind"),
        "the tree is labelled, not `Debug`-dumped: {tree}"
    );
}

#[test]
fn parse_spans_are_byte_offsets_into_the_file() {
    let out = spinel(&["parse", &fixture("parse/nested/deep.rb")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let tree = stdout(&out);
    // `0xff` is printed in the base it was written in, not re-spelled as 255.
    assert!(tree.contains("int 0xff"), "{tree}");
    assert!(
        tree.lines().any(|line| line.contains("..")),
        "every node carries a span: {tree}"
    );
}

#[test]
fn parse_debug_format_is_still_available() {
    // The tree elides; `--format debug` is the escape hatch for the times a
    // field is missing rather than merely unprinted.
    let out = spinel(&[
        "parse",
        "--format",
        "debug",
        &fixture("parse/nested/deep.rb"),
    ]);
    assert!(out.status.success());
    let dump = stdout(&out);
    assert!(dump.contains("Program {"), "{dump}");
    assert!(dump.contains("Span {"), "every wrapper spelled out: {dump}");
}

#[test]
fn a_syntax_error_is_shown_with_a_caret_and_exits_one() {
    let out = spinel(&["parse", &fixture("parse/broken.rb")]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a syntax error exits 1, the way ruby does"
    );

    let err = stderr(&out);
    assert!(err.contains("broken.rb:1:"), "error names file:line: {err}");
    assert!(err.contains("error:"), "{err}");
    assert!(
        err.contains('^'),
        "the caret is the point of rendering it: {err}"
    );
    assert!(
        err.contains("def broken("),
        "the source line is quoted back: {err}"
    );
    // Prism recovers, so the half-tree is still worth printing.
    assert!(stdout(&out).starts_with("program"), "{}", stdout(&out));
}

#[test]
fn a_missing_file_is_a_usage_error_not_a_syntax_error() {
    let out = spinel(&["parse", &fixture("parse/nope.rb")]);
    assert_eq!(out.status.code(), Some(2), "usage errors exit 2");
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[test]
fn a_directory_sweeps_every_ruby_file_under_it() {
    let out = spinel(&["parse", &fixture("parse")]);
    let report = stdout(&out);

    // Three `.rb` files, one of them deliberately broken, and a `.txt` that is
    // not Ruby and must not be counted.
    assert!(report.contains("3 files"), "{report}");
    assert!(report.contains("0 unhandled"), "{report}");
    assert!(report.contains("1 syntax errors"), "{report}");
    assert!(!report.contains("not_ruby"), "{report}");
    // A sweep names only what failed; a clean file is silence.
    assert!(!report.contains("deep.rb"), "{report}");
    assert!(report.contains("broken.rb"), "{report}");
}

#[test]
fn a_sweep_passes_when_only_the_corpus_is_at_fault() {
    // The distinction the sweep exists for: ruby/spec ships files that are
    // deliberately invalid Ruby, and those are not parser bugs. Only an
    // unhandled node fails the run.
    let out = spinel(&["parse", &fixture("parse")]);
    assert!(
        out.status.success(),
        "syntax errors in the corpus do not fail the sweep: {}",
        stdout(&out)
    );
}

#[test]
fn parse_is_advertised_in_help() {
    let help = stdout(&spinel(&["--help"]));
    assert!(help.contains("parse"), "the one working subcommand: {help}");
    assert!(help.contains("[COMMAND]"), "usage shows it: {help}");
}

#[test]
fn a_mistyped_subcommand_is_not_mistaken_for_a_file() {
    // `spinel pasre app.rb` must not answer as though `pasre` were a Ruby file
    // waiting on a VM. The two mistakes need opposite answers.
    let out = spinel(&["pasre"]);
    assert_eq!(out.status.code(), Some(2));

    let err = stderr(&out);
    assert!(err.contains("unknown subcommand"), "{err}");
    assert!(err.contains("pasre"), "it names what was typed: {err}");
    assert!(err.contains("parse"), "it names what exists: {err}");
    assert!(
        !err.contains("no VM yet"),
        "a typo is not a Ruby file: {err}"
    );
}
