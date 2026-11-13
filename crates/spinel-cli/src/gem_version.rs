//! `Gem::Version` ported from RubyGems (#75, docs/package-manager.md,
//! "Resolver"): parsing and validation, segments, the canonical form,
//! ordering with prereleases, `bump`, `release`, and the `~>` pessimistic
//! check `Gem::Requirement` makes with them.
//!
//! RubyGems' `lib/rubygems/version.rb` is the source of truth, and this
//! follows it step for step, regular expressions included, so that the
//! resolver orders and matches versions exactly as Bundler does. Where its
//! behavior looks odd it is kept, with a note:
//!
//! - `1.0` and `1` are equal (`==`, `Ord`, `Hash`) but not `eql?`
//!   ([`Version::eql`]), which compares the strings.
//! - A `-` starts a prerelease and is read as `.pre.`: `1.0.0-rc1` is
//!   `1.0.0.pre.rc1`, so `1.0.0-rc1` and `1.0.0.pre.rc1` are equal.
//! - Zeros before a prerelease's first letters do not count:
//!   `0.0.beta.1 == 0.beta.1`, and `5.a < 5.0.0.rc2` because `a < rc`.
//! - Letters sort below numbers in the same position, so `1.a < 1.0 < 1.0.1`,
//!   and letters compare as bytes, so `1.B < 1.a`.
//! - A blank string is version `0`.
//!
//! Not ported: RubyGems' `sort_key` (one integer standing for a release's
//! first five segments), a speed-up of the comparison below rather than
//! another ordering; and `approximate_recommendation`, the requirement
//! `gem` suggests for a version, which orders nothing and whose output
//! RubyGems changed after 4.0.20 (`~> 1.0` there, `>= 1.0` upstream), so
//! the command that needs it (`spinel add`) will pin the one it emulates.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

/// One segment of a version: a run of digits or a run of letters, split as
/// RubyGems' `partition_segments` splits them (`/\d+|[a-z]+/i`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Segment {
    /// The digits without their leading zeros (`"0"` for zero). RubyGems
    /// reads them with `to_i`, an Integer of any size, so they are kept as
    /// text rather than narrowed to a machine word.
    Number(String),
    /// ASCII letters, case kept.
    Letters(String),
}

impl Segment {
    fn of(run: &str) -> Segment {
        if run.as_bytes()[0].is_ascii_digit() {
            let digits = run.trim_start_matches('0');
            Segment::Number(if digits.is_empty() { "0" } else { digits }.to_owned())
        } else {
            Segment::Letters(run.to_owned())
        }
    }

    fn is_letters(&self) -> bool {
        matches!(self, Segment::Letters(_))
    }

    fn is_zero(&self) -> bool {
        matches!(self, Segment::Number(n) if n == "0")
    }

    fn as_str(&self) -> &str {
        match self {
            Segment::Number(s) | Segment::Letters(s) => s,
        }
    }
}

/// `Gem::Version#<=>` on two segments in the same position: letters before
/// numbers, numbers by value, letters byte by byte (Ruby's `String#<=>`).
fn compare_segment(lhs: &Segment, rhs: &Segment) -> Ordering {
    match (lhs, rhs) {
        (Segment::Letters(_), Segment::Number(_)) => Ordering::Less,
        (Segment::Number(_), Segment::Letters(_)) => Ordering::Greater,
        (Segment::Number(l), Segment::Number(r)) => l.len().cmp(&r.len()).then_with(|| l.cmp(r)),
        (Segment::Letters(l), Segment::Letters(r)) => l.as_bytes().cmp(r.as_bytes()),
    }
}

/// A version string `Gem::Version.new` would reject, worded as its
/// `ArgumentError` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedVersion(pub String);

impl fmt::Display for MalformedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Malformed version number string {}", self.0)
    }
}

impl std::error::Error for MalformedVersion {}

/// A `Gem::Version`.
#[derive(Debug, Clone)]
pub struct Version {
    /// `@version`: the input stripped, `-` read as `.pre.`, blank as `0`.
    version: String,
    /// `segments`, the version's own.
    segments: Vec<Segment>,
    /// `canonical_segments`, what comparison and hashing use.
    canonical: Vec<Segment>,
}

/// Ruby's `\s`: what `ANCHORED_VERSION_PATTERN` allows around a version and
/// `String#strip` takes off. Not `char::is_ascii_whitespace`, which leaves
/// out the vertical tab.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// `VERSION_PATTERN`, matched against the whole of `s`:
/// `[0-9]+(?>\.[0-9a-zA-Z]+)*(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?`.
///
/// Greedy without backtracking is exact here: the pattern is followed only by
/// the end of the string, and every point where a greedy step could have
/// stopped short leaves a `.` or `-` that nothing after it can match.
fn matches_version_pattern(s: &[u8]) -> bool {
    let release = |c: u8| c.is_ascii_alphanumeric();
    let pre = |c: u8| c.is_ascii_alphanumeric() || c == b'-';
    // From `i`: a separator `sep` and one or more of `class`, repeated.
    let repeat = |mut i: usize, sep: u8, class: &dyn Fn(u8) -> bool| {
        while s.get(i) == Some(&sep) && s.get(i + 1).is_some_and(|&c| class(c)) {
            i += 2;
            while s.get(i).is_some_and(|&c| class(c)) {
                i += 1;
            }
        }
        i
    };
    let mut i = s.iter().take_while(|c| c.is_ascii_digit()).count();
    if i == 0 {
        return false;
    }
    i = repeat(i, b'.', &release);
    if s.get(i) == Some(&b'-') {
        let start = i + 1;
        i = start + s[start..].iter().take_while(|&&c| pre(c)).count();
        if i == start {
            return false;
        }
        i = repeat(i, b'.', &pre);
    }
    i == s.len()
}

/// `partition_segments`: `ver.scan(/\d+|[a-z]+/i)`, digits made numbers.
fn partition_segments(version: &str) -> Vec<Segment> {
    let bytes = version.as_bytes();
    let mut segments = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let class: fn(&u8) -> bool = if bytes[i].is_ascii_digit() {
            u8::is_ascii_digit
        } else if bytes[i].is_ascii_alphabetic() {
            u8::is_ascii_alphabetic
        } else {
            i += 1;
            continue;
        };
        let len = bytes[i..].iter().take_while(|c| class(c)).count();
        segments.push(Segment::of(&version[i..i + len]));
        i += len;
    }
    segments
}

/// The string `canonical_segments` partitions:
///
/// ```ruby
/// canonical_version = @version.sub(/(?<=[a-zA-Z.])[.0]+\z/, "")
/// canonical_version.sub!(/(?<=\.|\A)[0.]+(?=[a-zA-Z])/, "") if prerelease?
/// ```
///
/// The first drops trailing zeros (and dots) after a letter or a dot; the
/// second, for a prerelease, the zeros and dots that start a segment and run
/// into its first letters. Each replaces the leftmost match only, as `sub`
/// does.
fn canonical_version(version: &str, prerelease: bool) -> String {
    let b = version.as_bytes();
    let zero_or_dot = |c: u8| c == b'.' || c == b'0';
    let tail = b.len() - b.iter().rev().take_while(|&&c| zero_or_dot(c)).count();
    let cut = (tail.max(1)..b.len()).find(|&i| b[i - 1].is_ascii_alphabetic() || b[i - 1] == b'.');
    let mut canonical = version[..cut.unwrap_or(b.len())].to_owned();
    if prerelease {
        let b = canonical.as_bytes();
        let found = (0..b.len()).find_map(|i| {
            if !(i == 0 || b[i - 1] == b'.') || !zero_or_dot(b[i]) {
                return None;
            }
            let end = i + b[i..].iter().take_while(|&&c| zero_or_dot(c)).count();
            b.get(end)
                .is_some_and(u8::is_ascii_alphabetic)
                .then_some(i..end)
        });
        if let Some(run) = found {
            canonical.replace_range(run, "");
        }
    }
    canonical
}

impl Version {
    /// `Gem::Version.new(string)`. Integer and Float arguments, which RubyGems
    /// also takes, are the caller's `to_s`.
    pub fn new(input: &str) -> Result<Version, MalformedVersion> {
        if !Version::correct(input) {
            return Err(MalformedVersion(input.to_owned()));
        }
        let stripped = input.trim_matches(is_space);
        Ok(if stripped.is_empty() {
            Version::build("0".to_owned())
        } else {
            Version::build(stripped.replace('-', ".pre."))
        })
    }

    /// `Gem::Version.correct?`: `ANCHORED_VERSION_PATTERN`, a version or
    /// nothing, with whitespace around it.
    pub fn correct(input: &str) -> bool {
        let stripped = input.trim_matches(is_space);
        stripped.is_empty() || matches_version_pattern(stripped.as_bytes())
    }

    /// From an already normalized `@version`.
    fn build(version: String) -> Version {
        let segments = partition_segments(&version);
        let prerelease = version.bytes().any(|c| c.is_ascii_alphabetic());
        let canonical = partition_segments(&canonical_version(&version, prerelease));
        Version {
            version,
            segments,
            canonical,
        }
    }

    /// From segments, as RubyGems makes one with `segments.join(".")`.
    fn joined(segments: &[Segment]) -> Version {
        let strings: Vec<&str> = segments.iter().map(Segment::as_str).collect();
        Version::build(strings.join("."))
    }

    /// `version` / `to_s`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the resolver's API; only tests read it until the resolver lands"
        )
    )]
    pub fn as_str(&self) -> &str {
        &self.version
    }

    /// `segments`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the resolver's API; only tests read it until the resolver lands"
        )
    )]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// `canonical_segments`: what `<=>` and `hash` see.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the resolver's API; only tests read it until the resolver lands"
        )
    )]
    pub fn canonical_segments(&self) -> &[Segment] {
        &self.canonical
    }

    /// `prerelease?`: any letter in the version.
    pub fn prerelease(&self) -> bool {
        self.segments.iter().any(Segment::is_letters)
    }

    /// `eql?`: the same version string, which `1.0` and `1` are not.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the resolver's API; only tests read it until the resolver lands"
        )
    )]
    pub fn eql(&self, other: &Version) -> bool {
        self.version == other.version
    }

    /// The segments before the first letters, as `release` and `bump` take
    /// them (`segments.pop while segments.any? { String === _1 }`).
    fn release_segments(&self) -> &[Segment] {
        let letters = self.segments.iter().position(Segment::is_letters);
        &self.segments[..letters.unwrap_or(self.segments.len())]
    }

    /// `release`: the version without its prerelease part, `1.2.0.a` to
    /// `1.2.0`; a release is itself.
    pub fn release(&self) -> Version {
        if self.prerelease() {
            Version::joined(self.release_segments())
        } else {
            self.clone()
        }
    }

    /// `bump`: the release segments without the last one (unless it is the
    /// only one), the new last one plus one. `5.2.4` and `5.2.4.a` bump to
    /// `5.3`, `5` to `6`.
    pub fn bump(&self) -> Version {
        let mut segments = self.release_segments().to_vec();
        if segments.len() > 1 {
            segments.pop();
        }
        // A version starts with a number, so the release segments are never
        // empty and the last one is a number.
        if let Some(Segment::Number(last)) = segments.last_mut() {
            *last = succ(last);
        }
        Version::joined(&segments)
    }
}

/// `Integer#succ` on a decimal string without leading zeros.
fn succ(digits: &str) -> String {
    let mut out: Vec<u8> = digits.bytes().collect();
    let mut carry = true;
    for c in out.iter_mut().rev() {
        if *c == b'9' {
            *c = b'0';
        } else {
            *c += 1;
            carry = false;
            break;
        }
    }
    if carry {
        out.insert(0, b'1');
    }
    out.into_iter().map(char::from).collect()
}

/// `Gem::Requirement`'s `~>`, the pessimistic operator:
/// `v >= r && v.release < r.bump`. `~> 1.4` allows `1.4` up to but not
/// including `2.0`; `~> 1.4.4` allows `1.4.4` up to but not including `1.5`;
/// `~> 1` allows `1.0` up to but not including `2`. A prerelease of the bump
/// (`2.0.a` for `~> 1.0`) is outside, because its release is the bump.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the resolver's API; only tests read it until the resolver lands"
    )
)]
pub fn pessimistic_satisfied_by(requirement: &Version, version: &Version) -> bool {
    version >= requirement && version.release() < requirement.bump()
}

impl Ord for Version {
    /// `Gem::Version#<=>` over the canonical segments: position by position,
    /// then, past the shorter one, the longer one's rest decides: a letter
    /// there makes it the smaller (a prerelease), a number other than zero
    /// the larger.
    fn cmp(&self, other: &Version) -> Ordering {
        let (lhs, rhs) = (&self.canonical, &other.canonical);
        for (l, r) in lhs.iter().zip(rhs) {
            if l != r {
                return compare_segment(l, r);
            }
        }
        let limit = lhs.len().min(rhs.len());
        if lhs.len() <= rhs.len() {
            for r in &rhs[limit..] {
                if r.is_letters() {
                    return Ordering::Greater;
                }
                if !r.is_zero() {
                    return Ordering::Less;
                }
            }
        } else {
            for l in &lhs[limit..] {
                if l.is_letters() {
                    return Ordering::Less;
                }
                if !l.is_zero() {
                    return Ordering::Greater;
                }
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `==`, from `Comparable`: `<=>` is zero.
impl PartialEq for Version {
    fn eq(&self, other: &Version) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

/// `hash`: the canonical segments', so versions `==` hash alike.
impl Hash for Version {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.canonical.hash(state);
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.version)
    }
}

#[cfg(test)]
mod tests;
