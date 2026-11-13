//! The Gemfile evaluator, `shims/gemfile_eval.rb` (#74, docs/package-manager.md,
//! "1. Gemfile evaluation"): the built `spinel` evaluates a real Gemfile as
//! Ruby and prints the dependency JSON documented there. The expected JSON in
//! `fixtures/gemfile_eval/*.json` spells the fixture directory `@ROOT@`. Each
//! fixture's dependency list was checked against Bundler 4.0.20's own
//! `Bundler::Dsl` on the same Gemfile.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// The fixture directory, as `Dir.pwd` and `File.expand_path` spell it:
/// symlinks resolved.
fn fixtures() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gemfile_eval");
    fs::canonicalize(dir).expect("the gemfile_eval fixtures exist")
}

fn shim() -> PathBuf {
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shims/gemfile_eval.rb");
    fs::canonicalize(shim).expect("shims/gemfile_eval.rb exists")
}

/// The shim, run by `spinel run` in `dir`, with only the given variables of
/// the ones the fixtures read set.
fn eval_in(dir: &Path, env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(SPINEL);
    cmd.arg("run")
        .arg(shim())
        .current_dir(dir)
        .env_remove("BUNDLE_GEMFILE")
        .env_remove("GEMFILE_EVAL_REDIS")
        .env_remove("GEMFILE_EVAL_FEATURE");
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("spinel runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `fixtures/gemfile_eval/<name>.json` with `@ROOT@` filled in.
fn expected(name: &str) -> String {
    let text = fs::read_to_string(fixtures().join(format!("{name}.json")))
        .expect("the expected JSON is readable");
    text.replace("@ROOT@", &fixtures().to_string_lossy())
}

fn assert_json(out: &Output, name: &str) {
    assert!(out.status.success(), "stderr: {}", stderr(out));
    assert_eq!(stderr(out), "");
    assert_eq!(stdout(out), expected(name));
}

#[test]
fn every_dsl_method_is_recorded_as_bundler_records_it() {
    let out = eval_in(&fixtures().join("dsl"), &[]);
    assert_json(&out, "dsl");
}

#[test]
fn the_gemfile_is_found_from_a_subdirectory() {
    let out = eval_in(&fixtures().join("dsl/gemfiles"), &[]);
    assert_json(&out, "dsl");
}

#[test]
fn an_env_condition_unset_takes_its_else_branch() {
    let out = eval_in(&fixtures().join("env"), &[]);
    assert_json(&out, "env");
}

#[test]
fn an_env_condition_set_takes_its_then_branch() {
    let out = eval_in(&fixtures().join("env"), &[("GEMFILE_EVAL_REDIS", "1")]);
    assert_json(&out, "env_redis");
}

#[test]
fn bundle_gemfile_names_the_gemfile_wherever_it_runs() {
    let gemfile = fixtures().join("env/Gemfile");
    // From the `dsl` project, whose own Gemfile BUNDLE_GEMFILE overrides.
    let out = eval_in(
        &fixtures().join("dsl"),
        &[("BUNDLE_GEMFILE", &gemfile.to_string_lossy())],
    );
    assert_json(&out, "env");
}

/// A Gemfile of `contents` in a directory of its own, and what the shim
/// makes of it, with the Gemfile's path.
fn eval_contents(name: &str, contents: &str) -> (Output, PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("spinel-gemfile-eval-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the temporary directory is writable");
    let dir = fs::canonicalize(&dir).expect("the temporary directory exists");
    let gemfile = dir.join("Gemfile");
    fs::write(&gemfile, contents).expect("the Gemfile is writable");
    let out = eval_in(&dir, &[]);
    let _ = fs::remove_dir_all(&dir);
    (out, gemfile)
}

/// The shim fails as Bundler does: status 1, Bundler's message, and the
/// Gemfile line that raised it.
fn assert_rejected(name: &str, contents: &str, message: &str, line: usize) {
    let (out, gemfile) = eval_contents(name, contents);
    assert_eq!(out.status.code(), Some(1), "stdout: {}", stdout(&out));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        format!(
            "There was an error parsing `Gemfile`: {message}\n  from {}:{line}\n",
            gemfile.display()
        )
    );
}

#[test]
fn an_undefined_method_is_an_error_at_its_line() {
    assert_rejected(
        "undefined",
        "source \"https://rubygems.org\"\ngem \"rake\"\ngems \"rack\"\n",
        "Undefined local variable or method `gems' for Gemfile",
        3,
    );
}

#[test]
fn one_gem_with_two_requirements_is_an_error() {
    assert_rejected(
        "twice",
        "gem \"rack\", \"~> 3.0\"\ngem \"rack\", \"~> 2.2\"\n",
        "You cannot specify the same gem twice with different version requirements.\n\
         You specified: rack (~> 3.0) and rack (~> 2.2)",
        2,
    );
}

#[test]
fn an_unknown_platform_is_an_error() {
    let (out, _) = eval_contents("platform", "gem \"rack\", platforms: :spinel\n");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with(
            "There was an error parsing `Gemfile`: `spinel` is not a valid platform. \
             The available options are: [:ruby, :ruby_18,"
        ),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn a_second_global_source_is_an_error() {
    assert_rejected(
        "sources",
        "source \"https://rubygems.org\"\nsource \"https://gems.example.com\"\n",
        "This Gemfile contains multiple global sources. Each source after the first \
         must include a block to indicate which gems should come from that source",
        2,
    );
}

#[test]
fn a_gem_listed_twice_alike_is_a_warning() {
    let (out, _) = eval_contents("duplicate", "gem \"rack\"\ngem \"rack\"\n");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "Your Gemfile lists the gem rack (>= 0) more than once.\n\
         You should probably keep only one of them.\n\
         Remove any duplicate entries and specify the gem only once.\n\
         While it's not a problem now, it could cause errors if you change the version of one of them later.\n"
    );
    assert_eq!(stdout(&out).matches("\"name\": \"rack\"").count(), 2);
}
