//! RubyGems' own tests of `Gem::Version`, and of `Gem::Requirement`'s `~>`,
//! as tables.
//!
//! Copied from RubyGems, https://github.com/ruby/rubygems, at commit
//! c6c69d31d43a292ce4f3e6c75fd2769f02c436a9: `test/rubygems/test_gem_version.rb`
//! and the `~>` cases of `test/rubygems/test_gem_requirement.rb`. Each test
//! keeps its upstream name and cases; a case that is about Ruby objects rather
//! than versions (object identity, subclasses, `nil`, mutating `segments`,
//! whether `sort_key` was computed) says where it went instead, as does
//! `approximate_recommendation`, which is not ported.
//!
//! RubyGems is copyrighted free software by Chad Fowler, Rich Kilmer, Jim
//! Weirich and others, and the copied cases are used under its MIT license:
//!
//! > Copyright (c) Chad Fowler, Rich Kilmer, Jim Weirich and others.
//! > Portions copyright (c) 2010-2019 André Arko
//! > Portions copyright (c) 2009 Engine Yard
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining
//! > a copy of this software and associated documentation files (the
//! > "Software"), to deal in the Software without restriction, including
//! > without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject to
//! > the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be
//! > included in all copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
//! > EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! > MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
//! > NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
//! > LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! > OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
//! > WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
//!
//! The last table, `~>` at every segment depth, is not RubyGems': it was
//! recorded from `Gem::Requirement#satisfied_by?` on ruby 4.0.9 (RubyGems
//! 4.0.20).

use std::cmp::Ordering;
use std::hash::DefaultHasher;

use super::*;

fn v(version: &str) -> Version {
    Version::new(version).unwrap_or_else(|e| panic!("{e}"))
}

fn hash_of(version: &Version) -> u64 {
    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    hasher.finish()
}

fn n(digits: &str) -> Segment {
    Segment::Number(digits.to_owned())
}

fn s(letters: &str) -> Segment {
    Segment::Letters(letters.to_owned())
}

/// `Gem::Requirement.new("~> #{requirement}").satisfied_by?(v(version))`.
fn tilde(requirement: &str, version: &str) -> bool {
    pessimistic_satisfied_by(&v(requirement), &v(version))
}

// assert_version_equal: equal, and so the same hash.
fn assert_version_equal(expected: &str, actual: &Version) {
    assert_eq!(v(expected), *actual, "{actual} == {expected}");
    assert_eq!(
        hash_of(&v(expected)),
        hash_of(actual),
        "{actual} == {expected}, so the same hash"
    );
}

#[test]
fn test_nil_is_zero() {
    // `Gem::Version.create nil`: Rust has no nil string; a blank one is the
    // same version.
    assert_eq!(v(""), v("0"));
}

#[test]
fn test_bump() {
    for (expected, unbumped) in [
        ("5.3", "5.2.4"),     // test_bump
        ("5.3", "5.2.4.a"),   // test_bump_alpha
        ("5.3", "5.2.4.a10"), // test_bump_alphanumeric
        ("5.1", "5.0.0"),     // test_bump_trailing_zeros
        ("6", "5"),           // test_bump_one_level
    ] {
        assert_version_equal(expected, &v(unbumped).bump());
    }
}

#[test]
fn test_class_create() {
    // `Gem::Version.create` hands a Version back as it is (`assert_same`);
    // in Rust a `Version` is not made again from a `Version`. The rest:
    // `Gem::Version.new(1.0)`: a Float, read with `to_s`, which is `"1.0"`.
    assert_eq!(v("1.0"), v("1.0"));
    assert_eq!(v("5.1"), v("5.1"));
    assert_eq!(v("1.1"), v("1.1"));
}

#[test]
fn test_class_correct() {
    assert!(Version::correct("5.1"));
    assert!(!Version::correct("an incorrect version"));
    // `correct?(nil)` is true; a blank string is the Rust stand-in.
    assert!(Version::correct(""));
}

// test_class_new_subclass is about Ruby subclasses and the `new` cache, which
// a Rust struct has neither of.

#[test]
fn test_eql_eh() {
    let eql = |a: &str, b: &str| v(a).eql(&v(b)) && v(b).eql(&v(a));
    assert!(eql("1.2", "1.2"));
    assert!(!eql("1.2", "1.2.0"));
    assert!(!eql("1.2", "1.3"));
    assert!(!eql("1.2.b1", "1.2.b.1"));
}

#[test]
fn test_equals2() {
    assert_version_equal("1.2", &v("1.2"));
    assert_ne!(v("1.2"), v("1.3"));
    assert_version_equal("1.2.b1", &v("1.2.b.1"));
}

#[test]
fn test_hash() {
    assert_eq!(hash_of(&v("1.2")), hash_of(&v("1.2")));
    assert_ne!(hash_of(&v("1.2")), hash_of(&v("1.3")));
    assert_eq!(hash_of(&v("1.2")), hash_of(&v("1.2.0")));
    assert_eq!(hash_of(&v("1.2.pre.1")), hash_of(&v("1.2.0.pre.1.0")));
}

#[test]
fn test_initialize() {
    for good in ["1.0", "1.0 ", " 1.0 ", "1.0\n", "\n1.0\n", "1.0"] {
        assert_version_equal("1.0", &v(good));
    }
    // `Gem::Version.new(1)`: an Integer, read with `to_s`.
    assert_version_equal("1", &v(&1.to_string()));
}

#[test]
fn test_initialize_invalid() {
    let mut invalid_versions = vec!["junk", "1.0\n2.0", "1..2", "1.2 3.4"];

    // DON'T TOUCH THIS WITHOUT CHECKING CVE-2013-4287
    invalid_versions.push("2.3422222.222.222222222.22222.ads0as.dasd0.ddd2222.2.qd3e.");

    for invalid in invalid_versions {
        let e = Version::new(invalid).expect_err(invalid);
        assert_eq!(
            e.to_string(),
            format!("Malformed version number string {invalid}"),
            "{invalid}"
        );
    }
}

#[test]
fn test_empty_version() {
    for empty in ["", "   ", " "] {
        assert_eq!(v(empty).as_str(), "0");
    }
}

#[test]
fn test_prerelease() {
    for version in [
        "1.2.0.a",
        "2.9.b",
        "22.1.50.0.d",
        "1.2.d.42",
        "1.A",
        "1-1",
        "1-a",
    ] {
        assert!(v(version).prerelease(), "{version} is a prerelease");
    }
    for version in ["1.2.0", "2.9", "22.1.50.0"] {
        assert!(!v(version).prerelease(), "{version} is NOT a prerelease");
    }
}

#[test]
fn test_release() {
    for (release, version) in [
        ("1.2.0", "1.2.0.a"),
        ("1.1", "1.1.rc10"),
        ("1.9.3", "1.9.3.alpha.5"),
        ("1.9.3", "1.9.3"),
    ] {
        assert_version_equal(release, &v(version).release());
    }
}

#[test]
fn test_spaceship() {
    for (cmp, lhs, rhs) in [
        (Ordering::Equal, "1.0", "1.0.0"),
        (Ordering::Greater, "1.0", "1.0.a"),
        (Ordering::Greater, "1.8.2", "0.0.0"),
        (Ordering::Greater, "1.8.2", "1.8.2.a"),
        (Ordering::Greater, "1.8.2.b", "1.8.2.a"),
        (Ordering::Less, "1.8.2.a", "1.8.2"),
        (Ordering::Greater, "1.8.2.a10", "1.8.2.a9"),
        (Ordering::Equal, "", "0"),
        (Ordering::Equal, "0.beta.1", "0.0.beta.1"),
        (Ordering::Less, "0.0.beta", "0.0.beta.1"),
        (Ordering::Less, "0.0.beta", "0.beta.1"),
        (Ordering::Less, "5.a", "5.0.0.rc2"),
        (Ordering::Greater, "5.x", "5.0.0.rc2"),
    ] {
        assert_eq!(v(lhs).cmp(&v(rhs)), cmp, "{lhs} <=> {rhs}");
    }

    // `v("1.9.3") <=> string`: an incorrect string compares as nil, which
    // here is the string not being a version at all.
    for (cmp, string_ver) in [
        (Some(Ordering::Equal), "1.9.3"),
        (Some(Ordering::Greater), "1.9.2.99"),
        (Some(Ordering::Less), "1.9.3.1"),
        (None, "whatever"),
    ] {
        let other = Version::new(string_ver).ok();
        assert_eq!(
            other.map(|other| v("1.9.3").cmp(&other)),
            cmp,
            "{string_ver}"
        );
    }
}

// test_approximate_recommendation: not ported (the module comment).

#[test]
fn test_to_s() {
    assert_eq!(v("5.2.4").to_string(), "5.2.4");
}

#[test]
fn test_semver() {
    for (left, right) in [
        ("1.0.0-alpha", "1.0.0-alpha.1"),
        ("1.0.0-alpha.1", "1.0.0-beta.2"),
        ("1.0.0-beta.2", "1.0.0-beta.11"),
        ("1.0.0-beta.11", "1.0.0-rc.1"),
        ("1.0.0-rc1", "1.0.0"),
        ("1.0.0-1", "1"),
    ] {
        assert!(v(left) < v(right), "{left} not less than {right}");
    }
}

// test_sort_key_is_computed_on_regular_release,
// test_sort_key_is_computed_on_security_release,
// test_sort_key_is_not_computed_on_prerelease,
// test_sort_key_is_not_computed_on_version_with_more_segments and
// test_sort_key_is_not_computed_on_huge_numbers check whether RubyGems took
// its `sort_key` speed-up, which is not ported (the module comment). The
// orderings the sort_key tests check are kept:

#[test]
fn test_sort_key_on_timestamped_version() {
    assert!(v("1.0.0") > v("0.0.1.20220404083012"));
}

#[test]
fn test_sort_key_when_segment_is_higher_than_radix() {
    assert!(v("0.7.0") > v("0.6.63000"));
}

#[test]
fn test_sort_key_is_used_for_comparison() {
    assert!(v("18.0.1") < v("18.0.2"));
}

#[test]
fn test_segments() {
    // Upstream also checks that changing the Array `segments` returns does
    // not change the version; `segments` here is a shared borrow.
    assert_ne!(v("9.8.8"), v("9.8.7"));
    assert_eq!(v("9.8.7").segments(), [n("9"), n("8"), n("7")]);
}

#[test]
fn test_canonical_segments() {
    assert_eq!(v("1.0.0").canonical_segments(), [n("1")]);
    assert_eq!(
        v("1.0.0.a.1.0").canonical_segments(),
        [n("1"), s("a"), n("1")]
    );
    assert_eq!(
        v("1.2.3-1").canonical_segments(),
        [n("1"), n("2"), n("3"), s("pre"), n("1")]
    );
}

#[test]
fn test_frozen_version() {
    let version = v("1.freeze.test");
    assert!(version < v("1"));
    assert_version_equal("1", &version.release());
    assert_version_equal("2", &version.bump());
}

// test_gem_requirement.rb, the `~>` cases.

#[test]
fn test_satisfied_by_eh_tilde_gt() {
    // `~> 1.2`; its `satisfied_by? nil` ArgumentError has no Rust form.
    assert!(!tilde("1.2", "1.1"));
    assert!(tilde("1.2", "1.2"));
    assert!(tilde("1.2", "1.3"));
}

#[test]
fn test_satisfied_by_eh_tilde_gt_v0() {
    assert!(!tilde("0.0.1", "0.1.1"));
    assert!(tilde("0.0.1", "0.0.2"));
    assert!(tilde("0.0.1", "0.0.1"));
}

#[test]
fn test_satisfied_by_eh_good() {
    assert!(tilde("5.a", "5.0.0.rc2"));
    assert!(!tilde("5.x", "5.0.0.rc2"));

    assert!(tilde("5.a", "5.0.0"));
    assert!(tilde("5.x", "5.0.0"));
}

#[test]
fn test_satisfied_by_eh_boxed() {
    for (satisfied, version, requirement) in [
        (false, "1.3", "1.4"),
        (true, "1.4", "1.4"),
        (true, "1.5", "1.4"),
        (false, "2.0", "1.4"),
        (false, "1.3", "1.4.4"),
        (false, "1.4", "1.4.4"),
        (true, "1.4.4", "1.4.4"),
        (true, "1.4.5", "1.4.4"),
        (false, "1.5", "1.4.4"),
        (false, "2.0", "1.4.4"),
        (false, "1.1.pre", "1.0.0"),
        (false, "1.1.pre", "1.1"),
        (false, "2.0.a", "1.0"),
        (false, "2.0.a", "2.0"),
        (false, "0.9", "1"),
        (true, "1.0", "1"),
        (true, "1.1", "1"),
        (false, "2.0", "1"),
    ] {
        assert_eq!(
            tilde(requirement, version),
            satisfied,
            "{version} ~> {requirement}"
        );
    }
}

#[test]
fn test_satisfied_by_boxed() {
    for (satisfied, version, requirement) in [
        (false, "1.3", "1.4"),
        (true, "1.4", "1.4"),
        (true, "1.4.0", "1.4"),
        (true, "1.5", "1.4"),
        (false, "2.0", "1.4"),
        (false, "1.3", "1.4.4"),
        (false, "1.4", "1.4.4"),
        (true, "1.4.4", "1.4.4"),
        (true, "1.4.5", "1.4.4"),
        (false, "1.5", "1.4.4"),
        (false, "2.0", "1.4.4"),
    ] {
        assert_eq!(
            tilde(requirement, version),
            satisfied,
            "{version} ~> {requirement}"
        );
    }
}

/// `~>` at every segment depth, recorded from CRuby (the module comment):
/// `(requirement, version, satisfied)` for `~> requirement`.
const TILDE_AT_EVERY_DEPTH: &[(&str, &str, bool)] = &[
    ("0", "0", true),
    ("0", "1", false),
    ("0", "1.a", false),
    ("0", "1.0.1", false),
    ("0", "0.1", true),
    ("0", "0.0", true),
    ("0", "0.a", false),
    ("1", "1", true),
    ("1", "2", false),
    ("1", "2.a", false),
    ("1", "2.0.1", false),
    ("1", "0", false),
    ("1", "1.1", true),
    ("1", "1.0", true),
    ("1", "1.a", false),
    ("1.2", "1.2", true),
    ("1.2", "2", false),
    ("1.2", "2.a", false),
    ("1.2", "2.0.1", false),
    ("1.2", "2.2", false),
    ("1.2", "0.2", false),
    ("1.2", "1.3", true),
    ("1.2", "1.1", false),
    ("1.2", "1.2.1", true),
    ("1.2", "1.2.0", true),
    ("1.2", "1.2.a", false),
    ("1.2.3", "1.2.3", true),
    ("1.2.3", "1.3", false),
    ("1.2.3", "1.3.a", false),
    ("1.2.3", "1.3.0.1", false),
    ("1.2.3", "2.2.3", false),
    ("1.2.3", "0.2.3", false),
    ("1.2.3", "1.3.3", false),
    ("1.2.3", "1.1.3", false),
    ("1.2.3", "1.2.4", true),
    ("1.2.3", "1.2.2", false),
    ("1.2.3", "1.2.3.1", true),
    ("1.2.3", "1.2.3.0", true),
    ("1.2.3", "1.2.3.a", false),
    ("1.2.3.4", "1.2.3.4", true),
    ("1.2.3.4", "1.2.4", false),
    ("1.2.3.4", "1.2.4.a", false),
    ("1.2.3.4", "1.2.4.0.1", false),
    ("1.2.3.4", "2.2.3.4", false),
    ("1.2.3.4", "0.2.3.4", false),
    ("1.2.3.4", "1.3.3.4", false),
    ("1.2.3.4", "1.1.3.4", false),
    ("1.2.3.4", "1.2.4.4", false),
    ("1.2.3.4", "1.2.2.4", false),
    ("1.2.3.4", "1.2.3.5", true),
    ("1.2.3.4", "1.2.3.3", false),
    ("1.2.3.4", "1.2.3.4.1", true),
    ("1.2.3.4", "1.2.3.4.0", true),
    ("1.2.3.4", "1.2.3.4.a", false),
    ("1.2.3.4.5", "1.2.3.4.5", true),
    ("1.2.3.4.5", "1.2.3.5", false),
    ("1.2.3.4.5", "1.2.3.5.a", false),
    ("1.2.3.4.5", "1.2.3.5.0.1", false),
    ("1.2.3.4.5", "2.2.3.4.5", false),
    ("1.2.3.4.5", "0.2.3.4.5", false),
    ("1.2.3.4.5", "1.3.3.4.5", false),
    ("1.2.3.4.5", "1.1.3.4.5", false),
    ("1.2.3.4.5", "1.2.4.4.5", false),
    ("1.2.3.4.5", "1.2.2.4.5", false),
    ("1.2.3.4.5", "1.2.3.5.5", false),
    ("1.2.3.4.5", "1.2.3.3.5", false),
    ("1.2.3.4.5", "1.2.3.4.6", true),
    ("1.2.3.4.5", "1.2.3.4.4", false),
    ("1.2.3.4.5", "1.2.3.4.5.1", true),
    ("1.2.3.4.5", "1.2.3.4.5.0", true),
    ("1.2.3.4.5", "1.2.3.4.5.a", false),
    ("0.0.1", "0.0.1", true),
    ("0.0.1", "0.1", false),
    ("0.0.1", "0.1.a", false),
    ("0.0.1", "0.1.0.1", false),
    ("0.0.1", "1.0.1", false),
    ("0.0.1", "0.1.1", false),
    ("0.0.1", "0.0.2", true),
    ("0.0.1", "0.0.0", false),
    ("0.0.1", "0.0.1.1", true),
    ("0.0.1", "0.0.1.0", true),
    ("0.0.1", "0.0.1.a", false),
    ("1.0.0", "1.0.0", true),
    ("1.0.0", "1.1", false),
    ("1.0.0", "1.1.a", false),
    ("1.0.0", "1.1.0.1", false),
    ("1.0.0", "2.0.0", false),
    ("1.0.0", "0.0.0", false),
    ("1.0.0", "1.1.0", false),
    ("1.0.0", "1.0.1", true),
    ("1.0.0", "1.0.0.1", true),
    ("1.0.0", "1.0.0.0", true),
    ("1.0.0", "1.0.0.a", false),
    ("1.9", "1.9", true),
    ("1.9", "2", false),
    ("1.9", "2.a", false),
    ("1.9", "2.0.1", false),
    ("1.9", "2.9", false),
    ("1.9", "0.9", false),
    ("1.9", "1.10", true),
    ("1.9", "1.8", false),
    ("1.9", "1.9.1", true),
    ("1.9", "1.9.0", true),
    ("1.9", "1.9.a", false),
    ("1.2.a", "1.2.a", true),
    ("1.2.a", "2", false),
    ("1.2.a", "2.a", false),
    ("1.2.a", "2.0.1", false),
    ("1.2.a", "2.2", false),
    ("1.2.a", "0.2", false),
    ("1.2.a", "1.3", true),
    ("1.2.a", "1.1", false),
    ("1.2.a", "1.2.1", true),
    ("1.2.a", "1.2.0", true),
    ("1.2.a", "1.2", true),
    ("1.2.3.rc1", "1.2.3.rc1", true),
    ("1.2.3.rc1", "1.3", false),
    ("1.2.3.rc1", "1.3.a", false),
    ("1.2.3.rc1", "1.3.0.1", false),
    ("1.2.3.rc1", "2.2.3", false),
    ("1.2.3.rc1", "0.2.3", false),
    ("1.2.3.rc1", "1.3.3", false),
    ("1.2.3.rc1", "1.1.3", false),
    ("1.2.3.rc1", "1.2.4", true),
    ("1.2.3.rc1", "1.2.2", false),
    ("1.2.3.rc1", "1.2.3.1", true),
    ("1.2.3.rc1", "1.2.3.0", true),
    ("1.2.3.rc1", "1.2.3.a", false),
    ("1.2.3.rc1", "1.2.3", true),
    ("2.0.0-beta.2", "2.0.0-beta.2", true),
    ("2.0.0-beta.2", "2.1", false),
    ("2.0.0-beta.2", "2.1.a", false),
    ("2.0.0-beta.2", "2.1.0.1", false),
    ("2.0.0-beta.2", "3.0.0", false),
    ("2.0.0-beta.2", "1.0.0", false),
    ("2.0.0-beta.2", "2.1.0", false),
    ("2.0.0-beta.2", "2.0.1", true),
    ("2.0.0-beta.2", "2.0.0.1", true),
    ("2.0.0-beta.2", "2.0.0.0", true),
    ("2.0.0-beta.2", "2.0.0.a", false),
    ("2.0.0-beta.2", "2.0.0", true),
];

#[test]
fn tilde_at_every_segment_depth_matches_cruby() {
    for &(requirement, version, satisfied) in TILDE_AT_EVERY_DEPTH {
        assert_eq!(
            tilde(requirement, version),
            satisfied,
            "{version} ~> {requirement}"
        );
    }
}

#[test]
fn numbers_are_any_size_and_leading_zeros_do_not_count() {
    assert!(v("3.99999999999999999999999") < v("4"));
    assert!(v("1.99999999999999999999999") < v("1.100000000000000000000000"));
    assert_eq!(v("1.007"), v("1.7"));
    assert_eq!(v("1.99999999999999999999999").bump().as_str(), "2");
    assert_eq!(
        v("9.99999999999999999999999.1").bump().as_str(),
        "9.100000000000000000000000"
    );
}

/// Recorded from ruby 4.0.9: the whitespace RubyGems allows around a
/// version is Ruby's `\s`, vertical tab included, and nothing else.
#[test]
fn whitespace_around_a_version_is_rubys() {
    assert_eq!(v("\t1.0\r").as_str(), "1.0");
    assert_eq!(v("\x0b1.0\x0c").as_str(), "1.0");
    assert_eq!(v("\t").as_str(), "0");
    assert_eq!(v("\x0b").as_str(), "0");
    assert!(Version::new("1.0\0").is_err());
    assert!(Version::new("1 .0").is_err());
    assert!(Version::new("\u{ff11}.0").is_err());
}

/// Recorded from ruby 4.0.9: a `-` is `.pre.` even where that leaves an
/// empty segment, and such a version still orders and bumps.
#[test]
fn a_dash_is_always_pre() {
    assert_eq!(v("1--").as_str(), "1.pre..pre.");
    assert_eq!(v("1--").segments(), [n("1"), s("pre"), s("pre")]);
    assert_eq!(
        v("1.0.0-a.-").canonical_segments(),
        [n("1"), s("pre"), s("a"), s("pre")]
    );
    assert_eq!(v("1.0.0-a.-").bump().as_str(), "1.1");
    assert!(Version::new("1-").is_err());
    assert!(Version::new("1.0.0-a.").is_err());
}
