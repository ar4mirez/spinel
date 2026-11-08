//! The vendored `optparse` (#51): switches long, short, clustered, negated
//! and abbreviated; the built-in acceptors (`String`, `Integer`, `Float`,
//! `Numeric`, `OctalInteger`, `DecimalInteger`, `Array`, `TrueClass`,
//! enums, hashes, patterns, `Time`, `URI`) and a custom one; `parse`,
//! `parse!`, `order`, `order!`, `into:`, `getopts`, `environment` and
//! `define_by_keywords`; every `ParseError` subclass; and the help text.
//! ruby/spec's `library/optionparser` has only four examples, so this covers
//! the rest. Expected output is what ruby 4.0.9 prints for the same script.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

const EXPECTED: &str = r#"basic: [{verbose: true, name: "bob", count: 3}, ["file1", "file2"]]
no-verbose: [{verbose: false}, []]
equals: [{name: "alice", count: -7}, []]
attached: [{name: "bob", count: 42}, []]
clustered: [{x: true, y: true, verbose: true}, []]
abbrev: [{verbose: true, name: "zed"}, []]
float: [{ratio: 25.0}, []]
numeric: [{num: (3/4)}, []]
octal: [{oct: 15, dec: 10}, []]
bad octal: OptionParser::InvalidArgument: invalid argument: --oct 0x1f
list: [{list: ["a", "b", "c"]}, []]
enum: [{type: :binary}, []]
hash: [{code: 2}, []]
optional none: [{optional: nil, verbose: true}, []]
optional given: [{optional: "val"}, []]
bool: [{flag: false}, []]
regexp: [{re: ["12-34", "12", "34"]}, []]
time: [{time: 2024}, []]
uri: [{uri: "example.com"}, []]
terminator: [{verbose: true}, ["-x", "--name"]]
permute: [{verbose: true, count: 1}, ["a", "b", "c"]]
order: [["a", "-v", "b"], {}]
order!: [["x", "-y"], {verbose: true}]
parse!: [["x"], {verbose: true, y: true}]
into: {a: true, b: 5, "long-name": nil}
missing: OptionParser::MissingArgument: missing argument: --name
invalid: OptionParser::InvalidOption: invalid option: --bogus
invalid short: OptionParser::InvalidOption: invalid option: -q
bad int: OptionParser::InvalidArgument: invalid argument: -c abc
bad float: OptionParser::InvalidArgument: invalid argument: -r x
bad enum: OptionParser::InvalidArgument: invalid argument: -t zip
ambiguous: OptionParser::AmbiguousOption: ambiguous option: --n
needless: OptionParser::NeedlessArgument: needless argument: --verbose=yes
bad regexp: OptionParser::InvalidArgument: invalid argument: --re 12
require_exact: OptionParser::InvalidOption: invalid option: --verb
raise_unknown off: ["--zzz", "b"]
environment: {verbose: true, name: "envy"}
getopts: {"a" => true, "b" => "1", "foo" => true, "bar" => "2", "baz" => "9"}
summary width: 32
Usage: tool [options]

Specific options:
    -v, --[no-]verbose               Run verbosely
    -n, --name NAME                  A name
    -c, --count N                    A count
    -r, --ratio R                    A ratio
        --num X
        --oct X
        --dec X
    -l, --list A,B                   A list
    -t, --type TYPE                  Select type
        --code CODE
    -o, --optional [ARG]             Optional
        --flag=BOOL
        --re PAT
        --time T
        --uri U
    -x
    -y

    -h, --help                       Show help
tool 1.2.3
22
["invalid option", ["--zap"], "invalid option: --zap"]
2..5
{alpha: nil, beta_gamma: nil}
"#;

#[test]
fn optparse_parses_as_ruby_does() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/optparse/optparse.rb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .env_remove("TOOL")
        .output()
        .expect("spinel runs");

    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}
