//! The built-in `psych` (#108): ruby/psych 5.3.1's `lib/` vendored in
//! `stdlib/`, over its C half in Ruby (`lib/psych/spinel.rb`) over libyaml
//! 0.2.5 in Rust (`crates/spinel-vm/src/psych.rs`).
//!
//! ruby/spec's `library/yaml` checks the API with a handful of documents.
//! `fixtures/psych/` holds what CRuby's own Psych made of the cases in
//! `cases.rb` — `YAML.dump` and `dump_stream` of Hashes, Arrays, Strings that
//! need quoting or a block style, Symbols, Integers, Floats with `.inf` and
//! `.nan`, nil and the booleans, Times and Dates, Structs and objects,
//! multi-document streams, anchors and aliases, parsed documents emitted
//! again — written by `make.rb` on ruby 4.0.9, and `roundtrip.rb` checks
//! Spinel's dumps against those bytes and what loading them gives back,
//! along with `load` and `safe_load`'s refusals (permitted classes and
//! symbols, aliases, unknown anchors and classes, syntax errors). Its output
//! must be what it printed on CRuby, `roundtrip.out`. Everything runs offline.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/psych")
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
fn round_trips_against_cruby_produced_yaml() {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join("roundtrip.rb"))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8_lossy(&output.stdout);
    // Each dump is compared inside the fixture with the bytes CRuby wrote,
    // so a wrong byte reads MISMATCH whichever Ruby made `roundtrip.out`.
    let mismatches: Vec<&str> = actual
        .lines()
        .filter(|line| line.starts_with("MISMATCH"))
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    let expected = std::fs::read_to_string(fixtures().join("roundtrip.out"))
        .expect("roundtrip.out is readable");
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

/// psych 5.3.1, the release Ruby 4.0 bundles, over libyaml 0.2.5, the
/// release CRuby's Psych links.
#[test]
fn reports_the_psych_and_libyaml_it_is() {
    let out = spinel(
        r##"require "yaml"
puts Psych::VERSION, Psych::LIBYAML_VERSION, Psych.libyaml_version.inspect, YAML.equal?(Psych)"##,
    );
    assert_eq!(out, "5.3.1\n0.2.5\n[0, 2, 5]\ntrue\n");
}

/// The events a handler sees, with `event_location` before each and the
/// parser's mark libyaml's (what CRuby's Psych prints), from a String and
/// from an IO; and a syntax error after the events before it.
#[test]
fn a_handler_sees_libyamls_events() {
    let out = spinel(
        r##"require "psych"
require "stringio"
class Log < Psych::Handler
  def initialize(parser = nil) = (@parser = parser; @lines = [])
  attr_accessor :parser
  attr_reader :lines
  def event_location(*at) = @at = at
  %i[start_stream end_stream start_document end_document alias scalar
     start_sequence end_sequence start_mapping end_mapping].each do |name|
    define_method(name) do |*args|
      mark = @parser.mark
      @lines << "#{name} #{args.inspect} at=#{@at.inspect} mark=#{[mark.index, mark.line, mark.column].inspect}"
    end
  end
end
log = Log.new
parser = Psych::Parser.new(log)
log.parser = parser
parser.parse("%TAG !e! tag:example.com,2000:\n--- !e!x\na: &k [1, 'two']\nb: *k\n")
puts log.lines
puts parser.mark.to_a.inspect
log.lines.clear
parser.parse(StringIO.new("- x\n"))
puts log.lines.size
begin
  parser.parse("a: [1\n", "file.yml")
rescue Psych::SyntaxError => e
  puts e.message, parser.mark.to_a.inspect
end"##,
    );
    assert_eq!(
        out,
        r##"start_stream [1] at=[0, 0, 0, 0] mark=[0, 0, 0]
start_document [[], [["!e!", "tag:example.com,2000:"]], false] at=[0, 0, 1, 3] mark=[34, 1, 3]
start_mapping [nil, "tag:example.com,2000:x", false, 1] at=[1, 4, 2, 0] mark=[42, 2, 2]
scalar ["a", nil, nil, true, false, 1] at=[2, 0, 2, 1] mark=[42, 2, 2]
start_sequence ["k", nil, true, 2] at=[2, 3, 2, 7] mark=[47, 2, 7]
scalar ["1", nil, nil, true, false, 1] at=[2, 7, 2, 8] mark=[49, 2, 9]
scalar ["two", nil, nil, false, true, 2] at=[2, 10, 2, 15] mark=[56, 2, 16]
end_sequence [] at=[2, 15, 2, 16] mark=[56, 2, 16]
scalar ["b", nil, nil, true, false, 1] at=[3, 0, 3, 1] mark=[59, 3, 2]
alias ["k"] at=[3, 3, 3, 5] mark=[62, 3, 5]
end_mapping [] at=[4, 0, 4, 0] mark=[63, 4, 0]
end_document [true] at=[4, 0, 4, 0] mark=[63, 4, 0]
end_stream [] at=[4, 0, 4, 0] mark=[63, 4, 0]
[63, 4, 0]
7
(file.yml): did not find expected ',' or ']' while parsing a flow sequence at line 1 column 4
[0, 0, 0]
"##
    );
}

/// Each document reaches the IO as libyaml flushes it, at its end, so a
/// `Psych::Stream` writes as it goes; and an emitter refuses what libyaml
/// refuses, with libyaml's words.
#[test]
fn the_emitter_writes_as_libyaml_flushes() {
    let out = spinel(
        r##"require "psych"
require "stringio"
io = StringIO.new
stream = Psych::Stream.new(io)
stream.start
stream.push({"a" => 1})
puts io.string.inspect
stream.push([2])
puts io.string.inspect
stream.finish
emitter = Psych::Emitter.new(StringIO.new)
begin
  emitter.end_stream
rescue RuntimeError => e
  puts e.message
end
begin
  emitter.scalar(1, nil, nil, true, false, 1)
rescue TypeError => e
  puts e.message
end
# A failed event stays in libyaml's queue, so a fresh emitter.
emitter = Psych::Emitter.new(StringIO.new)
p [emitter.line_width, emitter.indentation, emitter.canonical]
emitter.start_stream(Psych::Nodes::Stream::UTF8)
p [emitter.line_width, emitter.indentation, emitter.canonical]"##,
    );
    assert_eq!(
        out,
        r##""---\na: 1\n...\n"
"---\na: 1\n...\n---\n- 2\n...\n"
expected STREAM-START
wrong argument type Integer (expected String)
[0, 2, false]
[80, 2, false]
"##
    );
}

/// A collection nested past the bound stops the parse with a
/// `Psych::SyntaxError` at the collection that went past it, rather than
/// recursing on until memory runs out; a deep one under it, deeper than
/// CRuby's Psych can build without overflowing its stack, still loads.
#[test]
fn nesting_is_bounded() {
    let out = spinel(
        r##"require "yaml"
deep = "[" * 10_001 + "]" * 10_001
begin
  YAML.load(deep)
rescue Psych::SyntaxError => e
  puts e.message
end
ok = "[" * 5_000 + "]" * 5_000
value = YAML.load(ok)
depth = 0
depth += 1 while (value = value[0])
puts depth"##,
    );
    assert_eq!(
        out,
        "(<unknown>): exceeded the maximum nesting depth of 10000 while parsing a node at line 1 column 10001\n4999\n"
    );
}

/// The bound is on building objects: a handler that only sees events gets
/// every level of a document deeper than it, with no error, as libyaml
/// gives them.
#[test]
fn event_parsing_is_unbounded() {
    let out = spinel(
        r##"require "yaml"
class Counter < Psych::Handler
  attr_reader :starts, :ends
  def initialize = (@starts = 0; @ends = 0)
  def start_sequence(*) = @starts += 1
  def end_sequence = @ends += 1
end
counter = Counter.new
Psych::Parser.new(counter).parse("[" * 10_050 + "]" * 10_050)
puts counter.starts, counter.ends"##,
    );
    assert_eq!(out, "10050\n10050\n");
}

/// With `Encoding.default_internal` set, every String a parse gives is in
/// it, ASCII-only ones too, as `rb_str_conv_enc` retags them on CRuby.
#[test]
fn parsed_strings_take_the_default_internal_encoding() {
    let out = spinel(
        r##"require "yaml"
Encoding.default_internal = Encoding::ISO_8859_1
doc = YAML.load("foo: bar\nn: \"caf\u00e9\"\n")
p doc.keys.map(&:encoding), doc.values.map(&:encoding)
Encoding.default_internal = Encoding::BINARY
p YAML.load("- x").first.encoding"##,
    );
    assert_eq!(
        out,
        "[#<Encoding:ISO-8859-1>, #<Encoding:ISO-8859-1>]\n[#<Encoding:ISO-8859-1>, #<Encoding:ISO-8859-1>]\n#<Encoding:BINARY (ASCII-8BIT)>\n"
    );
}
