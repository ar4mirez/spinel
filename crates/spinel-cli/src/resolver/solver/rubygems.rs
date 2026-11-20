//! `rubygems.rb` and `Bundler::Resolver#requirement_to_range`: a
//! `Gem::Requirement` as a [`RangeSet`].

use crate::gem_requirement::{Op, Requirement};
use crate::gem_version::Version as GemVersion;

use super::super::Candidate;
use super::Version;
use super::version_range::{RangeSet, VersionRange};

/// Each pair of `requirement` as a range, intersected (`inject(:intersect)`,
/// so a requirement of one pair is that range, its name kept). A lower bound
/// and a closed upper bound of `v` are `low(v)`; an open lower bound and a
/// closed upper bound are `high(v)`, and `~> v` ends before `low` of the
/// bump's `.A`, the lowest prerelease of it.
fn requirement_to_range<V: Version>(
    requirement: &Requirement,
    low: impl Fn(GemVersion) -> V,
    high: impl Fn(GemVersion) -> V,
) -> RangeSet<V> {
    let mut ranges = requirement.requirements().iter().map(|(op, version)| {
        let ver = || low(version.clone());
        let platform_ver = || high(version.clone());
        let range = |min, max, include_min, include_max| {
            RangeSet::Range(VersionRange::new(min, max, include_min, include_max))
        };
        match op {
            Op::Pessimistic => {
                let bump =
                    GemVersion::new(&format!("{}.A", version.bump())).expect("a bump is a version");
                RangeSet::Range(
                    VersionRange::new(Some(ver()), Some(low(bump)), true, false)
                        .named(format!("~> {version}")),
                )
            }
            Op::Gt => range(Some(platform_ver()), None, false, false),
            Op::Ge => range(Some(ver()), None, true, false),
            Op::Lt => range(None, Some(ver()), false, false),
            Op::Le => range(None, Some(platform_ver()), false, true),
            Op::Eq => range(Some(ver()), Some(platform_ver()), true, true),
            Op::Ne => range(Some(ver()), Some(platform_ver()), true, true).invert(),
        }
    });
    let first = ranges.next().expect("a requirement has a pair");
    ranges.fold(first, |acc, range| acc.intersect(&range))
}

/// `Bundler::Resolver#requirement_to_range`: over the two candidates of a
/// version, its `ruby` build (priority -1) below its platform builds
/// (priority 1), so `= 1.0` covers both and `> 1.0` neither.
pub fn candidate_range(requirement: &Requirement) -> RangeSet<Candidate> {
    requirement_to_range(
        requirement,
        |v| Candidate::new(v, -1),
        |v| Candidate::new(v, 1),
    )
}

/// `PubGrub::RubyGems.requirement_to_range`, over plain versions.
#[cfg(test)]
pub fn version_range(requirement: &Requirement) -> RangeSet<GemVersion> {
    requirement_to_range(requirement, |v| v, |v| v)
}

/// `PubGrub::RubyGems.parse_range`: `Gem::Requirement.new(dep)` as a range.
#[cfg(test)]
pub fn parse_range(dep: &[&str]) -> RangeSet<GemVersion> {
    let requirement = Requirement::new(dep.iter().copied()).expect("a requirement");
    version_range(&requirement)
}
