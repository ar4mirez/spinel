//! `ripper` and `prism` (#837). `require "prism"` loads the prism gem's Ruby
//! half (vendored in `stdlib/` at the release Spinel's parser links) over
//! Spinel's C half, `lib/prism/prism.rb`, and `require "ripper"` is the
//! gem's Ripper translation over it (`lib/ripper.rb`).
//!
//! `fixtures/ripper/ripper.rb` asks `Ripper.sexp`, `Ripper.lex` and
//! `Ripper.tokenize` what RSpec asks of them, over spec files, and hands
//! them sources that are not Strings; its output must be what ruby 4.0.9
//! prints, `fixtures/ripper/ripper.out`. `fixtures/prism/prism.rb` drives
//! the extension's API, options and errors; its output must be what ruby
//! 4.0.9 prints with the prism 1.9.0 gem's C extension,
//! `fixtures/prism/prism.out`.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Run `<name>/<name>.rb` and compare its output with `<name>/<name>.out`.
fn matches_ruby(name: &str) {
    let dir = fixtures(name);
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(dir.join(format!("{name}.rb")))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected =
        std::fs::read(dir.join(format!("{name}.out"))).expect("the expected output is readable");
    let actual = output.stdout;
    if actual != expected {
        let expected = String::from_utf8_lossy(&expected);
        let actual = String::from_utf8_lossy(&actual);
        let first = expected
            .lines()
            .zip(actual.lines())
            .position(|(e, a)| e != a)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "output differs from ruby 4.0.9's at line {}:\n  ruby:   {:?}\n  spinel: {:?}",
            first + 1,
            expected.lines().nth(first),
            actual.lines().nth(first)
        );
    }
}

#[test]
fn ripper_sexp_and_lex_match_ruby() {
    matches_ruby("ripper");
}

#[test]
fn prism_parses_lexes_and_refuses_as_its_extension_does() {
    matches_ruby("prism");
}
