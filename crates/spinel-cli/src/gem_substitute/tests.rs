//! The naming and selection rules of docs/package-manager.md, section 6.

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

#[test]
fn a_substitute_is_named_gem_dash_spinel() {
    assert_eq!(substitute_name("pg"), "pg-spinel");
    assert_eq!(substitute_name("sqlite3"), "sqlite3-spinel");
    assert_eq!(substitute_name("nio4r"), "nio4r-spinel");
    assert_eq!(substitute_name("puma"), "puma-spinel");
    // The rule is mechanical: a gem that already ends in -spinel is not
    // special, and whether anything is substituted is Spinel's list's call.
    assert_eq!(substitute_name("bundler-spinel"), "bundler-spinel-spinel");
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

#[test]
fn only_listed_gems_are_substituted() {
    let list = List::new(Some(" widget, ,gadget,widget"));
    assert!(list.contains("widget"));
    assert!(list.contains("gadget"));
    assert!(!list.contains("pg"));
    assert_eq!(list.upstream_of("widget-spinel"), Some("widget"));
    assert_eq!(list.upstream_of("widget"), None);
    // A name that ends in -spinel is nobody's substitute unless what it
    // stands in for is listed.
    assert_eq!(list.upstream_of("bundler-spinel"), None);
    assert_eq!(List::new(None), List::new(Some("")));
}

#[test]
fn a_substitute_may_need_only_what_upstream_needs_at_the_locked_versions() {
    let dep = |name: &str, req: &str| (name.to_owned(), Requirement::new([req]).unwrap());
    let gizmo = v("1.0.0");
    let base64 = v("0.2.0");
    let locked = |name: &str| match name {
        "gizmo" => Some(&gizmo),
        "base64" => Some(&base64),
        _ => None,
    };
    let upstream = [dep("gizmo", "~> 1.0"), dep("base64", ">= 0")];
    let names = |unlocked: Vec<Unlocked<'_, '_>>| -> Vec<(String, String, Option<String>)> {
        unlocked
            .iter()
            .map(|u| {
                (
                    u.name.to_owned(),
                    u.requirement.to_string(),
                    u.locked.map(Version::to_string),
                )
            })
            .collect()
    };

    // Met: upstream depends on it and the locked version satisfies it.
    assert!(unlocked_dependencies(&[dep("gizmo", ">= 1")], &upstream, locked).is_empty());
    assert!(unlocked_dependencies(&[], &upstream, locked).is_empty());
    // Name missing: upstream does not depend on it, so nothing is locked for
    // it, whatever else the lockfile holds.
    assert_eq!(
        names(unlocked_dependencies(
            &[dep("ffi", ">= 0")],
            &upstream,
            locked
        )),
        [("ffi".to_owned(), ">= 0".to_owned(), None)]
    );
    // Requirement unmet: the locked version does not satisfy it.
    assert_eq!(
        names(unlocked_dependencies(
            &[dep("gizmo", ">= 2")],
            &upstream,
            locked
        )),
        [(
            "gizmo".to_owned(),
            ">= 2".to_owned(),
            Some("1.0.0".to_owned())
        )]
    );
    // Every one of them, in the substitute's order.
    let substitute = [
        dep("ffi", ">= 0"),
        dep("gizmo", ">= 2"),
        dep("base64", ">= 0"),
        dep("extra", "~> 1.0"),
    ];
    assert_eq!(
        names(unlocked_dependencies(&substitute, &upstream, locked)),
        [
            ("ffi".to_owned(), ">= 0".to_owned(), None),
            (
                "gizmo".to_owned(),
                ">= 2".to_owned(),
                Some("1.0.0".to_owned())
            ),
            ("extra".to_owned(), "~> 1.0".to_owned(), None),
        ]
    );
}
