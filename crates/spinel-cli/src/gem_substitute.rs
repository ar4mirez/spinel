//! Spinel-native gems: what they are called, which version of one stands in
//! for which upstream version, and which of a substitute's builds Spinel
//! installs (docs/package-manager.md, section 6).
//!
//! A Spinel-native gem is published to rubygems.org under its own name,
//! `<gem>-spinel`, with ordinary RubyGems platforms. Stock Bundler resolves
//! dependencies by name, so it never picks `pg-spinel` in place of `pg`. Spinel
//! resolves the Gemfile exactly as Bundler would, and then at install time puts
//! the substitute in place of the locked gem. The lockfile never mentions
//! Spinel. An earlier scheme, the platform `<cpu>-<os>-spinel`, was dropped
//! because RubyGems reads most of those spellings as plain platforms
//! (`arm64-darwin-spinel` is `arm64-darwin`). The gem_platform tests keep the
//! record of that.
//!
//! - **Name**: [`substitute_name`]. The name only says what a substitute is
//!   called. Whether a gem is substituted is decided by Spinel's own list, not
//!   by a name that ends in `-spinel`, because anyone can push such a name.
//!   rubygems.org already has `bundler-spinel`, and it is not Spinel's.
//! - **Version**: a substitute's version is the upstream version string, then
//!   `.` and a revision, a positive decimal integer with no leading zero:
//!   `pg 1.5.0` is substituted by `pg-spinel 1.5.0.1`, `1.5.0.2`, and so on
//!   ([`substitute_version`], [`revision_of`]). The match is on the locked
//!   string, so `1.5` and `1.5.0` are different upstreams even though RubyGems
//!   calls them equal. No `Gem::Requirement` says this (`~> 1.5.0.1` also
//!   admits `1.5.0.1.2`, a revision of an upstream `1.5.0.1`), so the rule is
//!   a function rather than a requirement.
//! - **Build**: [`pick`] chooses, among the builds that `Gem.platforms` (`ruby`
//!   and the local platform) accepts, the highest revision, and within it the
//!   platform RubyGems ranks best (`sort_best_platform_match`).
//!
//! Not here: the list of substituted gems, the compact-index client that reads
//! a substitute's builds, and the installer that writes the upstream identity
//! into the GEM_HOME and `.spinel/substitutions.json`. They need the resolver
//! and installer, which do not exist yet.

use std::num::NonZeroU64;

use crate::gem_platform::{GemPlatform, Platform, match_platforms, sort_best_platform_match};
use crate::gem_version::Version;

/// What a Spinel-native gem's name adds to the upstream name.
pub const SUFFIX: &str = "-spinel";

/// The name of `gem`'s Spinel-native substitute: `pg` is `pg-spinel`. `gem`
/// comes from Spinel's own list of substituted gems, all valid RubyGems names.
pub fn substitute_name(gem: &str) -> String {
    format!("{gem}{SUFFIX}")
}

/// Revision `revision` of the substitute for `upstream`: `1.5.0` and 2 give
/// `1.5.0.2`. `upstream` is spelled as RubyGems normalizes it, so `2.0.0-rc1`
/// gives `2.0.0.pre.rc1.1`, and a substitute for a prerelease is itself a
/// prerelease.
pub fn substitute_version(upstream: &Version, revision: NonZeroU64) -> Version {
    let joined = format!("{}.{revision}", upstream.as_str());
    // A version followed by `.` and digits is still a version: the pattern
    // allows any number of `.`-separated segments after the first.
    Version::new(&joined).expect("a version with a numeric segment appended is a version")
}

/// The revision `candidate` is of the substitute for `upstream`, or `None`
/// when it is not one: its string must be `upstream`'s, a `.`, then a
/// positive decimal integer with no leading zero. A revision too large for
/// 64 bits counts as none.
pub fn revision_of(upstream: &Version, candidate: &Version) -> Option<NonZeroU64> {
    let digits = candidate
        .as_str()
        .strip_prefix(upstream.as_str())?
        .strip_prefix('.')?;
    // No leading zero: `1.5.0.01` and `1.5.0.0` would compare equal to
    // `1.5.0.1` and `1.5.0` under `Gem::Version#<=>`.
    if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// One published build of a substitute, as a compact-index `/info` line names
/// it: a version and a platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub version: Version,
    pub platform: GemPlatform,
}

/// The build of a substitute that Spinel installs in place of `upstream` on
/// `local`, or `None` when no build fits. Among the revisions of `upstream`
/// ([`revision_of`]) whose platform `Gem.platforms` accepts (`ruby` and the
/// local platform, [`match_platforms`]), the highest revision wins, and among
/// its builds the one `sort_best_platform_match` ranks first. A build for
/// another upstream version is never chosen, however close it is.
pub fn pick<'a>(upstream: &Version, builds: &'a [Build], local: &Platform) -> Option<&'a Build> {
    let local = GemPlatform::Platform(local.clone());
    let platforms = [GemPlatform::Ruby, local.clone()];
    let fitting: Vec<(NonZeroU64, &Build)> = builds
        .iter()
        .filter(|build| match_platforms(&build.platform, &platforms))
        .filter_map(|build| Some((revision_of(upstream, &build.version)?, build)))
        .collect();
    let newest = fitting.iter().map(|&(revision, _)| revision).max()?;
    let mut best: Vec<&Build> = fitting
        .into_iter()
        .filter(|&(revision, _)| revision == newest)
        .map(|(_, build)| build)
        .collect();
    sort_best_platform_match(&mut best, &local, |build| &build.platform);
    best.first().copied()
}

#[cfg(test)]
mod tests;
