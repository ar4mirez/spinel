//! `io/console` on a terminal, on the `spinel` binary (#110): the window size,
//! raw, cooked and no-echo modes and their round trips, `getch`, `getpass`,
//! `ttyname`, `IO.console` and the cursor query. Measured on ruby 4.0.9.
//!
//! ruby/spec has no `library/io/console`, and a pipe is no terminal:
//! util-linux's `script` gives the program one, fed from this test. A mode is
//! read back with `stty -a`, so the checks hold whatever modes a fresh
//! pseudo-terminal starts in.
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
        .join("tests/fixtures/io_console")
        .join(fixture);
    let base = std::env::temp_dir().join(format!(
        "spinel-io-console-{}-{}",
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
        if started.elapsed() > Duration::from_secs(60) {
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
fn modes_window_size_and_reads_match_ruby() {
    let (answer, _) = on_a_terminal("modes.rb", b"abc\nsecret\nhidden\n");
    let expected = r#"winsize=: [30, 100]
winsize: [[30, 100], "30 100"]
winsize= 4: [[31, 101], [31, 101]]
winsize= wraps: [1, 65535]
winsize= 3: [ArgumentError, "wrong number of arguments (given 3, expected 2 or 4)"]
echo?: true
echo= false: [false, [true, true, true, true]]
echo= true: [true, [true, true, true, true]]
raw: [true, [true, true, true, true, true, true, true, true, true, true, true], "1", "0"]
restored: true
raw min time: ["0", "15"]
raw min wraps: ["44", "0"]
raw intr: [true, true, true, true]
raw intr bad: [ArgumentError, "true or false expected as intr: 1"]
raw raises: ["boom", true]
raw no block: ["no block given", true]
cooked: [true, true, true, true, true, true]
noecho: [false, true]
raw!: [true, [true], "4"]
console_mode=: true
cooked!: [true, true]
mode raw: [[true], "2"]
mode raw!: [true, true]
mode dup: true
mode= type: [TypeError, "wrong argument type nil (expected console-mode)"]
getch: ["a", "b", "c", true]
getpass: ["", "secret", true]
ttyname: [true, true, #<Encoding:US-ASCII>, true]
console: [File, "/dev/tty", true, true, true, true]
console send: [1, 65535]
console type: ["wrong argument type nil (expected Symbol)", "wrong argument type false (expected Symbol)", "wrong argument type String (expected Symbol)"]
console close: [nil, true, false]
flushes: true
final: true
"#;
    assert_eq!(answer, expected);
}

#[test]
fn the_cursor_is_asked_for_and_moved_with_vt100_sequences() {
    let (answer, terminal) = on_a_terminal("cursor.rb", b"\x1b[12;34R");
    assert_eq!(answer, "[11, 33]");
    let written = "\x1b[6n\x1b[3;4H\x1b[1;1H\x1b[2A\x1b[1B\x1b[3D\x1b[4C\x1b[0K\x1b[1J\x1b[1S\x1b[1T\x1b[2J\x1b[1;1H\x1b[6G";
    assert!(terminal.contains(written), "{terminal:?}");
}
