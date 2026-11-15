use std::fs;
use std::path::{Path, PathBuf};

use super::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gem_spec")
}

/// Every `yaml/<case>.yaml` against the `gemspec/<case>.gemspec` RubyGems
/// wrote for it (`build.sh`), and the names of any that differ.
fn compare(yaml_dir: &Path, gemspec_dir: &Path) -> Vec<String> {
    // The fixtures' gemspecs were written with this, for the one with no date.
    // SAFETY: the tests that read the environment all set it to this value.
    unsafe { std::env::set_var("SOURCE_DATE_EPOCH", "1700000000") };
    let mut cases: Vec<PathBuf> = fs::read_dir(yaml_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no cases in {}", yaml_dir.display());
    let mut differ = Vec::new();
    for case in cases {
        let stem = case.file_stem().unwrap().to_string_lossy().into_owned();
        let expected = fs::read_to_string(gemspec_dir.join(format!("{stem}.gemspec")))
            .unwrap_or_else(|e| panic!("{stem}: {e}"));
        let actual = from_yaml(&fs::read(&case).unwrap()).and_then(|s| s.to_ruby_for_cache());
        match actual {
            Ok(actual) if actual == expected => {}
            Ok(actual) => {
                eprintln!("--- {stem}: expected\n{expected}\n--- {stem}: actual\n{actual}");
                differ.push(stem);
            }
            Err(e) => {
                eprintln!("--- {stem}: {e}");
                differ.push(stem);
            }
        }
    }
    differ
}

#[test]
fn installed_gemspecs_are_the_ones_rubygems_writes() {
    let dir = fixtures();
    let differ = compare(&dir.join("yaml"), &dir.join("gemspec"));
    assert!(differ.is_empty(), "differ from RubyGems: {differ:?}");
}

/// `SPINEL_GEMSPEC_CORPUS=<dir>` with `yaml/` and `gemspec/` in it, made as
/// `build.sh` makes the fixtures, checks a larger set: every gem a Ruby has
/// cached, say.
#[test]
fn a_corpus_given_matches_too() {
    let Some(dir) = std::env::var_os("SPINEL_GEMSPEC_CORPUS") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let differ = compare(&dir.join("yaml"), &dir.join("gemspec"));
    assert!(differ.is_empty(), "differ from RubyGems: {differ:?}");
}

#[test]
fn rubygems_version_is_the_one_spinel_ships() {
    let rubygems = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/rubygems.rb");
    let text = fs::read_to_string(&rubygems).unwrap();
    assert!(
        text.contains(&format!("  VERSION = \"{RUBYGEMS_VERSION}\"\n")),
        "stdlib/rubygems.rb is not RubyGems {RUBYGEMS_VERSION}"
    );
}

#[test]
fn names_come_from_the_spec() {
    let yaml = fs::read(fixtures().join("yaml/platform.yaml")).unwrap();
    let spec = from_yaml(&yaml).unwrap();
    assert_eq!(spec.full_name().unwrap(), "native-1.2.3-x86_64-darwin-19");
    assert_eq!(spec.executables().unwrap(), Vec::<String>::new());
    let yaml = fs::read(fixtures().join("yaml/rich.yaml")).unwrap();
    let spec = from_yaml(&yaml).unwrap();
    assert_eq!(spec.full_name().unwrap(), "rich-2.0.0.pre.rc1");
    assert_eq!(spec.bindir().unwrap().as_deref(), Some("bin"));
    assert_eq!(spec.executables().unwrap(), ["rich", "rich-server"]);
    assert_eq!(spec.extensions().unwrap(), ["ext/rich/extconf.rb"]);
}

#[test]
fn dump_escapes_as_ruby_does() {
    // `"...".dump` in CRuby 4.0.
    let cases: [(&[u8], bool, &str); 8] = [
        (b"plain", false, r#""plain""#),
        (b"a\"b\\c", false, r#""a\"b\\c""#),
        (b"#{x} #$y #@z #x", false, r#""\#{x} \#$y \#@z #x""#),
        (b"\x00\x1f\x7f\t\n", false, r#""\x00\x1F\x7F\t\n""#),
        ("é😀".as_bytes(), false, "\"\\u00E9\\u{1F600}\""),
        (b"\xff\xc3", false, r#""\xFF\xC3""#),
        ("é".as_bytes(), true, r#""\xC3\xA9""#),
        (b"\x07\x08\x0b\x0c\x0d\x1b", false, r#""\a\b\v\f\r\e""#),
    ];
    for (bytes, binary, expected) in cases {
        assert_eq!(dump(bytes, binary), expected, "{bytes:?}");
    }
}

#[test]
fn plain_scalars_are_typed_as_psych_types_them() {
    assert_eq!(tokenize("1.0.0"), Value::Str("1.0.0".to_owned()));
    assert_eq!(tokenize("4"), Value::Int(4));
    assert_eq!(tokenize("0x1f"), Value::Int(31));
    assert_eq!(tokenize("017"), Value::Int(15));
    assert_eq!(tokenize("1_000"), Value::Int(1000));
    // Psych weights the first field by 60 ** 2 whatever their number.
    assert_eq!(tokenize("1:30"), Value::Int(5400));
    assert_eq!(tokenize("1.5"), Value::Float(1.5));
    assert_eq!(tokenize("."), Value::Str(".".to_owned()));
    assert_eq!(tokenize("~"), Value::Nil);
    assert_eq!(tokenize("Null"), Value::Nil);
    assert_eq!(tokenize("yes"), Value::Bool(true));
    assert_eq!(tokenize("OFF"), Value::Bool(false));
    assert_eq!(tokenize("nullify"), Value::Str("nullify".to_owned()));
    assert_eq!(tokenize(":runtime"), Value::Sym("runtime".to_owned()));
    assert_eq!(tokenize(":\"a b\""), Value::Sym("a b".to_owned()));
    assert_eq!(tokenize("2024-02-29"), Value::Day(2024, 2, 29));
    assert_eq!(tokenize("2023-02-29"), Value::Str("2023-02-29".to_owned()));
    assert_eq!(
        tokenize("2010-07-04 00:00:00 -04:00"),
        Value::Day(2010, 7, 4)
    );
    assert_eq!(
        tokenize("2011-04-26 00:00:00.000000000 Z"),
        Value::Day(2011, 4, 26)
    );
    assert_eq!(tokenize(">="), Value::Str(">=".to_owned()));
    assert_eq!(tokenize("~>"), Value::Str("~>".to_owned()));
}

#[test]
fn floats_print_as_ruby_prints_them() {
    assert_eq!(float_to_s(1.0), "1.0");
    assert_eq!(float_to_s(1.5), "1.5");
    assert_eq!(float_to_s(0.1), "0.1");
    assert_eq!(float_to_s(1e16), "1.0e+16");
    assert_eq!(float_to_s(1.5e-5), "1.5e-05");
    assert_eq!(float_to_s(f64::INFINITY), "Infinity");
}

#[test]
fn what_rubygems_refuses_is_refused() {
    assert!(from_yaml(b"--- just a string\n").is_err());
    assert!(from_yaml(b"--- !ruby/object:Object\nname: x\n").is_err());
    assert!(from_yaml(b"--- !ruby/object:Gem::Specification\nname: x\ndate: tomorrow\n").is_err());
    assert!(
        from_yaml(b"--- !ruby/object:Gem::Specification\nfoo: !ruby/object:Kernel\n  a: 1\n")
            .is_err()
    );
}

#[test]
fn an_alias_bomb_is_refused_not_expanded() {
    let mut bomb = String::from("bomb0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..10 {
        let prev = format!("*l{}", level - 1);
        let items = [prev.as_str(); 10].join(", ");
        bomb.push_str(&format!("bomb{level}: &l{level} [{items}]\n"));
    }
    let err = with(&bomb).unwrap_err();
    assert!(err.contains("bytes"), "{err}");
    let big = format!(
        "big: &s \"{}\"\nfiles: [{}]\n",
        "x".repeat(1 << 20),
        ["*s"; 2_000].join(", ")
    );
    let err = with(&big).unwrap_err();
    assert!(err.contains("bytes"), "{err}");
    let shared = with("files: &f [a, b]\ntest_files: *f\n").unwrap();
    assert_eq!(shared.verify(), Ok(()));
}

#[test]
fn deep_nesting_is_refused_not_recursed_on() {
    let flow = format!(
        "files: {}{}\n",
        "[".repeat(1_000_000),
        "]".repeat(1_000_000)
    );
    assert!(with(&flow).is_err());
    let deep = format!(
        "files:\n{}{}{}\n",
        "- ".repeat(10),
        "[".repeat(250),
        "]".repeat(250)
    );
    let err = with(&deep).unwrap_err();
    assert!(err.contains("nests deeper"), "{err}");
    let nested = format!("metadata_x: {}{}\n", "[".repeat(100), "]".repeat(100));
    assert!(with(&nested).is_ok());
}

#[test]
fn civil_dates() {
    assert_eq!(civil(0), (1970, 1, 1));
    assert_eq!(civil(19_675), (2023, 11, 14));
    assert_eq!(civil(-1), (1969, 12, 31));
    assert_eq!(civil(11_016), (2000, 2, 29));
}

/// A gemspec with `extra` appended to a valid one's YAML.
fn with(extra: &str) -> Result<Spec, String> {
    let yaml = format!(
        "--- !ruby/object:Gem::Specification\nname: ok\nversion: !ruby/object:Gem::Version\n  version: '1'\n\
         summary: s\nspecification_version: 4\n{extra}"
    );
    from_yaml(yaml.as_bytes())
}

#[test]
fn what_gem_install_verifies_is_verified() {
    assert_eq!(with("").unwrap().verify(), Ok(()));
    // `from_yaml` gives a spec with none -1, which `verify_spec` refuses.
    let none = from_yaml(b"--- !ruby/object:Gem::Specification\nname: x\n").unwrap();
    assert_eq!(
        none.verify(),
        Err("an invalid specification_version".to_owned())
    );
    let refused = [
        ("name", "name: \"a b\"\n"),
        ("require_paths", "require_paths:\n- \"lib\\nsystem('x')\"\n"),
        ("extensions", "extensions:\n- \"ext\\u2028x\"\n"),
        (
            "specification_version",
            "specification_version: \"4; system('x')\"\n",
        ),
        (
            "dependencies",
            "dependencies:\n- !ruby/object:Gem::Dependency\n  name: \"x>.freeze, []); system('x'); s.add_runtime_dependency(%q<y\"\n  \
             type: :runtime\n  requirement: !ruby/object:Gem::Requirement\n    requirements: []\n",
        ),
        ("executable", "executables:\n- ../../evil\n"),
        ("executable", "executables:\n- \"..\"\n"),
        ("bindir", "bindir: ../..\n"),
        ("bindir", "bindir: a/../../b\n"),
    ];
    for (what, extra) in refused {
        let spec = with(extra).unwrap_or_else(|e| panic!("{extra}: {e}"));
        assert_eq!(spec.verify(), Err(format!("an invalid {what}")), "{extra}");
    }
    for fine in ["bindir: a/../b\n", "bindir: /exe\n", "bindir: .\n"] {
        assert_eq!(with(fine).unwrap().verify(), Ok(()), "{fine}");
    }
    // Only the two dependency types load as Symbols at all.
    assert!(with("bogus: :system\n").is_err());
    assert!(with("bogus: !ruby/symbol system\n").is_err());
    assert!(with("bogus: :runtime\n").is_ok());
}
