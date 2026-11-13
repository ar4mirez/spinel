//! A project's `.ruby-version` and Gemfile `ruby` directive (#73,
//! docs/cli.md, "Language version"): a Ruby this build does not implement is a
//! warning and the program still runs; `--strict-ruby-version` makes it an
//! error; a Ruby it does implement, and what `spinel init` writes, say nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruby_version")
        .join(name)
}

fn spinel_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(SPINEL)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spinel runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_newer_cruby_in_ruby_version_warns_and_runs() {
    let out = spinel_in(&fixture("newer_cruby"), &["run", "main.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "ran\n");
    let err = stderr(&out);
    assert!(err.contains("warning:"), "stderr: {err}");
    assert!(err.contains(".ruby-version"), "stderr: {err}");
    assert!(err.contains("Ruby 99.0.0"), "stderr: {err}");
}

#[test]
fn strict_ruby_version_makes_the_mismatch_an_error() {
    let out = spinel_in(
        &fixture("newer_cruby"),
        &["run", "--strict-ruby-version", "main.rb"],
    );
    assert!(
        !out.status.success(),
        "a mismatch under --strict-ruby-version must fail"
    );
    assert_eq!(stdout(&out), "", "the program must not run");
    let err = stderr(&out);
    assert!(!err.contains("warning:"), "stderr: {err}");
    assert!(err.contains("--strict-ruby-version"), "stderr: {err}");
    assert!(err.contains("Ruby 99.0.0"), "stderr: {err}");
}

#[test]
fn a_newer_ruby_directive_in_the_gemfile_warns_and_strict_fails() {
    let dir = fixture("newer_gemfile");
    let out = spinel_in(&dir, &["run", "main.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "ran\n");
    let err = stderr(&out);
    assert!(
        err.contains("warning:") && err.contains("Gemfile"),
        "stderr: {err}"
    );
    assert!(err.contains("Ruby 99.0"), "stderr: {err}");

    let out = spinel_in(&dir, &["run", "--strict-ruby-version", "main.rb"]);
    assert!(!out.status.success());
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("Gemfile"), "stderr: {}", stderr(&out));
}

#[test]
fn the_file_is_found_from_a_subdirectory() {
    let out = spinel_in(&fixture("newer_cruby").join("app"), &["run", "main.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains(".ruby-version"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn a_bare_file_reads_the_project_ruby_as_run_does() {
    let out = spinel_in(&fixture("newer_cruby"), &["main.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "ran\n");
    assert!(
        stderr(&out).contains("warning:"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn a_satisfied_cruby_version_says_nothing_even_when_strict() {
    for args in [
        &["run", "main.rb"][..],
        &["run", "--strict-ruby-version", "main.rb"][..],
    ] {
        let out = spinel_in(&fixture("satisfied"), args);
        assert!(out.status.success(), "stderr: {}", stderr(&out));
        assert_eq!(stdout(&out), "ran\n");
        assert_eq!(stderr(&out), "", "{args:?}");
    }
}

/// Versions compare as RubyGems compares them (`Gem::Version`, #75): a
/// prerelease of Ruby 4.0.0, written either way RubyGems reads one, comes
/// before 4.0.0, so this build meets it.
#[test]
fn a_prerelease_of_the_implemented_ruby_says_nothing_even_when_strict() {
    let out = spinel_in(
        &fixture("prerelease"),
        &["run", "--strict-ruby-version", "main.rb"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "ran\n");
    assert_eq!(stderr(&out), "");
}

/// Segments are numbers of any size, as in RubyGems: a request for
/// `3.99999999999999999999999` is still before 4.0.0, so this build meets it.
#[test]
fn a_segment_past_u64_is_compared_not_rejected_even_when_strict() {
    let out = spinel_in(
        &fixture("big_segment"),
        &["run", "--strict-ruby-version", "main.rb"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "ran\n");
    assert_eq!(stderr(&out), "");
}

#[test]
fn what_spinel_init_writes_says_nothing_even_when_strict() {
    let dir = std::env::temp_dir().join(format!("spinel-ruby-version-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    let init = spinel_in(&dir, &["init"]);
    assert!(init.status.success(), "init: {}", stderr(&init));
    fs::write(dir.join("main.rb"), "puts \"ran\"\n").expect("write main.rb");

    let out = spinel_in(&dir, &["run", "--strict-ruby-version", "main.rb"]);
    let _ = fs::remove_dir_all(&dir);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "ran\n");
    assert_eq!(stderr(&out), "");
}
