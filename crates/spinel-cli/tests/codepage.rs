//! The single-byte code pages and the locale, on the `spinel` binary (#41):
//! a Windows-1252 file read and written through `File.open`'s converters,
//! and the default external encoding the environment's locale names, as
//! CRuby gives them. Every expectation is ruby 4.0.9's output for the same
//! fixture.

use std::path::PathBuf;
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// A UTF-8 locale the machine has: glibc's `C.UTF-8`, or `en_US.UTF-8` on
/// macOS, which has no `C.UTF-8`.
#[cfg(not(target_os = "macos"))]
const UTF8_LOCALE: &str = "C.UTF-8";
#[cfg(target_os = "macos")]
const UTF8_LOCALE: &str = "en_US.UTF-8";

/// What `nl_langinfo(CODESET)` names the C locale's codeset: glibc's
/// `ANSI_X3.4-1968`, macOS's `US-ASCII`.
#[cfg(not(target_os = "macos"))]
const C_CHARMAP: &str = "ANSI_X3.4-1968";
#[cfg(target_os = "macos")]
const C_CHARMAP: &str = "US-ASCII";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codepage")
        .join(name)
}

/// The fixture `script` run with `flags` before it and `args` after, under
/// exactly the locale variables `locale` sets: the test's own environment's
/// are taken away first.
fn run(flags: &[&str], script: &str, args: &[&std::ffi::OsStr], locale: &[(&str, &str)]) -> Output {
    let mut command = Command::new(SPINEL);
    command.args(flags).arg(fixture(script)).args(args);
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
        &[],
        "windows1252.rb",
        &[source.as_os_str(), out.as_os_str()],
        &[("LANG", UTF8_LOCALE)],
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
    String::from_utf8_lossy(&run(&[], "locale.rb", &[source.as_os_str()], locale).stdout)
        .into_owned()
}

/// Under the C or POSIX locale, or none, CRuby's default external encoding
/// is US-ASCII, and so is what `File.read` returns, `ARGV` and `ENV`;
/// `Dir.pwd` is BINARY. A file opened with its own encodings is unchanged.
#[test]
fn the_c_locale_is_us_ascii() {
    let c = format!(
        concat!(
            "[#<Encoding:US-ASCII>, #<Encoding:US-ASCII>, #<Encoding:US-ASCII>, \"{}\"]\n",
            "[#<Encoding:US-ASCII>, false, #<Encoding:US-ASCII>, #<Encoding:US-ASCII>, #<Encoding:BINARY (ASCII-8BIT)>]\n",
            "[\"locale\", \"external\", \"filesystem\"]\n",
            "#<Encoding:UTF-8>\n",
        ),
        C_CHARMAP
    );
    assert_eq!(locale_report(&[("LANG", "C")]), c);
    assert_eq!(locale_report(&[("LANG", "POSIX")]), c);
    assert_eq!(locale_report(&[]), c);
    assert_eq!(locale_report(&[("LANG", UTF8_LOCALE), ("LC_ALL", "C")]), c);
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
    assert_eq!(locale_report(&[("LANG", UTF8_LOCALE)]), utf8);
    assert_eq!(
        locale_report(&[("LANG", "C"), ("LC_CTYPE", UTF8_LOCALE)]),
        utf8
    );
}

fn argv_report(flags: &[&str], locale: &[(&str, &str)], rubyopt: Option<&str>) -> String {
    let mut locale = locale.to_vec();
    if let Some(rubyopt) = rubyopt {
        locale.push(("RUBYOPT", rubyopt));
    }
    let args = ["é", "x"].map(std::ffi::OsStr::new);
    String::from_utf8_lossy(&run(flags, "argv.rb", &args, &locale).stdout).into_owned()
}

/// `ARGV` is in the default external encoding once `-E` or `RUBYOPT=-E` has
/// set it, BINARY for non-ASCII bytes under US-ASCII, and always frozen;
/// `$0` stays in the locale's.
#[test]
fn argv_follows_the_default_external_encoding() {
    assert_eq!(
        argv_report(&[], &[("LANG", "C")], None),
        "[[#<Encoding:BINARY (ASCII-8BIT)>, true], [#<Encoding:US-ASCII>, true]]\n#<Encoding:US-ASCII>\n"
    );
    let utf8_under_c =
        "[[#<Encoding:UTF-8>, true], [#<Encoding:UTF-8>, true]]\n#<Encoding:US-ASCII>\n";
    assert_eq!(
        argv_report(&["-E", "UTF-8"], &[("LANG", "C")], None),
        utf8_under_c
    );
    assert_eq!(
        argv_report(&[], &[("LANG", "C")], Some("-EUTF-8")),
        utf8_under_c
    );
    assert_eq!(
        argv_report(&[], &[("LANG", UTF8_LOCALE)], None),
        "[[#<Encoding:UTF-8>, true], [#<Encoding:UTF-8>, true]]\n#<Encoding:UTF-8>\n"
    );
    assert_eq!(
        argv_report(&["-E", "ISO-8859-1"], &[("LANG", UTF8_LOCALE)], None),
        "[[#<Encoding:ISO-8859-1>, true], [#<Encoding:ISO-8859-1>, true]]\n#<Encoding:UTF-8>\n"
    );
}
