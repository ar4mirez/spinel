//! How far a read through an IO's stateful converter goes on a long file, on
//! the `spinel` binary: CRuby's call is given 128KB and has 128KB of room, so
//! `pos` after a `getc` is where those run out, whatever read filled the
//! buffer first (#513).
//!
//! ruby/spec has no example for it, and it is past `eval.txt`'s instruction
//! budget, so the check is here.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn one_read_goes_as_far_as_cruby_buffers() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/read_buffer.rb");
    let dir = std::env::temp_dir().join(format!("spinel-io-read-buffer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .output()
        .expect("spinel runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Measured on ruby 4.0.9.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "66000\n43694\n66000\n"
    );
}
