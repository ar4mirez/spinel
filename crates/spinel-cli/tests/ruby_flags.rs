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
fn frozen_string_literal_flags_settle_what_literals_are() {
    let chilled = run(&["-W:deprecated", "literals.rb"], "");
    assert!(chilled.status.success());
    assert_eq!(
        String::from_utf8_lossy(&chilled.stdout),
        "[false, false, false]\n"
    );
    assert!(
        String::from_utf8_lossy(&chilled.stderr)
            .contains("literal string will be frozen in the future")
    );
    for disable in [
        "--disable-frozen-string-literal",
        "--disable=frozen-string-literal",
    ] {
        let plain = run(&["-W:deprecated", disable, "literals.rb"], "");
        assert!(plain.status.success());
        assert_eq!(
            String::from_utf8_lossy(&plain.stdout),
            "[false, true, true]\n"
        );
        assert_eq!(String::from_utf8_lossy(&plain.stderr), "");
    }
    assert_eq!(
        out(&["--enable=frozen-string-literal", "literals.rb"]),
        "[true, false, false]\n"
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

fn run_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(SPINEL)
        .args(args)
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .envs(env.iter().copied())
        .current_dir(fixtures())
        .output()
        .expect("spinel runs")
}

fn out_env(args: &[&str], env: &[(&str, &str)]) -> String {
    let output = Command::new(SPINEL)
        .args(args)
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .envs(env.iter().copied())
        .current_dir(fixtures())
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

#[test]
fn rubyopt_adds_switches_after_the_command_line() {
    assert_eq!(
        out_env(&["-e", "p $VERBOSE"], &[("RUBYOPT", "-w")]),
        "true\n"
    );
    assert_eq!(
        out_env(&["-e", "p $VERBOSE"], &[("RUBYOPT", "w")]),
        "true\n"
    );
    let refused = run_env(&["-e", "1"], &[("RUBYOPT", "-a")]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("invalid switch in RUBYOPT: -a (RuntimeError)")
    );
}

#[test]
fn rubylib_comes_after_the_include_flags() {
    assert_eq!(
        out_env(
            &["-I/i", "-e", "p $LOAD_PATH.first(2)"],
            &[("RUBYLIB", "/l")]
        ),
        "[\"/i\", \"/l\"]\n"
    );
}

#[test]
fn encodings() {
    assert_eq!(
        out(&["-Ke", "-e", "p __ENCODING__"]),
        "#<Encoding:EUC-JP>\n"
    );
    assert_eq!(
        out(&["-U", "-e", "p Encoding.default_internal"]),
        "#<Encoding:UTF-8>\n"
    );
    assert_eq!(
        out(&["--encoding", "ascii", "-e", "p Encoding.default_external"]),
        "#<Encoding:US-ASCII>\n"
    );
    let clash = run(&["-Eascii:ascii", "-U", "-e", "1"], "");
    assert_eq!(clash.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&clash.stderr)
            .contains("default_internal already set to ascii (RuntimeError)")
    );
}

#[test]
fn switches_that_are_globals() {
    assert_eq!(
        out(&["-a", "-l", "-e", "p [$-a, $-l, $-p]"]),
        "[true, true, false]\n"
    );
    assert_eq!(
        out(&["-d", "-e", "p [$-d, $DEBUG, $VERBOSE]"]),
        "[true, true, true]\n"
    );
}

#[test]
fn directories_and_the_path() {
    assert_eq!(
        out(&["-Xlib", "-e", "p Dir.pwd.end_with?(%q(lib))"]),
        "true\n"
    );
    assert_eq!(
        out_env(&["-S", "searched.rb"], &[("RUBYPATH", "lib")]),
        "found on the path\n"
    );
    // A `#!` line for something other than ruby is skipped to the ruby one.
    assert_eq!(out(&["launcher.sh"]), "the ruby part\n");
}
