//! What a terminal keeps, on the `spinel` binary: a tty cannot seek, and CRuby
//! marks it `FMODE_DUPLEX` when it opens, so a write leaves what was read ahead
//! (#497). Measured on ruby 4.0.9.
//!
//! ruby/spec cannot say this, and `eval.txt` has no terminal: util-linux's
//! `script` gives the program one, fed from this test.
#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn a_write_to_a_tty_leaves_what_was_read_ahead() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/tty_read_ahead.rb");
    let out = std::env::temp_dir().join(format!("spinel-tty-{}", std::process::id()));
    let command = format!("'{SPINEL}' run '{}' '{}'", fixture.display(), out.display());
    let mut script = Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("util-linux's script runs");
    script
        .stdin
        .take()
        .expect("its standard input")
        .write_all(b"hello\nworld\n")
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
    std::fs::remove_file(&out).ok();
    assert_eq!(answer, r#"["h", "ello\n"]"#);
}
