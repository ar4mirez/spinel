//! The single-byte code pages and the locale, on the `spinel` binary (#41):
//! a Windows-1252 file read and written through `File.open`'s converters,
//! and the default external encoding the environment's locale names, as
//! CRuby gives them. Every expectation is ruby 4.0.9's output for the same
//! fixture.

use std::path::PathBuf;
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codepage")
        .join(name)
}

/// The fixture `script` run with `args`, under exactly the locale variables
/// `locale` sets: the test's own environment's are taken away first.
fn run(script: &str, args: &[&std::ffi::OsStr], locale: &[(&str, &str)]) -> Output {
    let mut command = Command::new(SPINEL);
    command.arg(fixture(script)).args(args);
    for name in ["LANG", "LC_ALL", "LC_CTYPE"] {
        command.env_remove(name);
    }
    command.envs(locale.iter().copied());
    let output = command.output().expect("spinel runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn a_windows_1252_file_reads_and_writes_through_its_converters() {
    let dir = std::env::temp_dir().join(format!("spinel-codepage-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let out = dir.join("out");
    let source = fixture("windows1252.txt");
    let output = run(
        "windows1252.rb",
        &[source.as_os_str(), out.as_os_str()],
        &[("LANG", "C.UTF-8")],
    );
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "[\"café € 5\\r\\n“quoted”\\nµs\\n\", #<Encoding:UTF-8>, true]\n",
            "[\"café € 5\\r\\n\", \"“\", #<Encoding:Windows-1252>, #<Encoding:UTF-8>]\n",
            "[\"caf\\xE9 \\x80 5\\r\\n\", #<Encoding:Windows-1252>, \"café € 5\\r\\n\"]\n",
            "\"na\\xEFve \\x80 \\x93x\\x94 \\xB5\\n\"\n",
            "[Encoding::UndefinedConversionError, \"U+3042 from UTF-8 to WINDOWS-1252\", \"UTF-8\", \"WINDOWS-1252\"]\n",
            "[Encoding::UndefinedConversionError, \"\\\"\\\\x81\\\" from WINDOWS-1252 to UTF-8\", \"\\x81\", \"WINDOWS-1252\"]\n",
            "\"café € 5\\n“quoted”\\nµs\\n\"\n",
            "30\n",
        )
    );
}

fn locale_report(locale: &[(&str, &str)]) -> String {
    let source = fixture("windows1252.txt");
    String::from_utf8_lossy(&run("locale.rb", &[source.as_os_str()], locale).stdout).into_owned()
}

/// Under the C or POSIX locale, or none, CRuby's default external encoding
/// is US-ASCII, and so is what `File.read` returns, `ARGV` and `ENV`;
/// `Dir.pwd` is BINARY. A file opened with its own encodings is unchanged.
#[test]
fn the_c_locale_is_us_ascii() {
    let c = concat!(
        "[#<Encoding:US-ASCII>, #<Encoding:US-ASCII>, #<Encoding:US-ASCII>, \"ANSI_X3.4-1968\"]\n",
        "[#<Encoding:US-ASCII>, false, #<Encoding:US-ASCII>, #<Encoding:US-ASCII>, #<Encoding:BINARY (ASCII-8BIT)>]\n",
        "[\"locale\", \"external\", \"filesystem\"]\n",
        "#<Encoding:UTF-8>\n",
    );
    assert_eq!(locale_report(&[("LANG", "C")]), c);
    assert_eq!(locale_report(&[("LANG", "POSIX")]), c);
    assert_eq!(locale_report(&[]), c);
    assert_eq!(locale_report(&[("LANG", "C.UTF-8"), ("LC_ALL", "C")]), c);
}

/// Under a UTF-8 locale nothing changes: everything is UTF-8, as before.
#[test]
fn a_utf8_locale_is_utf8() {
    let utf8 = concat!(
        "[#<Encoding:UTF-8>, #<Encoding:UTF-8>, #<Encoding:UTF-8>, \"UTF-8\"]\n",
        "[#<Encoding:UTF-8>, false, #<Encoding:UTF-8>, #<Encoding:UTF-8>, #<Encoding:UTF-8>]\n",
        "[\"locale\", \"external\", \"filesystem\"]\n",
        "#<Encoding:UTF-8>\n",
    );
    assert_eq!(locale_report(&[("LANG", "C.UTF-8")]), utf8);
    assert_eq!(
        locale_report(&[("LANG", "C"), ("LC_CTYPE", "C.UTF-8")]),
        utf8
    );
}
