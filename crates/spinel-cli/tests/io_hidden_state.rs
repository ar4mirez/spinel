//! IO, File and socket state out of reflection's sight, as on CRuby, whose
//! IO keeps its state in the `T_FILE` struct and whose `File::Stat` and
//! `Addrinfo` are `T_DATA`: `instance_variables` of every one is `[]`.
//! Spinel keeps that state in Ruby, in `core/io.rb`, `core/file.rb` and
//! `lib/socket.rb`, spelled `@__name__`, which `instance_variables` leaves
//! out. Psych dumps an object by its instance variables, so a `File` dumps
//! as CRuby's does, `--- !ruby/object:File {}`.
//! Marshal cannot write a `File::Stat` or a `Socket::Ifaddr` and says so as
//! CRuby does, while a `Socket::Option` and a `Socket::AncillaryData` still
//! come back whole, as does a `Socket::ResolutionError`'s `error_code`.
//!
//! `fixtures/io_hidden_state/` holds the scripts and what ruby 4.0.9 printed
//! for them. Everything runs offline; the sockets are loopback, and a
//! machine that cannot open one skips that test.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io_hidden_state")
}

/// Runs `name.rb` from the fixtures directory, where `__FILE__` is the bare
/// name, as it was when ruby wrote `name.out`.
fn run(name: &str) -> String {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(format!("{name}.rb"))
        .current_dir(fixtures())
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(fixtures().join(format!("{name}.out")))
        .unwrap_or_else(|error| panic!("{name}.out is readable: {error}"))
}

#[test]
fn file_pipe_stdout_and_stat_have_no_instance_variables() {
    assert_eq!(run("state"), expected("state"));
}

#[test]
fn sockets_and_addrinfo_have_no_instance_variables() {
    let actual = run("sockets");
    if actual.starts_with("unavailable") {
        eprintln!("skipped: no loopback socket here ({})", actual.trim_end());
        return;
    }
    assert_eq!(actual, expected("sockets"));
}
