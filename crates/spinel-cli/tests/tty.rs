//! What a terminal keeps, on the `spinel` binary: a tty cannot seek, and CRuby
//! marks it `FMODE_DUPLEX` when it opens, so a write leaves what was read ahead
//! (#497), and has no write buffer for a close from a `to_s` to flush (#506).
//! Measured on ruby 4.0.9.
//!
//! ruby/spec cannot say this, and `eval.txt` has no terminal: util-linux's
//! `script` gives the program one, fed from this test.
#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// Runs a fixture on a terminal fed `input`, and gives what it left in the file
/// named by its argument and what reached the terminal.
fn on_a_terminal(fixture: &str, input: &[u8]) -> (String, String) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/io")
        .join(fixture);
    let base = std::env::temp_dir().join(format!(
        "spinel-tty-{}-{}",
        std::process::id(),
        fixture.file_stem().expect("a name").to_string_lossy()
    ));
    let out = base.with_extension("answer");
    let typescript = base.with_extension("typescript");
    let command = format!("'{SPINEL}' run '{}' '{}'", fixture.display(), out.display());
    let mut script = Command::new("script")
        .args(["-qec", &command])
        .arg(&typescript)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("util-linux's script runs");
    script
        .stdin
        .take()
        .expect("its standard input")
        .write_all(input)
        .expect("the terminal takes the input");
    let started = Instant::now();
    while script.try_wait().expect("script is waited for").is_none() {
        if started.elapsed() > Duration::from_secs(30) {
            script.kill().ok();
            panic!("the program did not finish");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let answer = std::fs::read_to_string(&out).expect("the program wrote its answer");
    let terminal =
        String::from_utf8_lossy(&std::fs::read(&typescript).expect("the typescript")).into_owned();
    std::fs::remove_file(&out).ok();
    std::fs::remove_file(&typescript).ok();
    (answer, terminal)
}

#[test]
fn a_write_to_a_tty_leaves_what_was_read_ahead() {
    let (answer, _) = on_a_terminal("tty_read_ahead.rb", b"hello\nworld\n");
    assert_eq!(answer, r#"["h", "ello\n"]"#);
}

#[test]
fn a_to_s_that_closes_a_tty_leaves_the_write_nothing_to_flush() {
    let (answer, terminal) = on_a_terminal("tty_closed_by_to_s.rb", b"");
    assert_eq!(answer, r#"[IOError, "closed stream"]"#);
    assert!(!terminal.contains("gathered"), "{terminal:?}");
    assert!(terminal.contains("all of it"), "{terminal:?}");
}
