//! The built-in `bigdecimal` (#109): the bundled gem bigdecimal 4.0.1's
//! `lib/` vendored in `stdlib/`, over its C half in Ruby over Integer
//! (`lib/bigdecimal/spinel.rb`).
//!
//! ruby/spec's `library/bigdecimal` checks the API. `fixtures/bigdecimal/`
//! holds known answers from CRuby's own bigdecimal: `cases.rb` prints a
//! label and a result per line — construction from Strings (underscores,
//! exponents, garbage, `exception: false`, `interpret_loosely`), from
//! Integers, Floats (including the digits Ruby's dtoa rounds up), Rationals
//! and Complexes; arithmetic with Integer, Float and Rational operands on
//! either side; `add`, `sub`, `mult` and `div` at given precisions; `%`,
//! `remainder` and `divmod`; every rounding mode through `round`, `floor`,
//! `ceil` and `truncate` and as the thread's mode; the precision limit;
//! `sqrt`, `**` and `BigMath.PI(50)`; `to_s` in every format; NaN and the
//! infinities under each exception mode; and Marshal's bytes. Each
//! BigDecimal is shown with its sign and its `_dump`, so the word count
//! bigdecimal.c keeps is compared too. `cases.out` is what ruby 4.0.9
//! printed, written by `make.rb`. Everything runs offline.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bigdecimal")
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
fn answers_what_cruby_answers() {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join("cases.rb"))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8_lossy(&output.stdout);
    let expected =
        std::fs::read_to_string(fixtures().join("cases.out")).expect("cases.out is readable");
    if actual != expected {
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

/// bigdecimal 4.0.1, the release Ruby 4.0 bundles, its Ruby half the
/// gem's own: `BigDecimal#sqrt` and `BigMath.log` are defined in its
/// `bigdecimal.rb`, not in the C half.
#[test]
fn is_the_bundled_gem_it_claims() {
    let out = spinel(
        r#"require "bigdecimal"
puts BigDecimal::VERSION
puts BigDecimal.instance_method(:sqrt).source_location[0].end_with?("bigdecimal.rb")
puts BigMath.method(:log).source_location[0].end_with?("bigdecimal.rb")
puts $LOADED_FEATURES.any? { |f| f.end_with?("bigdecimal/util.rb") }
require "bigdecimal/util"
puts $LOADED_FEATURES.any? { |f| f.end_with?("bigdecimal/util.rb") }"#,
    );
    assert_eq!(out, "4.0.1\ntrue\ntrue\nfalse\ntrue\n");
}

/// The thread's modes and limit are its own, as bigdecimal.c keeps them in
/// `Thread#[]`: a mode set in one thread is not another's.
#[test]
fn keeps_modes_per_thread() {
    let out = spinel(
        r#"require "bigdecimal"
BigDecimal.limit(3)
BigDecimal.mode(BigDecimal::ROUND_MODE, :down)
other = Thread.new { [BigDecimal.limit, BigDecimal.mode(BigDecimal::ROUND_MODE), (BigDecimal("2") / 3).to_s] }.value
p other
p [BigDecimal.limit, BigDecimal.mode(BigDecimal::ROUND_MODE), (BigDecimal("2") / 3).to_s]
p Thread.current[:"BigDecimal.precision_limit"]"#,
    );
    assert_eq!(
        out,
        "[0, 3, \"0.66666666666666666666666666666667e0\"]\n[3, 2, \"0.666e0\"]\n3\n"
    );
}
