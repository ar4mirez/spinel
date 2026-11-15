//! What `ungetc` and `ungetbyte` give back on an IO with no read converter,
//! read by the short path (#41): bytes in front of the read buffer, which
//! `gets`, `each_line` and `read(n)` take with the encoding and line numbers
//! CRuby gives them.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[cfg(not(target_os = "macos"))]
const UTF8_LOCALE: &str = "C.UTF-8";
#[cfg(target_os = "macos")]
const UTF8_LOCALE: &str = "en_US.UTF-8";

#[test]
fn pushed_back_bytes_read_as_cruby_reads_them() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/ungetc_plain.rb");
    let dir = std::env::temp_dir().join(format!("spinel-io-ungetc-plain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .env_remove("LANG")
        .env_remove("LC_CTYPE")
        .env("LC_ALL", UTF8_LOCALE)
        .output()
        .expect("spinel runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Measured on ruby 4.0.9.
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}

const EXPECTED: &str = r#"getc: "h"
gets after ungetc(getc): "hello\n" #<Encoding:UTF-8>
lineno: 1
$.: 1
gets: "wörld\n" #<Encoding:UTF-8>
gets: "last" #<Encoding:UTF-8>
gets at end: nil nil
gets after ungetc(é): "éhello\n" #<Encoding:UTF-8>
each_line after ungetc(éxy): "éxywörld\n" #<Encoding:UTF-8>
each_line after ungetc(éxy): "last" #<Encoding:UTF-8>
lineno: 3
gets after ungetc(0x41): "Ahello\n" #<Encoding:UTF-8>
gets after ungetc(0xe9): "éwörld\n" #<Encoding:UTF-8>
gets after ungetbyte(0x5a): "Zhello\n" #<Encoding:UTF-8>
read(2) after ungetbyte(pq\n): "pq"
gets: "\n" #<Encoding:UTF-8>
gets after ungetbyte(\xC3): "\xC3wörld\n" #<Encoding:UTF-8>
each_line after ungetbyte(r\ns): "r\n" #<Encoding:UTF-8>
each_line after ungetbyte(r\ns): "slast" #<Encoding:UTF-8>
lineno: 5
gets after getc, ungetc(abc\n): "abc\n" #<Encoding:UTF-8>
read(3): "ell"
gets: "o\n" #<Encoding:UTF-8>
lineno: 2
read(2) before any read: "xy"
gets: "\n" #<Encoding:UTF-8>
gets: "zhello\n" #<Encoding:UTF-8>
read(4): "w\xC3\xB6r"
each_line: "ld\n" #<Encoding:UTF-8>
each_line: "last" #<Encoding:UTF-8>
lineno: 4
$.: 4
each_line after ungetc(q\n * 3): "q\n" #<Encoding:UTF-8>
each_line after ungetc(q\n * 3): "q\n" #<Encoding:UTF-8>
each_line after ungetc(q\n * 3): "q\n" #<Encoding:UTF-8>
each_line after ungetc(q\n * 3): "hello\n" #<Encoding:UTF-8>
each_line after ungetc(q\n * 3): "wörld\n" #<Encoding:UTF-8>
each_line after ungetc(q\n * 3): "last" #<Encoding:UTF-8>
lineno: 6
gets ISO-8859-1: "ab\n" #<Encoding:ISO-8859-1>
gets ISO-8859-1: "hello\n" #<Encoding:ISO-8859-1>
"#;
