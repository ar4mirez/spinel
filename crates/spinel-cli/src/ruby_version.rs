//! The Ruby a project asks for: a Gemfile's `ruby` directive and a
//! `.ruby-version` file (#73, docs/cli.md, "Language version").
//!
//! Spinel implements one Ruby language version, so a project naming a CRuby
//! version is read as "this app expects at least that Ruby". A request this
//! build satisfies is silent; one it does not is a warning, never an error,
//! unless `--strict-ruby-version` asks for the error (for CI).
//!
//! Both files are looked for from the current directory upward, the nearest
//! one winning, as `shims/setup.rb` looks for `.spinel/` and as Bundler and
//! version managers look for theirs.

use std::path::{Path, PathBuf};

use spinel_vm::{ENGINE, ENGINE_VERSION, LANGUAGE_VERSION};

use crate::gem_version::Version;

/// One request this build does not satisfy, already worded for the user.
#[derive(Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub file: PathBuf,
    pub message: String,
}

/// Every unsatisfied request found from `dir` upward: the nearest
/// `.ruby-version`, then the nearest Gemfile's `ruby` directive.
pub fn check(dir: &Path) -> Vec<Mismatch> {
    let mut found = Vec::new();
    if let Some(file) = nearest(dir, ".ruby-version")
        && let Ok(text) = std::fs::read_to_string(&file)
        && let Some(message) = check_ruby_version_file(&text)
    {
        found.push(Mismatch { file, message });
    }
    if let Some(file) = nearest(dir, "Gemfile")
        && let Ok(text) = std::fs::read_to_string(&file)
        && let Some(message) = check_gemfile(&text)
    {
        found.push(Mismatch { file, message });
    }
    found
}

/// `name` in `dir` or the closest directory above it.
fn nearest(dir: &Path, name: &str) -> Option<PathBuf> {
    dir.ancestors()
        .map(|d| d.join(name))
        .find(|candidate| candidate.is_file())
}

/// What this build implements, for the end of every message.
fn implemented() -> String {
    format!("{ENGINE} {ENGINE_VERSION} implements Ruby {LANGUAGE_VERSION}")
}

/// A `.ruby-version`'s first word, read as rbenv, chruby and asdf read it:
/// `spinel-<version>` (what `spinel init` writes), `ruby-<version>` or a bare
/// `<version>` naming CRuby, or another engine's `<engine>-<version>`.
fn check_ruby_version_file(text: &str) -> Option<String> {
    let word = text.split_whitespace().next()?;
    if word == ENGINE || word == "ruby" {
        return None;
    }
    if let Some(version) = word.strip_prefix("spinel-") {
        return match parse_leniently(version) {
            Some(wanted) if at_least(ENGINE_VERSION, &wanted) => None,
            Some(_) => Some(format!(
                "asks for Spinel {version}; this is {}",
                implemented()
            )),
            None => Some(format!("names `{word}`, which is not a Spinel version")),
        };
    }
    let version = word.strip_prefix("ruby-").unwrap_or(word);
    if version.starts_with(|c: char| c.is_ascii_digit()) {
        return match parse_leniently(version) {
            Some(wanted) if at_least(LANGUAGE_VERSION, &wanted) => None,
            _ => Some(format!("asks for Ruby {version}; {}", implemented())),
        };
    }
    Some(format!(
        "names `{word}`, which is not a Ruby {ENGINE} can stand in for; {}",
        implemented()
    ))
}

/// A Gemfile's `ruby` directive: `ruby "3.3"`, `ruby "~> 3.3.0"`,
/// `ruby(">= 3.1", "< 5")`. Each requirement's version is read as a minimum,
/// the doc's "at least that Ruby"; upper bounds (`<`, `<=`) and exclusions
/// (`!=`) do not ask for anything Spinel could lack, so they are passed over.
/// Keyword options (`engine:`, `patchlevel:`, `file:`) are not requirements on
/// the language version and end the list.
// ponytail: a line scan, not an evaluation, so a directive built at runtime
// (`ruby File.read(...)`, a version in a constant) is not seen. The upgrade is
// the Gemfile evaluator the package manager brings (package-manager.md), which
// will hand this module the evaluated requirement instead.
fn check_gemfile(text: &str) -> Option<String> {
    let requirements = text.lines().find_map(ruby_directive)?;
    for requirement in requirements {
        let (operator, version) = split_requirement(&requirement);
        if matches!(operator, "<" | "<=" | "!=") {
            continue;
        }
        match parse(version) {
            Some(wanted) if at_least(LANGUAGE_VERSION, &wanted) => {}
            _ => {
                return Some(format!(
                    "asks for Ruby {} with `ruby`; {}",
                    requirement.trim(),
                    implemented()
                ));
            }
        }
    }
    None
}

/// The quoted requirements of a `ruby` directive on one line, or `None` when
/// the line is not one.
fn ruby_directive(line: &str) -> Option<Vec<String>> {
    let rest = line.trim_start().strip_prefix("ruby")?;
    if !rest.starts_with([' ', '\t', '(']) {
        return None;
    }
    let mut rest = rest
        .trim_start()
        .strip_prefix('(')
        .unwrap_or(rest)
        .trim_start();
    let mut requirements = Vec::new();
    while let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let body = &rest[1..];
        let end = body.find(quote)?;
        requirements.push(body[..end].to_owned());
        rest = body[end + 1..].trim_start();
        match rest.strip_prefix(',') {
            Some(after) => rest = after.trim_start(),
            None => break,
        }
    }
    (!requirements.is_empty()).then_some(requirements)
}

/// `"~> 3.3.0"` into `("~>", "3.3.0")`; a bare version has the operator `=`.
fn split_requirement(requirement: &str) -> (&str, &str) {
    let requirement = requirement.trim();
    for operator in ["~>", ">=", "<=", "!=", ">", "<", "="] {
        if let Some(version) = requirement.strip_prefix(operator) {
            return (operator, version.trim());
        }
    }
    ("=", requirement)
}

/// A requested version, read as RubyGems reads it (`Gem::Version`), so
/// `3.4.0-preview1` is a prerelease of `3.4.0` and sorts before it. `None`
/// for a blank string, which RubyGems would read as `0`, and for anything
/// RubyGems would reject.
fn parse(version: &str) -> Option<Version> {
    if version.trim().is_empty() {
        return None;
    }
    Version::new(version).ok()
}

/// A `.ruby-version`'s version, read as `parse` reads it when RubyGems takes
/// it (`3.3.0p0` is a prerelease of `3.3.0`), and otherwise by its leading numeric segments, as version managers
/// write words RubyGems rejects: `2.7.8_1` is `2.7.8`, `3.3.0@gemset` is
/// `3.3.0`.
fn parse_leniently(version: &str) -> Option<Version> {
    parse(version).or_else(|| {
        let mut numbers = Vec::new();
        for part in version.split('.') {
            let digits = &part[..part
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(part.len())];
            if digits.is_empty() {
                break;
            }
            numbers.push(digits);
            if digits.len() < part.len() {
                break;
            }
        }
        parse(&numbers.join("."))
    })
}

/// Whether `have`, this build's version, is `wanted` or later.
fn at_least(have: &str, wanted: &Version) -> bool {
    Version::new(have).is_ok_and(|have| have >= *wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_spinel_init_writes_is_satisfied() {
        let written = format!("spinel-{ENGINE_VERSION}\n");
        assert_eq!(check_ruby_version_file(&written), None);
    }

    #[test]
    fn an_older_cruby_is_satisfied_and_a_newer_one_is_not() {
        assert_eq!(check_ruby_version_file("3.3.0\n"), None);
        assert_eq!(check_ruby_version_file("ruby-3.3\n"), None);
        assert_eq!(check_ruby_version_file(LANGUAGE_VERSION), None);
        assert!(check_ruby_version_file("99.0.0").is_some());
        assert!(check_ruby_version_file("ruby-99.1").is_some());
    }

    #[test]
    fn a_patchlevel_word_is_a_prerelease_of_its_release() {
        assert_eq!(parse("3.3.0p0").map(|v| v.prerelease()), Some(true));
        assert_eq!(check_ruby_version_file("3.3.0p0\n"), None);
    }

    #[test]
    fn version_manager_words_rubygems_rejects_are_read_by_leading_numbers() {
        assert_eq!(parse("2.7.8_1"), None);
        assert_eq!(parse("3.3.0@gemset"), None);
        assert_eq!(check_ruby_version_file("2.7.8_1\n"), None);
        assert_eq!(check_ruby_version_file("ruby-3.3.0@gemset\n"), None);
        assert!(check_ruby_version_file("99.0.0_1").is_some());
        assert!(check_ruby_version_file("ruby-99.0@gemset").is_some());
        assert_eq!(parse_leniently("2.7.8_1"), parse("2.7.8"));
        assert_eq!(parse_leniently("4.0.0-preview1"), parse("4.0.0-preview1"));
        assert!(check_gemfile("ruby \"2.7.8_1\"\n").is_some());
    }

    #[test]
    fn a_later_spinel_or_another_engine_is_a_mismatch() {
        assert!(check_ruby_version_file("spinel-999.0.0").is_some());
        assert!(check_ruby_version_file("jruby-9.4.0.0").is_some());
        assert_eq!(check_ruby_version_file("spinel"), None);
        assert_eq!(check_ruby_version_file(""), None);
    }

    #[test]
    fn gemfile_directives_are_read_as_minimums() {
        assert_eq!(check_gemfile("source \"x\"\nruby \"3.3\"\n"), None);
        assert_eq!(check_gemfile("ruby '~> 3.3.0'\n"), None);
        assert_eq!(check_gemfile("ruby(\">= 3.1\", \"< 99\")\n"), None);
        assert_eq!(
            check_gemfile("ruby \"3.3\", engine: \"jruby\", engine_version: \"99\"\n"),
            None
        );
        assert!(check_gemfile("ruby \"99.0\"\n").is_some());
        assert!(check_gemfile("  ruby \">= 99\", \"< 100\"\n").is_some());
    }

    #[test]
    fn lines_that_are_not_the_directive_are_ignored() {
        assert_eq!(
            check_gemfile("gem \"ruby-progressbar\"\n# ruby \"99\"\n"),
            None
        );
        assert_eq!(check_gemfile("rubygems_version = \"99\"\n"), None);
        assert_eq!(check_gemfile("ruby file: \".ruby-version\"\n"), None);
    }

    #[test]
    fn versions_compare_as_rubygems_compares_them() {
        let v = |s: &str| parse(s).expect(s);
        assert!(at_least("4.0.0", &v("4")));
        assert!(at_least("4.0.0", &v("3.10")));
        assert!(!at_least("4.0.0", &v("4.0.1")));
        // A prerelease comes before its release, not level with it.
        assert!(at_least("4.0.0", &v("4.0.0-preview1")));
        assert!(!at_least("4.0.0-preview1", &v("4.0.0")));
        assert!(!at_least("4.0.0", &v("4.0.1.rc1")));
        // Segments are numbers of any size.
        assert!(at_least("4.0.0", &v("3.99999999999999999999999")));
        assert_eq!(parse("head"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("3.3.0_1"), None);
    }
}
