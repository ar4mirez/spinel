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
        let range = solver::candidate_range(&requirement);
        for version in versions {
            let expected = requirement.satisfied_by(&v(version));
            for priority in [-1, 1] {
                let held = range.includes(&Candidate::new(v(version), priority));
                assert_eq!(held, expected, "{r} holds {version} ({priority})");
            }
        }
    }
    let both = solver::candidate_range(&req(&[">= 3.0.0", "< 4"]));
    assert!(both.includes(&Candidate::new(v("3.2.8"), 1)));
    assert!(!both.includes(&Candidate::new(v("4.0.0"), -1)));
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
// `tests/fixtures/resolver/oracle/<name>/`). The resolver relaxes the same
// gems on each, by construction: its `find_names_to_relax` walks the same
// derivation.
#[test]
fn retries_with_prereleases_of_the_gems_bundler_relaxes() {
    let platforms = ["x86_64-linux"];
    // `needs_pre`: a gem asking for a prerelease of another. Bundler
    // retries with `lib`'s prereleases: `lib (2.0.beta)`, `needs_pre (1.0)`.
    let gems = index(&[
        ("needs_pre", "1.0 lib:>= 2.0.a\n"),
        ("lib", "1.0\n2.0.beta\n"),
    ]);
    let r = request(vec![dep("needs_pre", &[])], &platforms, "4.0.0");
    assert_eq!(resolved(&r, &gems), ["lib (2.0.beta)", "needs_pre (1.0)"]);
    // `case1`: Bundler retries with `y`'s prereleases: `b (2.0)`, `c (1.0)`,
    // `y (2.0.beta)`. pubgrub-ruby propagates `y`'s missing versions, so `y`
    // is on Bundler's chain.
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
    // And explains it as Bundler does, word for word.
    assert_explained_as_bundler("case2", &message);
}

// Random indexes from `tests/fixtures/resolver/differential.py`, kept
// under `tests/fixtures/resolver/oracle/` (`case0138` is seed 1's; the
// others are named for their seed and case), on which the `pubgrub` crate
// Spinel resolved with before #816 found other failures than pubgrub-ruby,
// so relaxed other gems than Bundler's `find_names_to_relax`, and let in
// prereleases Bundler leaves out or resolved to other versions. Each test
// asserts that Spinel resolves as Bundler 4.0.20 does, or explains the
// conflict as Bundler does.

/// The index and Gemfile of `tests/fixtures/resolver/oracle/<name>`, as
/// `bundle_case.sh` serves them to Bundler.
fn oracle_case(name: &str) -> (Request, Index) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/resolver/oracle")
        .join(name);
    let mut gems = Vec::new();
    for entry in std::fs::read_dir(dir.join("info")).unwrap() {
        let path = entry.unwrap().path();
        let gem = path.file_name().unwrap().to_str().unwrap().to_owned();
        gems.push((gem, std::fs::read_to_string(&path).unwrap()));
    }
    let gems: Vec<(&str, &str)> = gems.iter().map(|(g, i)| (g.as_str(), i.as_str())).collect();
    // `gem "name", "requirement", ...`: the quoted strings.
    let deps = std::fs::read_to_string(dir.join("Gemfile.gems"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
            dep(quoted[0], &quoted[1..])
        })
        .collect();
    (request(deps, &["x86_64-linux"], "4.0.0"), index(&gems))
}

/// Bundler reports a conflict.
const CONFLICT: Option<&[&str]> = None;

/// Asserts that `message` is Bundler's explanation of the conflict in
/// `tests/fixtures/resolver/oracle/<case>`, as `bundle_case.sh` printed it
/// (`bundler.txt`), word for word but for its source: Bundler's
/// `rubygems repository @SOURCE@/ or installed locally` is the tests'
/// `rubygems repository https://gems.example/`, as Spinel has no locally
/// installed gems to resolve with.
fn assert_explained_as_bundler(case: &str, message: &str) {
    let bundler = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/resolver/oracle")
            .join(case)
            .join("bundler.txt"),
    )
    .unwrap();
    let bundler = bundler.replace("@SOURCE@/ or installed locally", "https://gems.example/");
    assert_eq!(format!("{message}\n"), bundler, "{case}");
}

/// The outcome as the tests below spell one.
fn as_strs(outcome: &Option<Vec<String>>) -> Option<Vec<&str>> {
    outcome
        .as_ref()
        .map(|s| s.iter().map(String::as_str).collect())
}

/// Asserts that Spinel resolves `case` as Bundler 4.0.20 does, as
/// `bundler` records it, or explains its conflict as Bundler does (`None`).
fn resolves_as_bundler(case: &str, bundler: Option<&[&str]>) {
    let (r, gems) = oracle_case(case);
    let ours: Option<Vec<String>> = match resolve(&r, &gems) {
        Ok(specs) => Some(specs.iter().map(ToString::to_string).collect()),
        Err(Failure::Conflict(message)) => {
            assert_explained_as_bundler(case, &message);
            None
        }
        Err(e) => panic!("{case}: {e}"),
    };
    assert_eq!(
        as_strs(&ours).as_deref(),
        bundler,
        "{case}: not Bundler's resolution"
    );
}

macro_rules! resolves_as_bundler {
    ($($(#[$doc:meta])* $test:ident: $case:literal, $bundler:expr;)*) => {
        $(
            $(#[$doc])*
            #[test]
            fn $test() {
                resolves_as_bundler($case, $bundler);
            }
        )*
    };
}

resolves_as_bundler! {
    /// Bundler retries with `g1`'s and `g3`'s prereleases and reports the
    /// conflict; the `pubgrub` crate also relaxed `g0`, then `g2`, and
    /// resolved.
    resolves_case0138_as_bundler_does: "case0138", CONFLICT;
    /// Bundler relaxes `g0` and `g4` and reports the conflict.
    resolves_seed816_case0015_as_bundler_does: "seed816-case0015", CONFLICT;
    /// Bundler relaxes `g0`, `g2` and `g4`, then `g3`, and reports the
    /// conflict.
    resolves_seed816_case0358_as_bundler_does: "seed816-case0358", CONFLICT;
    /// Bundler relaxes `g3` and `g4` and reports the conflict.
    resolves_seed816_case0393_as_bundler_does: "seed816-case0393", CONFLICT;
    /// Bundler finds nothing to relax and reports the conflict.
    resolves_seed816_case0669_as_bundler_does: "seed816-case0669", CONFLICT;
    /// Bundler relaxes `g1`, `g2` and `g3`, then `g4`, and reports the
    /// conflict.
    resolves_seed816_case0969_as_bundler_does: "seed816-case0969", CONFLICT;
    /// Bundler resolves after relaxing `g1`.
    resolves_seed816_case0231_as_bundler_does: "seed816-case0231",
        Some(&["g0 (2.0)", "g1 (2.1.rc1)", "g2 (3.0)", "g3 (1.1)"]);
    /// Bundler resolves after relaxing `g0` and `g2`.
    resolves_seed816_case0377_as_bundler_does: "seed816-case0377",
        Some(&["g0 (2.1.rc1)"]);
    /// Bundler resolves after relaxing `g0` and `g3`.
    resolves_seed4242_case0250_as_bundler_does: "seed4242-case0250",
        Some(&["g0 (3.0.pre)", "g1 (1.0)", "g2 (2.0)", "g3 (3.0)"]);
    /// Bundler resolves after relaxing `g4`.
    resolves_seed4242_case0536_as_bundler_does: "seed4242-case0536",
        Some(&["g0 (1.1)", "g1 (2.0)", "g3 (1.0)", "g4 (2.0.a)"]);
    /// Bundler resolves after relaxing `g2`.
    resolves_seed4242_case0872_as_bundler_does: "seed4242-case0872",
        Some(&["g0 (1.1)", "g2 (2.0.beta)", "g3 (1.1)", "g4 (2.0)"]);
    /// Bundler's first try fails and it resolves after relaxing `g2`, where
    /// the `pubgrub` crate's first try resolved, to other versions.
    resolves_seed4242_case0924_as_bundler_does: "seed4242-case0924",
        Some(&["g0 (2.0)", "g1 (3.0)", "g2 (2.0.beta)", "g3 (1.0)"]);
}

// The port of pubgrub-ruby (#816), fed by Bundler's source side
// (`resolver/source.rs`).

/// What the resolver resolves `r` to, and every line pubgrub-ruby
/// logged on the way, as `DEBUG_RESOLVER` makes Bundler print them.
fn pubgrub_ruby_log(r: &Request, gems: &Index) -> (Result<Vec<String>, Failure>, Vec<String>) {
    let (result, lines, _) = pubgrub_ruby_tries(r, gems);
    (result, lines)
}

/// [`pubgrub_ruby_log`], and what each failed try's `find_names_to_relax`
/// found, as `relax_hook.rb` logs Bundler's.
fn pubgrub_ruby_tries(
    r: &Request,
    gems: &Index,
) -> (Result<Vec<String>, Failure>, Vec<String>, Vec<String>) {
    let lines = std::rc::Rc::new(RefCell::new(Vec::new()));
    let sink = lines.clone();
    let logger: solver::Logger =
        Box::new(move |_, line: &str| sink.borrow_mut().push(line.to_owned()));
    let mut relaxed = Vec::new();
    let result = solve(
        r,
        gems,
        Locked::default(),
        Some(logger),
        |names| relaxed.push(names.to_json()),
        resolved_builds,
    )
    .map(|specs| specs.iter().map(ToString::to_string).collect());
    let lines = lines.borrow().clone();
    (result, lines, relaxed)
}

/// On every case in `tests/fixtures/resolver/oracle/`, the port logs what
/// Bundler 4.0.20 logged on every try, line for line: the facts it learned,
/// each decision and derivation, each conflict and how it was resolved
/// (`resolution.log`, recorded with `decision_log.py --write`; a line's
/// trailing space, after `attempting` a gem with no version left, is not
/// kept). Between tries, its `find_names_to_relax` finds what Bundler's
/// found (`relax.log`, as `relax_hook.rb` logs it).
#[test]
fn the_pubgrub_ruby_port_decides_and_relaxes_as_bundler_does_on_every_try() {
    let oracle =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolver/oracle");
    let mut cases: Vec<String> = std::fs::read_dir(&oracle)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    cases.sort();
    assert_eq!(cases.len(), 15, "{cases:?}");
    for case in cases {
        let (mut r, gems) = oracle_case(&case);
        // As `decision_log.py` spells Bundler's source.
        r.source = "rubygems repository @SOURCE@".to_owned();
        let (_, ours, relaxed) = pubgrub_ruby_tries(&r, &gems);
        let bundler = std::fs::read_to_string(oracle.join(&case).join("resolution.log")).unwrap();
        let ours: Vec<&str> = ours.iter().map(|line| line.trim_end()).collect();
        let bundler: Vec<&str> = bundler.lines().collect();
        assert_eq!(ours, bundler, "{case}: not Bundler's resolution");
        let bundler = std::fs::read_to_string(oracle.join(&case).join("relax.log")).unwrap();
        let bundler: Vec<&str> = bundler.lines().collect();
        assert_eq!(relaxed, bundler, "{case}: not what Bundler relaxed");
    }
}

/// Where Bundler needs no retry, the first try's decisions are the
/// resolution, and nothing is relaxed.
#[test]
fn resolves_on_the_first_try_where_bundler_needs_no_retry() {
    let platforms = ["x86_64-linux"];
    let gems = index(&[
        ("app", "1.0 lib:~> 1.0\n2.0 lib:~> 2.0\n"),
        ("lib", "1.0\n1.5\n2.0\n2.1\n"),
        (
            "old",
            "1.0 |ruby:>= 2.0\n2.0 |ruby:>= 3.4\n3.0 |rubygems:>= 9\n",
        ),
        ("pre", "1.0\n2.0.beta\n"),
        ("same", "1.0\n1.0-java\n"),
        ("differs", "1.0\n1.0-java jruby-only:>= 0\n"),
        ("jruby-only", "1.0\n"),
    ]);
    for (deps, platforms, ruby, expected) in [
        (
            vec![dep("app", &[])],
            &platforms[..],
            "4.0.0",
            &["app (2.0)", "lib (2.1)"][..],
        ),
        (
            vec![dep("app", &[]), dep("lib", &["< 2"])],
            &platforms,
            "4.0.0",
            &["app (1.0)", "lib (1.5)"],
        ),
        (vec![dep("old", &[])], &platforms, "3.3.0", &["old (1.0)"]),
        (vec![dep("old", &[])], &platforms, "4.0.0", &["old (2.0)"]),
        (vec![dep("pre", &[])], &platforms, "4.0.0", &["pre (1.0)"]),
        (
            vec![dep("pre", &[">= 2.0.a"])],
            &platforms,
            "4.0.0",
            &["pre (2.0.beta)"],
        ),
        (
            vec![dep("same", &[]), dep("differs", &[])],
            &["x86_64-linux", "java"],
            "4.0.0",
            &[
                "differs (1.0)",
                "differs (1.0-java)",
                "jruby-only (1.0)",
                "same (1.0)",
                "same (1.0-java)",
            ],
        ),
    ] {
        let r = request(deps, platforms, ruby);
        let (ours, _, relaxed) = pubgrub_ruby_tries(&r, &gems);
        assert_eq!(ours.unwrap(), expected);
        assert!(relaxed.is_empty(), "{expected:?}: retried");
    }
}

/// `Resolver#incompatibilities_for`: a dependency is stated for the run of
/// versions that share it, open below when the run reaches the first
/// version and above when it reaches the last.
#[test]
fn the_pubgrub_ruby_port_states_a_dependency_for_the_versions_that_share_it() {
    let gems = index(&[
        (
            "app",
            "1.0 lib:~> 1.0\n2.0 lib:~> 2.0\n3.0 lib:~> 2.0\n4.0 lib:>= 3\n",
        ),
        ("lib", "1.0\n2.0\n3.0\n"),
    ]);
    let facts = |requirement: &[&str]| {
        let r = request(vec![dep("app", requirement)], &["x86_64-linux"], "4.0.0");
        let (_, log) = pubgrub_ruby_log(&r, &gems);
        log.into_iter()
            .filter(|line| line.starts_with("fact: app"))
            .collect::<Vec<_>>()
    };
    assert_eq!(facts(&[]), ["fact: app >= 4.0 depends on lib >= 3"]);
    assert_eq!(
        facts(&["< 4"]),
        ["fact: app >= 2.0, < 4.0 depends on lib ~> 2.0"]
    );
    assert_eq!(facts(&["< 2"]), ["fact: app < 2.0 depends on lib ~> 1.0"]);
}

/// `Resolver#no_versions_incompatibility_for`: what has no version is
/// worded as Bundler words it, naming the resolution platforms when the
/// source has builds for others.
#[test]
fn the_pubgrub_ruby_port_explains_a_missing_version_as_bundler_does() {
    let gems = index(&[
        ("app", "1.0 lib:>= 9\n"),
        ("native_app", "1.0 native:>= 1\n"),
        ("native", "1.0-java\n"),
        ("lib", "1.0\n"),
    ]);
    let fact = |gem: &str| {
        let r = request(vec![dep(gem, &[])], &["x86_64-linux"], "4.0.0");
        let (result, log) = pubgrub_ruby_log(&r, &gems);
        assert!(matches!(result, Err(Failure::Conflict(_))), "{gem}");
        log.into_iter()
            .filter(|line| line.starts_with("fact: ") && line.contains("could not be found"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fact("app"),
        ["fact: lib >= 9 could not be found in rubygems repository https://gems.example/"]
    );
    assert_eq!(
        fact("native_app"),
        [
            "fact: native >= 1 could not be found in rubygems repository https://gems.example/ \
          for any resolution platforms (x86_64-linux)"
        ]
    );
    // `find_names_to_relax` picks up those builds as the extended
    // explanation, which follows the explanation.
    let r = request(vec![dep("native_app", &[])], &["x86_64-linux"], "4.0.0");
    let (result, _) = pubgrub_ruby_log(&r, &gems);
    let Err(Failure::Conflict(message)) = result else {
        panic!("native_app resolved");
    };
    assert!(
        message.ends_with(
            "\n\nThe source contains the following gems matching 'native (>= 1)':\n  \
             * native-1.0-java"
        ),
        "{message}"
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

/// `CurrentRuby` on a Ruby counted as `ruby` (#72): `ruby` and the
/// `ruby_NN` of its own minor version, and no other engine's name.
#[test]
fn counts_spinel_as_ruby_of_its_own_version() {
    for (name, here) in [
        ("ruby", true),
        ("ruby_40", true),
        ("ruby_33", false),
        ("ruby_4", false),
        ("ruby_400", false),
        ("ruby_", false),
        ("mri", false),
        ("mri_40", false),
        ("truffleruby", false),
        ("jruby", false),
        ("windows_40", false),
    ] {
        assert_eq!(ruby_platform_here(name, "4.0.3"), here, "{name}");
    }
    assert!(ruby_platform_here("ruby_34", "3.4.9"));
    assert!(!ruby_platform_here("ruby_34", "13.4.0"));
    let mut dep = dep("lib", &[]);
    assert!(dep.current_platform());
    dep.platforms = vec!["mri".to_owned(), "ruby_33".to_owned()];
    assert!(!dep.current_platform());
    dep.platforms.push("ruby_40".to_owned());
    assert!(dep.current_platform());
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

/// What `resolve_locked` chose, by `full_name`.
fn resolved_locked(request: &Request, index: &Index, locked: &Locked) -> Vec<String> {
    let mut names: Vec<String> = resolve_locked(request, index, locked)
        .unwrap_or_else(|e| panic!("{e}"))
        .iter()
        .map(Spec::full_name)
        .collect();
    names.sort();
    names
}

/// A lockfile's builds of one gem, as `crate::locked` hands them over.
fn locked_of(index: &Index, name: &str, version: &str) -> Locked {
    let base: Vec<Spec> = index[name]
        .iter()
        .filter(|s| s.version.as_str() == version)
        .cloned()
        .collect();
    Locked {
        versions: [(name.to_owned(), v(version))].into_iter().collect(),
        requirements: [(name.to_owned(), req(&[&format!(">= {version}")]))]
            .into_iter()
            .collect(),
        locked_specs: base.clone(),
        base,
        ..Locked::default()
    }
}

#[test]
fn a_locked_prerelease_lets_the_gems_prereleases_in() {
    let index = index(&[("g", "1.0\n2.0.pre\n")]);
    let request = request(vec![dep("g", &[])], &["x86_64-linux"], "4.0.0");
    assert_eq!(resolved(&request, &index), ["g (1.0)"]);
    let locked = locked_of(&index, "g", "2.0.pre");
    assert_eq!(resolved_locked(&request, &index, &locked), ["g-2.0.pre"]);
}

#[test]
fn the_locked_version_is_tried_first_and_its_floor_gives_way_in_a_conflict() {
    let index = index(&[("g", "1.0\n2.0\n3.0\n"), ("h", "1.0 g:< 2\n")]);
    let request = request(vec![dep("g", &[])], &["x86_64-linux"], "4.0.0");
    let locked = locked_of(&index, "g", "2.0");
    assert_eq!(resolved_locked(&request, &index, &locked), ["g-2.0"]);
    // A new gem that needs g below its floor: the floor goes on the retry.
    let mut request = request.clone();
    request.dependencies.push(dep("h", &[]));
    assert_eq!(
        resolved_locked(&request, &index, &locked),
        ["g-1.0", "h-1.0"]
    );
}

#[test]
fn unlock_names_loosens_indirect_pins_then_pins_then_floors() {
    let index = index(&[("a", "1.0\n"), ("b", "2.0\n"), ("c", "3.0\n")]);
    let mut locked = Locked {
        base: index.values().flatten().cloned().collect(),
        requirements: [
            ("a".to_owned(), req(&["= 1.0"])),
            ("b".to_owned(), req(&["= 2.0"])),
            ("c".to_owned(), req(&[">= 3.0"])),
        ]
        .into_iter()
        .collect(),
        ..Locked::default()
    };
    let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let reqs = |l: &Locked| {
        let mut r: Vec<String> = l
            .requirements
            .iter()
            .map(|(name, req)| format!("{name} {req}"))
            .collect();
        r.sort();
        r
    };
    // `b` is pinned and the Gemfile does not name it: it goes first.
    locked.unlock_names(&names(&["a", "b", "c"]), &["a"]);
    assert_eq!(reqs(&locked), ["a = 1.0", "b >= 2.0", "c >= 3.0"]);
    assert!(!locked.base.iter().any(|s| s.name == "b"));
    // Then the pins the Gemfile names.
    locked.unlock_names(&names(&["a", "c"]), &["a"]);
    assert_eq!(reqs(&locked), ["a >= 1.0", "b >= 2.0", "c >= 3.0"]);
    // With no pins left, the requirements go.
    locked.unlock_names(&names(&["a", "c"]), &["a"]);
    assert_eq!(reqs(&locked), ["b >= 2.0"]);
}

#[test]
fn a_locked_platform_build_is_preferred_on_a_platform_the_lockfile_has() {
    let index = index(&[("g", "1.0\n1.0-x86_64-linux\n1.0-x86_64-linux-gnu\n")]);
    let request = request(vec![dep("g", &[])], &["x86_64-linux-gnu"], "4.0.0");
    let mut locked = locked_of(&index, "g", "1.0");
    locked
        .base
        .retain(|s| s.platform == platform("x86_64-linux"));
    locked.locked_specs = locked.base.clone();
    // The index's exact build without the lockfile; the locked one with it.
    let unlocked = resolved_locked(&request, &index, &Locked::default());
    assert_eq!(unlocked, ["g-1.0-x86_64-linux-gnu"]);
    assert_eq!(
        resolved_locked(&request, &index, &locked),
        ["g-1.0-x86_64-linux"]
    );
    // Not on a platform new to the lockfile: the index's exact build is
    // chosen there, and the locked build of the same version comes back
    // with it as an additional variant, for `SpecSet#for` to choose from.
    locked.new_platforms = vec![platform("x86_64-linux-gnu")];
    assert_eq!(
        resolved_locked(&request, &index, &locked),
        ["g-1.0-x86_64-linux", "g-1.0-x86_64-linux-gnu"]
    );
}
