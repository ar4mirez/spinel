//! The built-in `json` (#55): ruby/json 2.18.0's Ruby half, vendored in
//! `stdlib/json/`, over Spinel's parser (`crates/spinel-vm/src/json.rs`) and
//! generator (`lib/json/ext/`). ruby/spec has no `library/json`, so this is
//! the check: `fixtures/json/json.rb` parses valid, malformed and truncated
//! documents under every parser option, generates under every generator
//! option, and prints 2,000 random doubles, and its output must be what ruby
//! 4.0.9 prints, which is `fixtures/json/json.out`.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/json")
}

#[test]
fn json_parses_and_generates_as_ruby_does() {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join("json.rb"))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = std::fs::read(fixtures().join("json.out")).expect("json.out is readable");
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

/// What CRuby cannot be asked, because its parser recurses: a document a
/// million levels deep, with nesting unchecked, parses without running out
/// of stack, and with the default limit is refused at 101. Under `on_load`
/// the Ruby half builds the objects, also without recursing.
#[test]
fn a_million_levels_deep_parses_without_recursing() {
    let script = r#"
        require "json"
        n = 1_000_000
        x = JSON.parse("[" * n + "]" * n, max_nesting: false)
        depth = 0
        x, depth = x[0], depth + 1 while Array === x && !x.empty?
        p depth + 1
        m = 100_000
        x = JSON.parse("[" * m + "1" + "]" * m, max_nesting: false, on_load: ->(v) { v })
        depth = 0
        x, depth = x[0], depth + 1 while Array === x
        p [depth, x]
        begin
          JSON.parse("[" * n + "]" * n)
        rescue JSON::NestingError => e
          p e.message
        end
    "#;
    let output = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "1000000\n[100000, 1]\n\"nesting of 101 is too deep\"\n"
    );
}

/// The built-in is the json Spinel claims: `JSON::VERSION` from the vendored
/// `version.rb` is the release `scripts/vendor-stdlib.sh` pins, which is the
/// one Ruby 4.0 bundles, and is what the running binary reports.
#[test]
fn the_built_in_json_is_the_pinned_release() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = std::fs::read_to_string(root.join("scripts/vendor-stdlib.sh"))
        .expect("the vendoring script is readable");
    let pinned = script
        .lines()
        .find_map(|line| line.strip_prefix("JSON_TAG=\"v"))
        .and_then(|rest| rest.split('"').next())
        .expect("scripts/vendor-stdlib.sh pins JSON_TAG");
    let version = std::fs::read_to_string(root.join("stdlib/json/version.rb"))
        .expect("stdlib/json/version.rb is vendored");
    assert!(
        version.contains(&format!("VERSION = '{pinned}'")),
        "stdlib/json/version.rb is not json {pinned}; run scripts/vendor-stdlib.sh"
    );
    let output = Command::new(SPINEL)
        .args(["-e", "require 'json'; print JSON::VERSION"])
        .output()
        .expect("spinel runs");
    assert_eq!(String::from_utf8_lossy(&output.stdout), pinned);
}
