//! `ruby`'s command line on the `spinel` binary (#62): what scripts and
//! gems that shell out to `RUBY_EXE` pass, run the way `ruby` runs it.
//!
//! Each expectation was measured on ruby 4.0.6 with the same arguments.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ruby_flags")
}

fn run(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(SPINEL)
        .args(args)
        .current_dir(fixtures())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("stdin takes the input");
    child.wait_with_output().expect("spinel finishes")
}

fn out(args: &[&str]) -> String {
    out_with(args, "")
}

fn out_with(args: &[&str], input: &str) -> String {
    let output = run(args, input);
    assert!(
        output.status.success(),
        "spinel {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

#[test]
fn dash_e_runs_code_as_the_program_named_dash_e() {
    assert_eq!(out(&["-e", "p 1 + 1", "-e", "p $0"]), "2\n\"-e\"\n");
}

#[test]
fn arguments_after_the_program_are_argv() {
    assert_eq!(out(&["-e", "p ARGV", "a", "-w"]), "[\"a\", \"-w\"]\n");
    assert_eq!(out(&["args.rb", "x"]), "[\"args.rb\", [\"x\"]]\n");
    assert_eq!(out(&["-w", "args.rb", "-w"]), "[\"args.rb\", [\"-w\"]]\n");
}

#[test]
fn warning_levels_set_verbose() {
    assert_eq!(out(&["-W0", "-e", "p $VERBOSE"]), "nil\n");
    assert_eq!(out(&["-W1", "-e", "p $VERBOSE"]), "false\n");
    assert_eq!(out(&["-w", "-e", "p $VERBOSE"]), "true\n");
    assert_eq!(out(&["-e", "p $VERBOSE"]), "false\n");
}

#[test]
fn load_path_and_requires() {
    assert_eq!(
        out(&["-Ilib", "-rgreeting", "-e", "puts Greeting.hello"]),
        "hello from lib\n"
    );
    assert_eq!(
        out(&["-I", "lib", "-r", "greeting", "-e", "puts Greeting.hello"]),
        "hello from lib\n"
    );
}

#[test]
fn line_loops() {
    assert_eq!(
        out(&["-ne", "print $_.upcase", "lines.txt"]),
        "ONE TWO\nTHREE FOUR\n"
    );
    assert_eq!(out_with(&["-pe", "$_ = $_.tr('o', '0')"], "foo\n"), "f00\n");
    assert_eq!(
        out(&["-lane", "p $F", "lines.txt"]),
        "[\"one\", \"two\"]\n[\"three\", \"four\"]\n"
    );
}

#[test]
fn switches_become_globals() {
    assert_eq!(
        out(&[
            "-s",
            "-e",
            "p [$flag, $name, ARGV]",
            "--",
            "-flag",
            "-name=x",
            "rest"
        ]),
        "[true, \"x\", [\"rest\"]]\n"
    );
}

#[test]
fn a_program_on_standard_input() {
    assert_eq!(out_with(&["-"], "puts :stdin\n"), "stdin\n");
    assert_eq!(out_with(&["-w"], "p $VERBOSE\n"), "true\n");
}

#[test]
fn check_syntax_only() {
    assert_eq!(out(&["-c", "-e", "def x = 1"]), "Syntax OK\n");
    assert!(!run(&["-c", "-e", "def ("], "").status.success());
}

#[test]
fn skip_to_the_ruby_line() {
    assert_eq!(out(&["-x", "embedded.txt"]), "found the script\n");
}

#[test]
fn record_separator() {
    assert_eq!(out(&["-072", "-e", "p $/"]), "\":\"\n");
    assert_eq!(out(&["-00", "-e", "p $/"]), "\"\"\n");
}

#[test]
fn features_spinel_does_not_have_are_accepted() {
    assert_eq!(
        out(&["--disable-gems", "--disable=did_you_mean", "-e", "puts :ok"]),
        "ok\n"
    );
}

#[test]
fn a_missing_script_is_a_load_error() {
    let output = run(&["missing.rb"], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("No such file or directory -- missing.rb (LoadError)")
    );
}

#[test]
fn an_unknown_flag_is_refused() {
    let output = run(&["-q"], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid option -q"));
}
