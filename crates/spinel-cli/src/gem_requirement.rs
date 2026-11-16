//! `Gem::Requirement` ported from RubyGems (#78, docs/package-manager.md,
//! "Resolver"): what a `/info` line's dependencies and its `ruby:` and
//! `rubygems:` fields say, and what a Gemfile's `gem` requirements say, read
//! as RubyGems reads them.
//!
//! RubyGems' `lib/rubygems/requirement.rb` is the source of truth. A
//! requirement is a list of `(operator, version)` pairs, all of which a
//! version must satisfy; `>= 0` when the list is empty. Each is
//! `PATTERN`: optional whitespace, an operator (`=` when none is written),
//! optional whitespace, a version, optional whitespace.

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::gem_version::{MalformedVersion, Version, pessimistic_satisfied_by};

/// One of `Gem::Requirement::OPS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Pessimistic,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Gt => ">",
            Op::Lt => "<",
            Op::Ge => ">=",
            Op::Le => "<=",
            Op::Pessimistic => "~>",
        }
    }
}

/// A requirement RubyGems would not read: `Gem::Requirement::BadRequirementError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRequirement(pub String);

impl fmt::Display for BadRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Illformed requirement [{:?}]", self.0)
    }
}

impl std::error::Error for BadRequirement {}

impl From<MalformedVersion> for BadRequirement {
    fn from(e: MalformedVersion) -> BadRequirement {
        BadRequirement(e.0)
    }
}

/// `Gem::Requirement`: every pair must hold.
#[derive(Debug, Clone)]
pub struct Requirement {
    requirements: Vec<(Op, Version)>,
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

impl Requirement {
    /// `Gem::Requirement.parse`: one `"op version"`.
    pub fn parse_one(input: &str) -> Result<(Op, Version), BadRequirement> {
        let bad = || BadRequirement(input.to_owned());
        let rest = input.trim_start_matches(is_space);
        // Two-character operators first: `Regexp.union` would try `>` before
        // `>=`, then backtrack to it when no version follows the `>`.
        let (op, rest) = [
            ("!=", Op::Ne),
            (">=", Op::Ge),
            ("<=", Op::Le),
            ("~>", Op::Pessimistic),
            ("=", Op::Eq),
            (">", Op::Gt),
            ("<", Op::Lt),
        ]
        .into_iter()
        .find_map(|(spelling, op)| rest.strip_prefix(spelling).map(|rest| (op, rest)))
        .unwrap_or((Op::Eq, rest));
        let version = rest.trim_matches(is_space);
        // `VERSION_PATTERN` needs at least one digit, where `Version.new`
        // reads a blank string as `0`.
        if version.is_empty() || !Version::correct(version) {
            return Err(bad());
        }
        Ok((op, Version::new(version)?))
    }

    /// `Gem::Requirement.new(list)`: each parsed, duplicates dropped, `>= 0`
    /// when there are none.
    pub fn new<'a>(list: impl IntoIterator<Item = &'a str>) -> Result<Requirement, BadRequirement> {
        let mut requirements: Vec<(Op, Version)> = Vec::new();
        for item in list {
            let pair = Requirement::parse_one(item)?;
            // `uniq!` on the strings, then on the parsed pairs (`==`).
            if !requirements.contains(&pair) {
                requirements.push(pair);
            }
        }
        if requirements.is_empty() {
            return Ok(Requirement::default());
        }
        Ok(Requirement { requirements })
    }

    /// The pairs, in the order given.
    pub fn requirements(&self) -> &[(Op, Version)] {
        &self.requirements
    }

    /// `none?`: only `>= 0`, which every version satisfies.
    pub fn none(&self) -> bool {
        let zero = Version::new("0").expect("0 is a version");
        matches!(self.requirements.as_slice(), [(Op::Ge, v)] if *v == zero)
    }

    /// `prerelease?`: any version in it is a prerelease, which is how a
    /// Gemfile asks for prereleases of a gem.
    pub fn prerelease(&self) -> bool {
        self.requirements.iter().any(|(_, v)| v.prerelease())
    }

    /// `satisfied_by?`.
    pub fn satisfied_by(&self, version: &Version) -> bool {
        self.requirements.iter().all(|(op, r)| match op {
            Op::Eq => version == r,
            Op::Ne => version != r,
            Op::Gt => version > r,
            Op::Lt => version < r,
            Op::Ge => version >= r,
            Op::Le => version <= r,
            Op::Pessimistic => pessimistic_satisfied_by(r, version),
        })
    }
}

impl Requirement {
    /// `_sorted_requirements`: the pairs sorted by `to_s`, which orders them
    /// by operator, then by how the version is written.
    fn sorted(&self) -> Vec<&(Op, Version)> {
        let mut pairs: Vec<&(Op, Version)> = self.requirements.iter().collect();
        pairs.sort_by_cached_key(|(op, version)| (op.as_str(), version.to_string()));
        pairs
    }
}

/// `==`: the same pairs in any order, a `~>` version written the same way
/// (`eql?`), any other version only equal.
impl PartialEq for Requirement {
    fn eq(&self, other: &Requirement) -> bool {
        let (a, b) = (self.sorted(), other.sorted());
        a == b
            && a.iter()
                .zip(&b)
                .all(|(x, y)| x.0 != Op::Pessimistic || x.1.eql(&y.1))
    }
}

impl Eq for Requirement {}

/// `hash`: the sorted pairs', a `~>` version's spelling with it.
impl Hash for Requirement {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (op, version) in self.sorted() {
            op.hash(state);
            if *op == Op::Pessimistic {
                version.to_string().hash(state);
            } else {
                version.hash(state);
            }
        }
    }
}

/// `Gem::Requirement.default`: `>= 0`.
impl Default for Requirement {
    fn default() -> Requirement {
        Requirement {
            requirements: vec![(Op::Ge, Version::new("0").expect("0 is a version"))],
        }
    }
}

/// `to_s`: `as_list` joined by `, `.
impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (op, version)) in self.requirements.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{} {version}", op.as_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(list: &[&str]) -> Requirement {
        Requirement::new(list.iter().copied()).unwrap()
    }

    fn v(s: &str) -> Version {
        Version::new(s).unwrap()
    }

    #[test]
    fn parses_as_rubygems_does() {
        assert_eq!(req(&["1.0"]).to_string(), "= 1.0");
        assert_eq!(req(&[" >=  2.7.8 "]).to_string(), ">= 2.7.8");
        assert_eq!(req(&["< 4", ">= 3.0.0"]).to_string(), "< 4, >= 3.0.0");
        assert_eq!(req(&["~>1.0"]).to_string(), "~> 1.0");
        assert_eq!(req(&["!= 1.1"]).to_string(), "!= 1.1");
        assert_eq!(req(&["= 1", "= 1"]).to_string(), "= 1");
        assert_eq!(req(&[]).to_string(), ">= 0");
        assert!(req(&[]).none());
        assert!(req(&[">= 0"]).none());
        assert!(req(&[">= 0.0"]).none());
        assert!(!req(&["> 0.0.0"]).none());
        for bad in ["", ">=", "~> x", "=> 1.0", "1.0 2.0", ">= 1..0"] {
            assert!(Requirement::new([bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn satisfied_by_each_operator() {
        let cases = [
            ("= 1.0", "1", true),
            ("= 1.0", "1.0.1", false),
            ("!= 1.0", "1.0.1", true),
            ("> 1.0", "1.0.a", false),
            ("< 2", "2.0.a", true),
            ("<= 1.0", "1.0", true),
            (">= 1.0", "1.0.a", false),
            ("~> 1.4", "1.9.9", true),
            ("~> 1.4", "2.0", false),
            ("~> 1.4.4", "1.4.9", true),
            ("~> 1.4.4", "1.5", false),
            ("~> 1.0", "2.0.a", false),
        ];
        for (r, version, expected) in cases {
            assert_eq!(
                req(&[r]).satisfied_by(&v(version)),
                expected,
                "{r} {version}"
            );
        }
        assert!(req(&["< 4", ">= 3.0.0"]).satisfied_by(&v("3.2.8")));
        assert!(!req(&["< 4", ">= 3.0.0"]).satisfied_by(&v("4.0")));
    }

    #[test]
    fn equal_as_rubygems_compares() {
        assert_eq!(req(&["< 4", ">= 2"]), req(&[">= 2", "< 4"]));
        assert_eq!(req(&[">= 2.0", "< 4"]), req(&["< 4", ">= 2"]));
        assert_eq!(req(&[]), req(&[">= 0"]));
        assert_ne!(req(&["~> 1.0"]), req(&["~> 1"]));
        assert_ne!(req(&["< 4", ">= 2"]), req(&[">= 2"]));
        let hash = |r: &Requirement| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            r.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&req(&["< 4", ">= 2"])), hash(&req(&[">= 2.0", "< 4"])));
    }

    #[test]
    fn prerelease_requirements() {
        assert!(req(&[">= 3.0.0.beta1", "< 3.0.0"]).prerelease());
        assert!(!req(&["< 3.0.0"]).prerelease());
    }
}
