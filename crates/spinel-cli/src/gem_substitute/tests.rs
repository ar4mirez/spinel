//! The naming and selection rules of docs/package-manager.md, section 6. The
//! name table is recorded from RubyGems 4.0.20's
//! `Gem::SpecificationPolicy#validate_name` (ruby 4.0.9).

use super::*;

fn v(s: &str) -> Version {
    Version::new(s).unwrap()
}

fn rev(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).unwrap()
}

fn build(version: &str, platform: &str) -> Build {
    Build {
        version: v(version),
        platform: GemPlatform::new(Some(platform), &Platform::parse("x86_64-linux")),
    }
}

/// Which `validate_name` error RubyGems raises (`None`: the name is valid).
type Expected = Option<fn(String) -> InvalidName>;

/// Name, then what RubyGems says of it.
const NAMES: &[(&str, Expected)] = &[
    ("pg", None),
    ("sqlite3", None),
    ("nio4r", None),
    ("rack-test", None),
    ("net_http2", None),
    ("a.b", None),
    ("pg-", None),
    ("pg_", None),
    ("pg.", None),
    ("1pg", None),
    ("9-a", None),
    ("a", None),
    ("PG", None),
    ("", Some(InvalidName::NoLetter)),
    ("1", Some(InvalidName::NoLetter)),
    ("123", Some(InvalidName::NoLetter)),
    ("-", Some(InvalidName::NoLetter)),
    ("_", Some(InvalidName::NoLetter)),
    (".", Some(InvalidName::NoLetter)),
    ("-pg", Some(InvalidName::LeadingSpecial)),
    ("_pg", Some(InvalidName::LeadingSpecial)),
    (".pg", Some(InvalidName::LeadingSpecial)),
    ("p g", Some(InvalidName::BadCharacter)),
    ("pg/x", Some(InvalidName::BadCharacter)),
    ("pg\n", Some(InvalidName::BadCharacter)),
    ("\npg", Some(InvalidName::BadCharacter)),
    ("pg\u{e9}", Some(InvalidName::BadCharacter)),
    ("pg+", Some(InvalidName::BadCharacter)),
];

#[test]
fn names_are_validated_as_rubygems_does() {
    for &(name, error) in NAMES {
        assert_eq!(
            valid_name(name),
            error.map_or(Ok(()), |e| Err(e(name.to_owned()))),
            "{name:?}"
        );
    }
}

#[test]
fn a_substitute_is_named_gem_dash_spinel() {
    assert_eq!(substitute_name("pg").unwrap(), "pg-spinel");
    assert_eq!(substitute_name("sqlite3").unwrap(), "sqlite3-spinel");
    assert_eq!(substitute_name("nio4r").unwrap(), "nio4r-spinel");
    assert_eq!(substitute_name("puma").unwrap(), "puma-spinel");
    // The rule is mechanical: a gem that already ends in -spinel is not
    // special, and whether anything is substituted is Spinel's list's call.
    assert_eq!(
        substitute_name("bundler-spinel").unwrap(),
        "bundler-spinel-spinel"
    );
    assert_eq!(
        substitute_name("-pg"),
        Err(InvalidName::LeadingSpecial("-pg".into()))
    );
}

#[test]
fn every_substitute_name_of_a_valid_name_is_valid() {
    for &(name, error) in NAMES {
        if error.is_none() {
            let substitute = substitute_name(name).unwrap();
            assert_eq!(valid_name(&substitute), Ok(()), "{substitute:?}");
            assert!(substitute.ends_with(SUFFIX));
        }
    }
}

#[test]
fn a_substitute_version_is_the_upstream_string_and_a_revision() {
    assert_eq!(substitute_version(&v("1.5.0"), rev(1)).as_str(), "1.5.0.1");
    assert_eq!(substitute_version(&v("1.5"), rev(12)).as_str(), "1.5.12");
    assert_eq!(
        substitute_version(&v("7.1.3.4"), rev(2)).as_str(),
        "7.1.3.4.2"
    );
    let pre = substitute_version(&v("2.0.0-rc1"), rev(1));
    assert_eq!(pre.as_str(), "2.0.0.pre.rc1.1");
    assert!(pre.prerelease());
    assert!(!substitute_version(&v("1.5.0"), rev(1)).prerelease());
}

/// Locked upstream, candidate substitute version, the revision it is.
const REVISIONS: &[(&str, &str, Option<u64>)] = &[
    ("1.5.0", "1.5.0.1", Some(1)),
    ("1.5.0", "1.5.0.3", Some(3)),
    ("1.5.0", "1.5.0.10", Some(10)),
    ("1.5.0", "1.5.0", None),
    ("1.5.0", "1.5.0.0", None),
    ("1.5.0", "1.5.0.01", None),
    ("1.5.0", "1.5.0.1.2", None),
    ("1.5.0", "1.5.0.a", None),
    ("1.5.0", "1.5.0.1a", None),
    ("1.5.0", "1.5.1", None),
    ("1.5.0", "1.5.1.1", None),
    ("1.5.0", "1.5.01.1", None),
    ("1.5.0", "1.5.00", None),
    ("1.5.0", "99.0.0.1", None),
    ("1.5", "1.5.0.1", None),
    ("1.5", "1.5.1", Some(1)),
    ("1.5.0.1", "1.5.0.1.2", Some(2)),
    ("7.1.3", "7.1.3.4", Some(4)),
    ("7.1.3.4", "7.1.3.4", None),
    ("2.0.0-rc1", "2.0.0.pre.rc1.1", Some(1)),
    ("2.0.0.pre.rc1", "2.0.0.pre.rc1.1", Some(1)),
    ("2.0.0.rc1", "2.0.0.rc1.1", Some(1)),
    ("2.0.0.rc1", "2.0.0.rc.1.1", None),
    ("1.5.0", "1.5.0.18446744073709551615", Some(u64::MAX)),
    ("1.5.0", "1.5.0.18446744073709551616", None),
];

#[test]
fn revisions_follow_the_locked_string() {
    for &(upstream, candidate, revision) in REVISIONS {
        assert_eq!(
            revision_of(&v(upstream), &v(candidate)).map(NonZeroU64::get),
            revision,
            "{candidate} for {upstream}"
        );
    }
}

#[test]
fn revision_of_inverts_substitute_version() {
    for upstream in [
        "1.5.0",
        "1.5",
        "0",
        "7.1.3.4",
        "2.0.0-rc1",
        "1.0.0.a.1",
        "3.10.0",
    ] {
        for n in [1, 2, 9, 10, 1234, u64::MAX] {
            let substitute = substitute_version(&v(upstream), rev(n));
            assert_eq!(
                revision_of(&v(upstream), &substitute),
                Some(rev(n)),
                "{substitute}"
            );
        }
    }
}

/// The platforms a `<gem>-spinel` is published under (docs/package-manager.md,
/// section 6), and for each `RUBY_PLATFORM` Spinel is built with
/// (`spinel-vm/build.rs`), which of them RubyGems accepts there. Each runtime
/// accepts exactly one: Linux names its libc, because a `.spinel` extension
/// links one. Recorded from RubyGems 4.0.20 (`match_platforms?` with `ruby`
/// and the runtime).
const PUBLISHED: &[&str] = &[
    "x86_64-linux-gnu",
    "x86_64-linux-musl",
    "aarch64-linux-gnu",
    "aarch64-linux-musl",
    "arm64-darwin",
    "x86_64-darwin",
];

const RUNTIMES: &[(&str, &str)] = &[
    ("x86_64-linux", "x86_64-linux-gnu"),
    ("x86_64-linux-musl", "x86_64-linux-musl"),
    ("aarch64-linux", "aarch64-linux-gnu"),
    ("aarch64-linux-musl", "aarch64-linux-musl"),
    ("arm64-darwin24", "arm64-darwin"),
    ("arm64-darwin25", "arm64-darwin"),
    ("arm64-darwin", "arm64-darwin"),
    ("x86_64-darwin23", "x86_64-darwin"),
];

#[test]
fn each_spinel_runtime_accepts_one_published_platform() {
    for &(runtime, expected) in RUNTIMES {
        let local = GemPlatform::Platform(Platform::parse(runtime));
        let accepted: Vec<&str> = PUBLISHED
            .iter()
            .copied()
            .filter(|published| {
                let published = GemPlatform::Platform(Platform::parse(published));
                match_platforms(&published, &[GemPlatform::Ruby, local.clone()])
            })
            .collect();
        assert_eq!(accepted, [expected], "{runtime}");
    }
}

#[test]
fn a_bare_linux_build_would_also_land_on_musl() {
    // Why Linux builds name the libc: RubyGems reads a gem with no libc as one
    // that fits musl too, and a glibc-linked extension does not.
    let bare = GemPlatform::Platform(Platform::parse("x86_64-linux"));
    let musl = GemPlatform::Platform(Platform::parse("x86_64-linux-musl"));
    assert!(match_platforms(&bare, &[GemPlatform::Ruby, musl]));
}

fn picked(upstream: &str, builds: &[Build], local: &str) -> Option<(String, String)> {
    pick(&v(upstream), builds, &Platform::parse(local))
        .map(|b| (b.version.as_str().to_owned(), b.platform.to_string()))
}

fn pair(version: &str, platform: &str) -> Option<(String, String)> {
    Some((version.to_owned(), platform.to_owned()))
}

#[test]
fn pick_takes_the_local_platform_of_the_newest_revision() {
    let mut builds: Vec<Build> = PUBLISHED.iter().map(|p| build("1.5.0.1", p)).collect();
    builds.push(build("1.5.0.1", "ruby"));
    for &(runtime, expected) in RUNTIMES {
        assert_eq!(
            picked("1.5.0", &builds, runtime),
            pair("1.5.0.1", expected),
            "{runtime}"
        );
    }
    builds.push(build("1.5.0.2", "arm64-darwin"));
    assert_eq!(
        picked("1.5.0", &builds, "arm64-darwin24"),
        pair("1.5.0.2", "arm64-darwin")
    );
    assert_eq!(
        picked("1.5.0", &builds, "x86_64-linux"),
        pair("1.5.0.1", "x86_64-linux-gnu")
    );
}

#[test]
fn pick_prefers_a_native_build_to_ruby_in_the_same_revision() {
    let builds = [
        build("1.5.0.1", "ruby"),
        build("1.5.0.1", "x86_64-linux-gnu"),
    ];
    assert_eq!(
        picked("1.5.0", &builds, "x86_64-linux"),
        pair("1.5.0.1", "x86_64-linux-gnu")
    );
    assert_eq!(
        picked("1.5.0", &builds, "arm64-darwin24"),
        pair("1.5.0.1", "ruby")
    );
}

#[test]
fn pick_takes_a_newer_ruby_build_over_an_older_native_one() {
    let builds = [
        build("1.5.0.1", "x86_64-linux-gnu"),
        build("1.5.0.2", "ruby"),
    ];
    assert_eq!(
        picked("1.5.0", &builds, "x86_64-linux"),
        pair("1.5.0.2", "ruby")
    );
}

#[test]
fn pick_never_moves_the_locked_version() {
    let builds = [
        build("1.5.1.1", "x86_64-linux-gnu"),
        build("99.0.0.1", "x86_64-linux-gnu"),
        build("1.5.0", "x86_64-linux-gnu"),
        build("1.5.0.1.2", "x86_64-linux-gnu"),
        build("1.5.0.1", "x86_64-linux-musl"),
        build("1.5.0.1", "arm64-darwin"),
    ];
    assert_eq!(picked("1.5.0", &builds, "x86_64-linux"), None);
    assert_eq!(picked("1.5.0", &[], "x86_64-linux"), None);
    assert_eq!(
        picked("1.5.1", &builds, "x86_64-linux"),
        pair("1.5.1.1", "x86_64-linux-gnu")
    );
}
