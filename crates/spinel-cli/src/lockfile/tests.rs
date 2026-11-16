use super::*;
use crate::resolver::{GemfileDependency, parse_info, resolve};

fn local() -> Platform {
    Platform::parse("x86_64-linux")
}

fn parse(text: &str) -> Lockfile {
    Lockfile::parse(text, "Gemfile.lock", &local()).unwrap_or_else(|e| panic!("{e}"))
}

fn round_trip(text: &str) -> String {
    parse(text).write()
}

/// Each line as Ruby's regexp engine matches `NAME_VERSION`: recorded from
/// Bundler 4.0.20's pattern on Ruby 4.0.9, as
/// `(spaces, name, version, platform, pinned, checksum)`.
#[test]
fn name_version_matches_as_ruby_does() {
    type Expected = Option<(
        usize,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        bool,
        Option<&'static str>,
    )>;
    let table: &[(&str, Expected)] = &[
        (
            "    rack (3.2.8)",
            Some((4, "rack", Some("3.2.8"), None, false, None)),
        ),
        (
            "      rack (>= 3.0.0, < 4)",
            Some((6, "rack", Some(">= 3.0.0, < 4"), None, false, None)),
        ),
        (
            "    nokogiri (1.19.7-x86_64-linux-gnu)",
            Some((
                4,
                "nokogiri",
                Some("1.19.7"),
                Some("x86_64-linux-gnu"),
                false,
                None,
            )),
        ),
        (
            "  json (3.1.0-java) sha256=abc",
            Some((
                2,
                "json",
                Some("3.1.0"),
                Some("java"),
                false,
                Some("sha256=abc"),
            )),
        ),
        ("  git_tool!", Some((2, "git_tool", None, None, true, None))),
        (
            "  rack (~> 3.2)!",
            Some((2, "rack", Some("~> 3.2"), None, true, None)),
        ),
        (
            "  local_tool (0.1.0)",
            Some((2, "local_tool", Some("0.1.0"), None, false, None)),
        ),
        (
            "  rack (3.2.8) sha256=a,sha512=b",
            Some((
                2,
                "rack",
                Some("3.2.8"),
                None,
                false,
                Some("sha256=a,sha512=b"),
            )),
        ),
        ("  rack", Some((2, "rack", None, None, false, None))),
        ("   rack", None),
        ("        rack", None),
        ("  a b", Some((2, "a", None, None, false, Some("b")))),
        ("  a b c", Some((2, "a b", None, None, false, Some("c")))),
        (
            "  foo (1.0-x-y) (2)",
            Some((2, "foo", Some("1.0"), Some("x-y) (2"), false, None)),
        ),
        (
            "  foo (1.0))",
            Some((2, "foo", Some("1.0)"), None, false, None)),
        ),
        (
            "  foo (1.0-java)) x",
            Some((2, "foo", Some("1.0"), Some("java)"), false, Some("x"))),
        ),
        (
            "  weird (name) (1.0)",
            Some((2, "weird", Some("name) (1.0"), None, false, None)),
        ),
        (
            "  foo (1.0)! sha256=x",
            Some((2, "foo", Some("1.0"), None, true, Some("sha256=x"))),
        ),
    ];
    for (line, expected) in table {
        let got = name_version(line).map(|m| {
            (
                m.spaces, m.name, m.version, m.platform, m.pinned, m.checksum,
            )
        });
        assert_eq!(got, *expected, "{line:?}");
    }
}

const SORTED: &str = "\
GEM
  remote: https://rubygems.org/
  specs:
    nokogiri (1.19.7-x86_64-linux-gnu)
      racc (~> 1.4)
    rack (3.2.8)
    rack-session (2.1.2)
      base64 (>= 0.1.0)
      rack (>= 3.0.0)

PLATFORMS
  ruby
  x86_64-linux-gnu

DEPENDENCIES
  nokogiri (~> 1.19)
  rack (>= 3.0, < 4)
  rack-session

CHECKSUMS
  nokogiri (1.19.7-x86_64-linux-gnu) sha256=25865ccb462f714e712fdf8906fde07849cb6704a808e3d629d75baa5af5dd3b
  rack (3.2.8) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7
  rack-session (2.1.2)

BUNDLED WITH
  4.0.20
";

#[test]
fn a_bundler_lockfile_comes_back_unchanged() {
    assert_eq!(round_trip(SORTED), SORTED);
}

/// The writer sorts as `LockfileGenerator` does, whatever order it reads:
/// it does not copy lines through.
#[test]
fn writes_in_bundlers_order_whatever_it_read() {
    let shuffled = "\
GEM
  remote: https://rubygems.org
  specs:
    rack-session (2.1.2)
      rack (>= 3.0.0)
      base64 (>= 0.1.0)
    rack (3.2.8)
    nokogiri (1.19.7-x86_64-linux-gnu)
      racc (~>1.4)

PLATFORMS
  x86_64-linux-gnu
  ruby

DEPENDENCIES
  rack-session
  rack (< 4, >= 3.0)
  nokogiri (~> 1.19)

CHECKSUMS
  rack (3.2.8) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7
  nokogiri (1.19.7-x86_64-linux-gnu) sha256=25865ccb462f714e712fdf8906fde07849cb6704a808e3d629d75baa5af5dd3b

BUNDLED WITH
   4.0.20
";
    assert_eq!(round_trip(shuffled), SORTED);
}

/// `lock_name` and `full_name` sort apart: `rack (` before `rack-session (`
/// by the space, `rack-3` before `rack-session-2` by the digit; and a
/// platform build after its ruby build.
#[test]
fn sorts_specs_by_full_name_and_checksums_by_line() {
    let text = "\
GEM
  remote: https://rubygems.org/
  specs:
    a-b (1.0)
    a (2.0-java)
    a (2.0)

PLATFORMS
  java
  ruby

DEPENDENCIES
  a
  a-b

CHECKSUMS
  a (2.0)
  a (2.0-java)
  a-b (1.0)

BUNDLED WITH
  4.0.20
";
    let written = round_trip(text);
    let specs: Vec<&str> = written
        .lines()
        .skip(3)
        .take_while(|l| !l.is_empty())
        .collect();
    assert_eq!(specs, ["    a (2.0)", "    a (2.0-java)", "    a-b (1.0)"]);
    assert!(written.contains("CHECKSUMS\n  a (2.0)\n  a (2.0-java)\n  a-b (1.0)\n"));
}

/// GIT and PATH sections are read and written whole, before the GEM one as
/// the lockfile has them; a default glob is not written, a pinned
/// dependency keeps its `!` only when a spec is locked for it.
#[test]
fn keeps_git_and_path_sources() {
    let text = "\
GIT
  remote: https://github.com/example/tool.git
  revision: 163dd0329101b222853e11d28773fddf8adeeb37
  ref: 163dd03
  branch: main
  tag: v1
  submodules: true
  glob: {,*,*/*}.gemspec
  specs:
    tool (1.2.0)
      rack (~> 3.0)

PATH
  remote: vendor/local_tool
  glob: gems/*/*.gemspec
  specs:
    local_tool (0.1.0)

GEM
  remote: https://rubygems.org/
  specs:
    rack (3.2.8)

PLATFORMS
  ruby

DEPENDENCIES
  ghost!
  local_tool!
  rack
  tool!

BUNDLED WITH
  4.0.20
";
    let written = round_trip(text);
    assert!(written.starts_with(
        "GIT
  remote: https://github.com/example/tool.git
  revision: 163dd0329101b222853e11d28773fddf8adeeb37
  ref: 163dd03
  branch: main
  tag: v1
  submodules: true
  specs:
    tool (1.2.0)
      rack (~> 3.0)

PATH
  remote: vendor/local_tool
  glob: gems/*/*.gemspec
  specs:
    local_tool (0.1.0)

GEM
"
    ));
    assert!(written.contains("DEPENDENCIES\n  ghost\n  local_tool!\n  rack\n  tool!\n"));
    // No CHECKSUMS section was read, so none is written.
    assert!(!written.contains("CHECKSUMS"));
}

#[test]
fn keeps_a_plugin_sources_options() {
    let text = "\
PLUGIN SOURCE
  remote: https://example.com/plugin
  type: mytype
  extra: kept
  specs:
    plugged (1.0)

PLATFORMS
  ruby

DEPENDENCIES
  plugged!

BUNDLED WITH
  4.0.20
";
    assert_eq!(round_trip(text), text);
}

/// `CHECKSUMS`: a base64 sha256 is kept as hex, a checksum of a gem the
/// lockfile does not lock is dropped, one of `bundler` is kept, and a locked
/// gem with no line gets one with no checksum.
#[test]
fn checksums_as_bundler_keeps_them() {
    let text = "\
GEM
  remote: https://rubygems.org/
  specs:
    rack (3.2.8)
    rackup (2.3.1)

PLATFORMS
  ruby

DEPENDENCIES
  rack
  rackup

CHECKSUMS
  bundler (4.0.20) sha256=Bn7kfR/qx4QzbjSxa4HAyLmfvYPFWnBaUyqj6sNR1Xs=
  ghost (1.0) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7
  rack (3.2.8) sha512=bb,sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7

BUNDLED WITH
  4.0.20
";
    let written = round_trip(text);
    assert!(
        written.contains(
            "CHECKSUMS
  bundler (4.0.20) sha256=067ee47d1feac78433 6e34b16b81c0c8b99fbd83c55a705a532aa3eac351d57b
  rack (3.2.8) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7,sha512=bb
  rackup (2.3.1)
"
            .replace("433 6e", "4336e")
            .as_str()
        ),
        "{written}"
    );
}

/// Bundler 4.0.20's `ChecksumMismatchError` for this lockfile begins with
/// these lines; the instructions it goes on with are Bundler's commands.
#[test]
fn two_digests_of_one_algorithm_are_an_error() {
    let text = "\
GEM
  remote: https://rubygems.org/
  specs:
    rack (3.2.8)

CHECKSUMS
  rack (3.2.8) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7
  rack (3.2.8) sha256=0000000000000000000000000000000000000000000000000000000000000000
";
    let err = Lockfile::parse(text, "Gemfile.lock", &local()).unwrap_err();
    assert_eq!(
        err.0,
        "Bundler found mismatched checksums. This is a potential security risk.
  rack (3.2.8) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7
    from the lockfile CHECKSUMS at Gemfile.lock:7:16
  rack (3.2.8) sha256=0000000000000000000000000000000000000000000000000000000000000000
    from the lockfile CHECKSUMS at Gemfile.lock:8:16"
    );
}

#[test]
fn reads_crlf_and_writes_lf() {
    assert_eq!(round_trip(&SORTED.replace('\n', "\r\n")), SORTED);
}

#[test]
fn a_lockfile_with_no_bundled_with_gets_the_emulated_bundler() {
    let text = SORTED.replace("\nBUNDLED WITH\n  4.0.20\n", "");
    assert_eq!(round_trip(&text), SORTED);
}

#[test]
fn unknown_sections_are_not_written() {
    let text = SORTED.replace(
        "\nBUNDLED WITH",
        "\nFUTURE SECTION\n  something\n\nBUNDLED WITH",
    );
    assert_eq!(round_trip(&text), SORTED);
}

#[test]
fn ruby_version_is_kept() {
    let text = SORTED.replace(
        "\nBUNDLED WITH",
        "\nRUBY VERSION\n  ruby 3.4.1 (jruby 10.0.0.0)\n\nBUNDLED WITH",
    );
    assert_eq!(round_trip(&text), text);
}

#[test]
fn merge_conflicts_are_refused_as_bundler_refuses_them() {
    let text = format!("<<<<<<< HEAD\n{SORTED}=======\n{SORTED}>>>>>>> branch\n");
    let err = Lockfile::parse(&text, "Gemfile.lock", &local()).unwrap_err();
    assert_eq!(
        err.0,
        "Your Gemfile.lock contains merge conflicts.\n\
         Run `git checkout HEAD -- Gemfile.lock` first to get a clean lock."
    );
}

#[test]
fn an_unreadable_version_names_its_line() {
    let text = SORTED.replace(
        "rack (3.2.8)\n    rack-session",
        "rack (3..2)\n    rack-session",
    );
    let err = Lockfile::parse(&text, "Gemfile.lock", &local()).unwrap_err();
    // Bundler 4.0.20's words for it.
    assert_eq!(
        err.0,
        "Your lockfile is unreadable. Run `rm Gemfile.lock` and then `bundle install` to \
         generate a new lockfile. The error occurred while evaluating Gemfile.lock:6:1"
    );
}

#[test]
fn credentials_are_not_written() {
    assert_eq!(
        credless("https://user:secret@gems.example/private"),
        "https://gems.example/private"
    );
    assert_eq!(credless("https://gems.example/"), "https://gems.example/");
}

// A new lockfile.

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

fn request(gems: &[&str], platforms: &[&str]) -> Request {
    Request {
        gemfile: "Gemfile".to_owned(),
        source: "rubygems repository https://gems.example/".to_owned(),
        dependencies: gems
            .iter()
            .map(|name| GemfileDependency {
                name: (*name).to_owned(),
                requirement: Requirement::default(),
                platforms: Vec::new(),
                force_ruby_platform: false,
            })
            .collect(),
        platforms: platforms
            .iter()
            .map(|p| GemPlatform::new(Some(p), &local()))
            .collect(),
        ruby_version: Version::new("4.0.9").unwrap(),
        rubygems_version: Version::new("4.0.20").unwrap(),
    }
}

fn new_lockfile(request: &Request, index: &Index, ruby_directive: bool) -> String {
    let resolved = resolve(request, index).unwrap_or_else(|e| panic!("{e}"));
    Lockfile::from_resolution(
        "https://gems.example",
        request,
        index,
        &resolved,
        ruby_directive,
    )
    .write()
}

/// Nothing has a native build, so `ruby` is added beside the local
/// platform; checksums come from the index, and `bundler` is depended on
/// but never locked.
#[test]
fn a_pure_ruby_lockfile_gets_ruby() {
    let index = index(&[
        (
            "app",
            "---\n1.0.0 bundler:>= 1.15,helper:~> 1.0|checksum:aa\n",
        ),
        ("helper", "---\n1.0.0 |checksum:bb\n1.1.0 |checksum:cc\n"),
    ]);
    let request = request(&["app"], &["x86_64-linux"]);
    assert_eq!(
        new_lockfile(&request, &index, true),
        "GEM
  remote: https://gems.example/
  specs:
    app (1.0.0)
      bundler (>= 1.15)
      helper (~> 1.0)
    helper (1.1.0)

PLATFORMS
  ruby
  x86_64-linux

DEPENDENCIES
  app

CHECKSUMS
  app (1.0.0) sha256=aa
  helper (1.1.0) sha256=cc

RUBY VERSION
  ruby 4.0.9

BUNDLED WITH
  4.0.20
"
    );
}

/// The ruby build needs a gem the platform builds do not, so `ruby` is not
/// complete; each Linux libc and macOS build is, so their platforms come in,
/// and the local `x86_64-linux` goes, the `-gnu` one naming it more
/// specifically. `java` is not generic `ruby`, so it never comes in.
#[test]
fn native_builds_bring_their_platforms() {
    let index = index(&[
        (
            "native",
            "---
1.0.0-x86_64-linux-gnu |checksum:01
1.0.0-x86_64-linux-musl |checksum:02
1.0.0-arm64-darwin |checksum:03
1.0.0-java |checksum:04
1.0.0 builder:>= 0|checksum:05
",
        ),
        ("builder", "---\n1.0.0 |checksum:06\n"),
    ]);
    let request = request(&["native"], &["x86_64-linux"]);
    let written = new_lockfile(&request, &index, false);
    assert_eq!(
        written,
        "GEM
  remote: https://gems.example/
  specs:
    native (1.0.0-arm64-darwin)
    native (1.0.0-x86_64-linux-gnu)
    native (1.0.0-x86_64-linux-musl)

PLATFORMS
  arm64-darwin
  x86_64-linux-gnu
  x86_64-linux-musl

DEPENDENCIES
  native

CHECKSUMS
  native (1.0.0-arm64-darwin) sha256=03
  native (1.0.0-x86_64-linux-gnu) sha256=01
  native (1.0.0-x86_64-linux-musl) sha256=02

BUNDLED WITH
  4.0.20
"
    );
}

/// On a local platform whose generic one is not `ruby` no platform is
/// added (`should_add_extra_platforms?`).
#[test]
fn a_java_lockfile_stays_java() {
    let index = index(&[("json", "---\n3.1.0 |checksum:aa\n3.1.0-java |checksum:bb\n")]);
    let request = request(&["json"], &["java"]);
    let written = new_lockfile(&request, &index, false);
    assert!(written.contains("PLATFORMS\n  java\n\n"), "{written}");
    assert!(
        written.contains("  specs:\n    json (3.1.0-java)\n\n"),
        "{written}"
    );
}

/// What Spinel writes reads back as itself.
#[test]
fn a_new_lockfile_round_trips() {
    let sum = |c: char| c.to_string().repeat(64);
    let index = index(&[
        (
            "app",
            &format!("---\n1.0.0 helper:~> 1.0|checksum:{}\n", sum('a')),
        ),
        (
            "helper",
            &format!(
                "---\n1.0.0 |checksum:{}\n1.0.0-java |checksum:{}\n",
                sum('b'),
                sum('c')
            ),
        ),
    ]);
    let request = request(&["app"], &["x86_64-linux", "java"]);
    let written = new_lockfile(&request, &index, false);
    assert_eq!(round_trip(&written), written);
}
