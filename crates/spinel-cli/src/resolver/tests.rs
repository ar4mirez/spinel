use super::report::constraint_string;
use super::*;

fn local() -> Platform {
    Platform::parse("x86_64-linux")
}

fn v(s: &str) -> Version {
    Version::new(s).unwrap()
}

fn req(list: &[&str]) -> Requirement {
    Requirement::new(list.iter().copied()).unwrap()
}

fn platform(s: &str) -> GemPlatform {
    GemPlatform::new(Some(s), &local())
}

/// An index from `/info` texts.
fn index(gems: &[(&str, &str)]) -> Index {
    gems.iter()
        .map(|(name, info)| {
            (
                (*name).to_owned(),
                parse_info(name, info.as_bytes(), &local()).unwrap(),
            )
        })
        .collect()
}

fn dep(name: &str, requirement: &[&str]) -> GemfileDependency {
    GemfileDependency {
        name: name.to_owned(),
        requirement: req(requirement),
        platforms: Vec::new(),
        force_ruby_platform: false,
    }
}

fn request(deps: Vec<GemfileDependency>, platforms: &[&str], ruby: &str) -> Request {
    Request {
        gemfile: "Gemfile".to_owned(),
        source: "rubygems repository https://gems.example/".to_owned(),
        dependencies: deps,
        platforms: platforms.iter().map(|p| platform(p)).collect(),
        ruby_version: v(ruby),
        rubygems_version: v("4.0.16"),
    }
}

fn resolved(request: &Request, index: &Index) -> Vec<String> {
    resolve(request, index)
        .unwrap_or_else(|e| panic!("{e}"))
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn reads_info_lines_as_gem_parser_does() {
    let info = "---\n\
        1.0.0 |checksum:aa\n\
        1.1.0 rack:< 4&>= 3.0.0,tilt:~> 2.0|checksum:bb,ruby:>= 2.7&< 4,rubygems:> 1.3.1,created_at:2025-01-01T00:00:00Z\n\
        \n\
        1.1.0-java rack:>= 3|checksum:cc\n\
        2.0.0.rc1 bare\n";
    let specs = parse_info("demo", info.as_bytes(), &local()).unwrap();
    assert_eq!(specs.len(), 4);
    assert_eq!(specs[0].full_name(), "demo-1.0.0");
    assert!(specs[0].dependencies.is_empty());
    assert_eq!(specs[1].dependencies.len(), 2);
    assert_eq!(specs[1].dependencies[0].0, "rack");
    assert_eq!(specs[1].dependencies[0].1.to_string(), "< 4, >= 3.0.0");
    assert_eq!(specs[1].required_ruby_version.to_string(), ">= 2.7, < 4");
    assert_eq!(specs[1].required_rubygems_version.to_string(), "> 1.3.1");
    assert_eq!(specs[2].full_name(), "demo-1.1.0-java");
    assert_eq!(specs[2].platform, platform("java"));
    // A dependency with no requirement is `>= 0`.
    assert_eq!(specs[3].dependencies, vec![("bare".to_owned(), req(&[]))]);
    assert!(parse_info("demo", b"1.0 rack:>= x", &local()).is_err());
    assert!(parse_info("demo", b"not.a.version!", &local()).is_err());
}

#[test]
fn ranges_hold_what_requirements_are_satisfied_by() {
    let versions = [
        "0.9", "1.0.a", "1", "1.0", "1.0.1", "1.4", "1.4.4", "1.5", "1.9", "2.0.a", "2.0", "2.1",
    ];
    let requirements = [
        "= 1.0", "!= 1.0", "> 1.0", "< 1.0", ">= 1.0", "<= 1.0", "~> 1.0", "~> 1.4.4", "~> 1",
    ];
    for r in requirements {
        let requirement = req(&[r]);
        let range = requirement_to_range(&requirement);
        for version in versions {
            let expected = requirement.satisfied_by(&v(version));
            for priority in [-1, 1] {
                let held = range.contains(&Candidate::new(v(version), priority));
                assert_eq!(held, expected, "{r} holds {version} ({priority})");
            }
        }
    }
    let both = requirement_to_range(&req(&[">= 3.0.0", "< 4"]));
    assert!(both.contains(&Candidate::new(v("3.2.8"), 1)));
    assert!(!both.contains(&Candidate::new(v("4.0.0"), -1)));
}

#[test]
fn writes_ranges_as_pubgrub_ruby_does() {
    let range = |r: &[&str]| requirement_to_range(&req(r));
    assert_eq!(constraint_string(&range(&[">= 0"]), true), ">= 0");
    assert_eq!(constraint_string(&range(&["~> 2.2"]), true), "~> 2.2");
    assert_eq!(
        constraint_string(&range(&["~> 2.2"]), false),
        ">= 2.2, < 3.A"
    );
    assert_eq!(constraint_string(&range(&["= 2.6.10"]), true), "= 2.6.10");
    assert_eq!(constraint_string(&range(&["!= 1.0"]), true), "!= 1.0");
    assert_eq!(
        constraint_string(&range(&[">= 3.0.0", "< 4"]), true),
        ">= 3.0.0, < 4"
    );
    assert_eq!(
        constraint_string(&range(&["> 1", "!= 1.5"]), true),
        "> 1, != 1.5"
    );
    let either = range(&["< 1"]).union(&range(&[">= 2"]));
    assert_eq!(constraint_string(&either, true), "< 1 OR >= 2");
    assert_eq!(constraint_string(&Ranges::empty(), true), "(no versions)");
}

#[test]
fn resolves_newest_with_dependencies() {
    let index = index(&[
        ("app", "1.0 lib:~> 1.0\n2.0 lib:~> 2.0\n"),
        ("lib", "1.0\n1.5\n2.0\n2.1\n"),
    ]);
    let r = request(vec![dep("app", &[])], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&r, &index), ["app (2.0)", "lib (2.1)"]);
    let r = request(
        vec![dep("app", &[]), dep("lib", &["< 2"])],
        &["x86_64-linux"],
        "4.0.0",
    );
    assert_eq!(resolved(&r, &index), ["app (1.0)", "lib (1.5)"]);
}

#[test]
fn filters_by_ruby_and_rubygems_requirements() {
    let index = index(&[(
        "lib",
        "1.0 |ruby:>= 2.0\n2.0 |ruby:>= 3.4\n3.0 |rubygems:>= 9\n",
    )]);
    let r = request(vec![dep("lib", &[])], &["x86_64-linux"], "3.3.0");
    assert_eq!(resolved(&r, &index), ["lib (1.0)"]);
    let r = request(vec![dep("lib", &[])], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (2.0)"]);
}

#[test]
fn leaves_prereleases_out_unless_asked_or_alone() {
    let index = index(&[
        ("lib", "1.0\n2.0.beta\n"),
        ("only", "1.0.rc1\n"),
        ("needs_pre", "1.0 lib:>= 2.0.a\n"),
    ]);
    let platforms = ["x86_64-linux"];
    let r = request(vec![dep("lib", &[])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (1.0)"]);
    let r = request(vec![dep("lib", &[">= 2.0.a"])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (2.0.beta)"]);
    // A gem whose one build is a prerelease keeps it.
    let r = request(vec![dep("only", &[])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &index), ["only (1.0.rc1)"]);
    // A conflict on a gem whose prereleases were left out retries with them.
    let r = request(vec![dep("needs_pre", &[])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (2.0.beta)", "needs_pre (1.0)"]);
}

// Each expectation is what Bundler 4.0.20 did with the same /info lines and
// Gemfile, on Ruby 4.0.0, RubyGems 4.0.16 and x86_64-linux, run with
// `tests/fixtures/resolver/bundle_case.sh` (the indexes are
// `tests/fixtures/resolver/oracle/<name>/`).
#[test]
fn retries_with_prereleases_of_the_gems_bundler_relaxes() {
    let platforms = ["x86_64-linux"];
    // `needs_pre`: a gem asking for a prerelease of another. Bundler
    // retries with `lib`'s prereleases: `lib (2.0.beta)`, `needs_pre (1.0)`.
    // pubgrub-rs puts `lib` under the second cause of its first derivation,
    // behind a fact about the gaps between `needs_pre`'s versions, which is
    // why the walk follows what was learned from a single term.
    let gems = index(&[
        ("needs_pre", "1.0 lib:>= 2.0.a\n"),
        ("lib", "1.0\n2.0.beta\n"),
    ]);
    let r = request(vec![dep("needs_pre", &[])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &gems), ["lib (2.0.beta)", "needs_pre (1.0)"]);
    // `case1`: Bundler retries with `y`'s prereleases: `b (2.0)`, `c (1.0)`,
    // `y (2.0.beta)`. pubgrub-rs learns `b 2.0` cannot be used from `y`'s
    // missing versions, which pubgrub-ruby propagates instead, so `y` is on
    // Bundler's chain.
    let gems = index(&[
        ("c", "1.0 b:>= 2\n"),
        ("b", "1.0\n2.0 y:>= 2.0.a\n"),
        ("y", "1.0\n2.0.beta\n"),
    ]);
    let r = request(vec![dep("c", &[]), dep("b", &[])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &gems), ["b (2.0)", "c (1.0)", "y (2.0.beta)"]);
    // `case2`: Bundler reports the conflict, beginning `Because every version
    // of c depends on y < 3`: `c` is only in what an earlier conflict taught,
    // so its prereleases stay out, although `a 2.0.beta` asks for
    // `c >= 2.0.a`, and `c (3.0.rc1)`, which has no dependency on `y`, is
    // never taken.
    let gems = index(&[
        ("a", "2.0.beta c:>= 2.0.a,y:>= 3\n2.0 b:>= 3\n"),
        ("b", "1.0\n"),
        ("c", "3.0.rc1\n2.0.beta y:>= 1\n2.0 y:< 3\n"),
        ("y", "3.0\n2.0.beta\n"),
    ]);
    let r = request(vec![dep("a", &[])], &platforms, "4.0.0");
    let Err(Failure::Conflict(message)) = resolve(&r, &gems) else {
        panic!("resolved with a prerelease Bundler leaves out");
    };
    assert!(
        message.starts_with(
            "Could not find compatible versions\n\n\
             Because every version of c depends on y < 3\n  \
             and a < 2.0 depends on c >= 2.0.a,\n  \
             a < 2.0 requires y < 3.\n\
             And because a < 2.0 depends on y >= 3,\n  \
             a < 2.0 cannot be used."
        ),
        "{message}"
    );
}

/// A known difference from Bundler (#816): `tests/fixtures/resolver/oracle/
/// case0138`, the one of 400 random indexes (`differential.py`, seed 1) on
/// which Spinel and Bundler 4.0.20 disagree. Bundler retries with `g1`'s
/// prereleases but not `g2`'s, and reports the conflict ("g2 >= 2.0, < 3.0
/// could not be found"); Spinel also retries with `g2`'s, through the
/// single-term case of `names_to_relax`, and resolves with `g2 (2.1.rc1)`.
/// This asserts what Spinel does, so a change to it is seen; closing the
/// difference turns it into Bundler's conflict.
#[test]
fn known_difference_case0138_takes_a_prerelease_bundler_leaves_out() {
    let gems = index(&[
        (
            "g0",
            "2.1.rc1 g1:>= 0,g2:>= 2.0.a,g3:< 2.0.beta\n\
             2.0 g1:~> 2.0.a\n\
             3.0 g1:>= 1.0&< 3,g2:~> 1.0,g3:>= 1.0&< 3\n",
        ),
        (
            "g1",
            "2.0.beta g2:>= 2.0,g3:~> 2.0.a\n\
             0.9\n\
             2.1.rc1 g3:= 2.0.beta\n\
             1.0 g2:>= 0,g3:= 2.0.beta\n",
        ),
        ("g2", "3.0 g3:~> 1.0\n2.0.beta\n1.1 g3:~> 2.0.a\n2.1.rc1\n"),
        ("g3", "2.0.a\n2.1.rc1\n2.0\n"),
    ]);
    let r = request(
        vec![dep("g0", &[]), dep("g3", &[])],
        &["x86_64-linux"],
        "4.0.0",
    );
    assert_eq!(
        resolved(&r, &gems),
        ["g0 (2.0)", "g1 (2.0.beta)", "g2 (2.1.rc1)", "g3 (2.0)"]
    );
}

#[test]
fn narrows_to_builds_whose_requirements_are_equal_as_rubygems_compares_them() {
    // As `Gem::Platform.sort_and_filter_best_platform_match` narrows them
    // on RubyGems 4.0.20: the same pairs in another order, and no `ruby:`
    // beside `ruby:>= 0`, are the same requirements; `~> 2.0` and `~> 2`
    // are not.
    let specs = parse_info(
        "lib",
        b"1.0-x86_64-darwin-22 rack:< 4&>= 2\n\
          1.0-x86_64-darwin-21 rack:>= 2&< 4|ruby:>= 0\n\
          1.0-x86_64-darwin-20 rack:~> 2.0\n\
          1.0-x86_64-darwin-19 rack:~> 2\n",
        &local(),
    )
    .unwrap();
    let names = |picked: Vec<usize>| -> Vec<String> {
        picked.iter().map(|&i| specs[i].full_name()).collect()
    };
    let darwin = platform("x86_64-darwin");
    assert_eq!(
        names(best_platform_match(&[0, 1], &specs, &darwin)),
        ["lib-1.0-x86_64-darwin-22", "lib-1.0-x86_64-darwin-21"]
    );
    assert_eq!(
        names(best_platform_match(&[2, 3], &specs, &darwin)),
        ["lib-1.0-x86_64-darwin-20"]
    );
}

#[test]
fn takes_the_best_build_for_each_platform() {
    let index = index(&[
        ("same", "1.0\n1.0-java\n"),
        ("differs", "1.0\n1.0-java jruby-only:>= 0\n"),
        ("jruby-only", "1.0\n"),
        (
            "native",
            "1.0-x86_64-linux\n1.0-x86_64-linux-musl\n1.0-arm64-darwin\n",
        ),
    ]);
    let both = ["x86_64-linux", "java"];
    let r = request(vec![dep("same", &[])], &both, "4.0.0");
    assert_eq!(resolved(&r, &index), ["same (1.0)", "same (1.0-java)"]);
    // A build with dependencies of its own is a candidate of its own, and
    // the platform build is preferred.
    let r = request(vec![dep("differs", &[])], &both, "4.0.0");
    assert_eq!(
        resolved(&r, &index),
        ["differs (1.0)", "differs (1.0-java)", "jruby-only (1.0)"]
    );
    // The exact platform beats one that also installs there.
    let r = request(vec![dep("native", &[])], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&r, &index), ["native (1.0-x86_64-linux)"]);
    // A top-level gem with no build for a resolution platform is not found.
    let r = request(vec![dep("native", &[])], &["x86_64-linux", "java"], "4.0.0");
    let Err(Failure::NotFound(message)) = resolve(&r, &index) else {
        panic!("resolved without a java build");
    };
    assert!(
        message.starts_with(
            "Could not find gems matching 'native' valid for all resolution platforms \
             (x86_64-linux, java)"
        ),
        "{message}"
    );
}

#[test]
fn applies_gemfile_platforms_and_force_ruby_platform() {
    let index = index(&[("lib", "1.0\n1.0-x86_64-linux\n"), ("jr", "1.0\n")]);
    let mut jruby = dep("jr", &[]);
    jruby.platforms = vec!["jruby".to_owned()];
    let r = request(vec![dep("lib", &[]), jruby], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (1.0-x86_64-linux)"]);
    let mut ruby_only = dep("lib", &[]);
    ruby_only.force_ruby_platform = true;
    let r = request(vec![ruby_only], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (1.0)"]);
    assert_eq!(
        expanded_platforms(&["mri_33".to_owned(), "windows".to_owned()]),
        [
            GemPlatform::Ruby,
            platform("mswin32"),
            platform("mswin64"),
            platform("universal-mingw"),
        ]
    );
}

#[test]
fn ignores_builds_that_depend_on_themselves_impossibly() {
    let index = index(&[("lib", "1.0\n2.0 lib:< 2\n")]);
    let r = request(vec![dep("lib", &[])], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&r, &index), ["lib (1.0)"]);
}

#[test]
fn explains_a_conflict() {
    let index = index(&[
        ("app", "1.0 lib:~> 1.0\n1.1 lib:~> 1.0\n"),
        ("lib", "1.0\n2.0\n"),
    ]);
    let r = request(
        vec![dep("app", &[]), dep("lib", &[">= 2"])],
        &["x86_64-linux"],
        "4.0.0",
    );
    let Err(Failure::Conflict(message)) = resolve(&r, &index) else {
        panic!("resolved");
    };
    assert_eq!(
        message,
        "Could not find compatible versions\n\n\
         Because every version of app depends on lib ~> 1.0\n  \
         and Gemfile depends on app >= 0,\n  \
         lib ~> 1.0 is required.\n\
         So, because Gemfile depends on lib >= 2,\n  \
         version solving has failed."
    );
}
