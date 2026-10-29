//! What an IO left open does when the program ends, on the `spinel` binary:
//! CRuby finishes each IO's converter when it finalizes the IO, after the
//! `at_exit` blocks and the other finalizers, so an `xml: :attr` quote still
//! open is closed (#456).
//!
//! ruby/spec cannot say this — it is what another process finds afterwards.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn an_xml_attr_quote_left_open_is_closed_at_exit() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/xml_attr_at_exit.rb");
    let dir = std::env::temp_dir().join(format!("spinel-io-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .output()
        .expect("spinel runs");
    let read = |name: &str| std::fs::read(dir.join(name)).expect("the file was written");
    let files = [
        read("a"),
        read("empty"),
        read("utf16"),
        read("closed"),
        read("binmode"),
        read("both"),
    ];
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Measured on ruby 4.0.9: the standard output's quote too, once the
    // `at_exit` blocks have run.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "\"aat_exit\n\"");
    assert_eq!(
        files,
        [
            b"\"&lt;&amp;\"".to_vec(),
            b"".to_vec(),
            b"\"\0a\0\"\0".to_vec(),
            b"\"a\"".to_vec(),
            b"\"a".to_vec(),
            b"\"\"".to_vec(),
        ]
    );
}

/// `exit!` runs nothing, so it finishes nothing: the quote stays open.
/// CRuby loses the `"a` too, still in its write buffer; Spinel has none.
#[test]
fn exit_bang_leaves_an_xml_attr_quote_open() {
    let path = std::env::temp_dir().join(format!("spinel-io-exit-bang-{}", std::process::id()));
    let output = Command::new(SPINEL)
        .arg("-e")
        .arg("io = File.open(ARGV[0], 'w:UTF-8', xml: :attr); io.write('a'); exit!(true)")
        .arg(&path)
        .output()
        .expect("spinel runs");
    let written = std::fs::read(&path).expect("the file was written");
    std::fs::remove_file(&path).ok();
    assert!(output.status.success());
    assert_eq!(written, b"\"a");
}
