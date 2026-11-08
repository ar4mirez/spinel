//! The built-in `zlib` (#59): zlib.c in Ruby (`lib/zlib.rb`) over zlib 1.3.1
//! ported to Rust (`crates/spinel-vm/src/zlib.rs`). ruby/spec's
//! `library/zlib` checks the API; what it cannot check is that the bytes are
//! zlib's. `fixtures/zlib/zlib.rb` deflates at every level and strategy,
//! over several window sizes and memory levels, streams with every flush,
//! inflates corrupt, truncated and oversized streams and 300 mutated ones,
//! and writes and reads gzip files, and its output must be what ruby 4.0.9
//! prints, which is `fixtures/zlib/zlib.out`.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zlib")
}

fn spinel(script: &str) -> String {
    let output = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn zlib_compresses_and_decompresses_as_ruby_does() {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join("zlib.rb"))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = std::fs::read(fixtures().join("zlib.out")).expect("zlib.out is readable");
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

/// A stream that inflates to far more than it is, handed to a block, comes
/// out in pieces no larger than the 16 KiB zlib.c yields at, so a program
/// that streams it never holds it whole.
#[test]
fn a_bomb_inflates_in_bounded_pieces() {
    let script = r#"
        require "zlib"
        bomb = Zlib.deflate("\0" * 50_000_000, 9)
        total = 0
        largest = 0
        count = 0
        Zlib::Inflate.new.inflate(bomb) do |chunk|
          total += chunk.bytesize
          largest = chunk.bytesize if chunk.bytesize > largest
          count += 1
        end
        p [bomb.bytesize < 60_000, total, largest, count > 3000]
    "#;
    assert_eq!(spinel(script), "[true, 50000000, 16384, true]\n");
}

/// The built-in is the zlib Spinel claims: the gem's version is 3.2.3, the
/// release Ruby 4.0 bundles, over the zlib 1.3.1 its bytes are.
#[test]
fn the_built_in_zlib_is_the_pinned_release() {
    assert_eq!(
        spinel(
            "require 'zlib'; print Zlib::VERSION, ' ', Zlib::ZLIB_VERSION, ' ', Zlib.zlib_version"
        ),
        "3.2.3 1.3.1 1.3.1"
    );
}
