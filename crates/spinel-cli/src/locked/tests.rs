//! What `Definition` makes of a lockfile (#79), on small hand-written ones.
//! Whole resolutions are checked against Bundler itself in
//! `tests/locked.rs`; these pin down the decisions Bundler's cases there do
//! not reach.

use super::*;
use crate::gem_platform::Platform;

const REMOTE: &str = "https://gems.example";

const LOCK: &str = "\
GEM
  remote: https://gems.example/
  specs:
    rack (2.2.25)
    sinatra (3.2.0)
      rack (~> 2.2)

PLATFORMS
  ruby
  x86_64-linux

DEPENDENCIES
  sinatra

CHECKSUMS
  rack (2.2.25) sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  sinatra (3.2.0) sha256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb

BUNDLED WITH
  4.0.20
";

/// Rack 2.2.25 and 3.0.0, Sinatra 3.2.0 (Rack 2) and 4.0.0 (Rack 3).
const INDEX: &[(&str, &str)] = &[
    (
        "rack",
        "2.2.25 |checksum:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n3.0.0 |checksum:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n",
    ),
    (
        "sinatra",
        "3.2.0 rack:~> 2.2|checksum:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n4.0.0 rack:>= 3|checksum:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\n",
    ),
];

fn local() -> Platform {
    Platform::parse("x86_64-linux")
}

fn platform(s: &str) -> GemPlatform {
    GemPlatform::new(Some(s), &local())
}

fn lock(text: &str) -> Lockfile {
    Lockfile::parse(text, "Gemfile.lock", &local()).unwrap()
}

fn dep(name: &str, requirement: &[&str]) -> GemfileDependency {
    GemfileDependency {
        name: name.to_owned(),
        requirement: Requirement::new(requirement.iter().copied()).unwrap(),
        platforms: Vec::new(),
        force_ruby_platform: false,
    }
}

fn definition(text: &str, deps: &[GemfileDependency], ruby: Option<&[&str]>) -> Definition {
    let ruby: Option<Vec<String>> = ruby.map(|r| r.iter().map(|s| (*s).to_owned()).collect());
    Definition::unlocking(
        &lock(text),
        REMOTE,
        deps,
        ruby.as_deref(),
        &platform("x86_64-linux"),
        &[],
        &Unlock::Nothing,
    )
    .unwrap()
}

fn request(definition: &Definition, deps: Vec<GemfileDependency>) -> Request {
    Request {
        gemfile: "Gemfile".to_owned(),
        source: format!("rubygems repository {REMOTE}/"),
        dependencies: deps,
        platforms: definition.platforms().to_vec(),
        ruby_version: Version::new("4.0.0").unwrap(),
        rubygems_version: Version::new("4.0.16").unwrap(),
    }
}

fn index() -> Index {
    INDEX
        .iter()
        .map(|(name, info)| {
            (
                (*name).to_owned(),
                resolver::parse_info(name, info.as_bytes(), &local()).unwrap(),
            )
        })
        .collect()
}

fn names(specs: &[Spec]) -> Vec<String> {
    let mut names: Vec<String> = specs.iter().map(Spec::full_name).collect();
    names.sort();
    names
}

#[test]
fn an_unchanged_gemfile_is_the_lockfile() {
    let deps = [dep("sinatra", &[])];
    let def = definition(LOCK, &deps, None);
    assert!(!def.resolve_needed());
    let specs = def.unchanged(&deps);
    assert_eq!(names(&specs), ["rack-2.2.25", "sinatra-3.2.0"]);
    let request = request(&def, deps.to_vec());
    let written = def.lockfile(REMOTE, &request, None, &specs).write();
    assert_eq!(written, LOCK);
}

#[test]
fn a_missing_checksum_resolves_again_and_takes_the_index_one() {
    let text = LOCK.replace(
        "  rack (2.2.25) sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        "",
    );
    let deps = [dep("sinatra", &[])];
    let mut def = definition(&text, &deps, None);
    assert!(def.resolve_needed());
    let request = request(&def, deps.to_vec());
    let index = index();
    let specs = def.start_resolution(&request, &index).unwrap();
    // Sinatra 4 is newer, but the locked versions are tried first.
    assert_eq!(names(&specs), ["rack-2.2.25", "sinatra-3.2.0"]);
    let written = def.lockfile(REMOTE, &request, Some(&index), &specs).write();
    assert_eq!(written, LOCK);
}

#[test]
fn an_empty_checksum_resolves_again() {
    let text = LOCK.replace(
        "  rack (2.2.25) sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        "  rack (2.2.25)\n",
    );
    assert!(definition(&text, &[dep("sinatra", &[])], None).resolve_needed());
}

#[test]
fn no_checksums_section_writes_none() {
    let text = LOCK.replace(
        "CHECKSUMS\n  rack (2.2.25) sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n  sinatra (3.2.0) sha256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n\n",
        "",
    );
    let deps = [dep("sinatra", &[]), dep("rack", &[])];
    let mut def = definition(&text, &deps, None);
    // `rack` is a new dependency: a change.
    assert!(def.resolve_needed());
    let request = request(&def, deps.to_vec());
    let index = index();
    let specs = def.start_resolution(&request, &index).unwrap();
    let lock = def.lockfile(REMOTE, &request, Some(&index), &specs);
    assert_eq!(lock.checksums, None);
}

#[test]
fn the_local_platform_comes_in_only_when_nothing_locked_runs_here() {
    let java_only = LOCK.replace("  ruby\n  x86_64-linux\n", "  java\n");
    let def = definition(&java_only, &[dep("sinatra", &[])], None);
    assert!(def.resolve_needed());
    assert_eq!(
        def.platforms(),
        [platform("java"), platform("x86_64-linux")]
    );
    assert!(def.current_platform_missing);

    // `ruby` runs anywhere: nothing is added, and nothing changed.
    let ruby_only = LOCK.replace("  ruby\n  x86_64-linux\n", "  ruby\n");
    let def = definition(&ruby_only, &[dep("sinatra", &[])], None);
    assert!(!def.resolve_needed());
    assert_eq!(def.platforms(), [GemPlatform::Ruby]);
    assert_eq!(def.most_specific_non_local, Some(GemPlatform::Ruby));
}

#[test]
fn a_locked_ruby_version_is_kept_while_the_directive_allows_it() {
    let text = LOCK.replace(
        "BUNDLED WITH",
        "RUBY VERSION\n  ruby 3.4.1p0\n\nBUNDLED WITH",
    );
    let deps = [dep("sinatra", &[])];

    // Ruby 4.0.0 is resolved for, but the lockfile's Ruby is kept.
    let def = definition(&text, &deps, Some(&[">= 3.0"]));
    assert!(!def.resolve_needed());
    let request = request(&def, deps.to_vec());
    let specs = def.unchanged(&deps);
    let lock = def.lockfile(REMOTE, &request, None, &specs);
    assert_eq!(lock.ruby_version.as_deref(), Some("ruby 3.4.1p0"));
    assert_eq!(lock.write(), text);

    // A directive the locked Ruby does not meet takes the Ruby resolved for.
    let def = definition(&text, &deps, Some(&["~> 4.0"]));
    assert!(def.resolve_needed());
    let lock = def.lockfile(REMOTE, &request, None, &specs);
    assert_eq!(lock.ruby_version.as_deref(), Some("ruby 4.0.0"));

    // So does a directive the lockfile had none for; and none drops it.
    assert!(definition(LOCK, &deps, Some(&[">= 3.0"])).resolve_needed());
    let def = definition(&text, &deps, None);
    assert!(def.resolve_needed());
    assert_eq!(
        def.lockfile(REMOTE, &request, None, &specs).ruby_version,
        None
    );
}

#[test]
fn an_unreadable_locked_ruby_version_is_refused() {
    let text = LOCK.replace("BUNDLED WITH", "RUBY VERSION\n  jruby\n\nBUNDLED WITH");
    let ruby = vec![">= 3.0".to_owned()];
    let err = Definition::unlocking(
        &lock(&text),
        REMOTE,
        &[dep("sinatra", &[])],
        Some(&ruby),
        &platform("x86_64-linux"),
        &[],
        &Unlock::Nothing,
    )
    .unwrap_err();
    assert!(err.0.starts_with("The Ruby version jruby from"), "{err}");
}

#[test]
fn reads_the_version_of_a_locked_ruby() {
    let version = |s: &str| locked_ruby_version(s).map(|v| v.to_string());
    assert_eq!(version("ruby 3.4.1p0").as_deref(), Some("3.4.1"));
    assert_eq!(version("ruby 3.3.0").as_deref(), Some("3.3.0"));
    assert_eq!(
        version("ruby 3.4.0.preview1p-1").as_deref(),
        Some("3.4.0.preview1")
    );
    assert_eq!(
        version("ruby 3.2.2p53 (jruby 9.4.5.0)").as_deref(),
        Some("3.2.2")
    );
    assert_eq!(version("jruby"), None);
}

#[test]
fn a_spec_whose_dependencies_are_not_locked_is_dropped() {
    let text = LOCK.replace("      rack (~> 2.2)\n", "      rack (~> 3.0)\n");
    let deps = [dep("sinatra", &[])];
    let def = definition(&text, &deps, None);
    assert!(def.resolve_needed());
    assert_eq!(names(&def.unchanged(&deps)), ["rack-2.2.25"]);
    let base = def.resolution_base(&deps);
    assert!(base.base.is_empty(), "{:?}", base.base);
    assert!(!base.requirements.contains_key("sinatra"));
}

#[test]
fn a_changed_source_holds_nothing_but_the_preference() {
    let def = Definition::unlocking(
        &lock(LOCK),
        "https://elsewhere.example",
        &[dep("sinatra", &[])],
        None,
        &platform("x86_64-linux"),
        &[],
        &Unlock::Nothing,
    )
    .unwrap();
    assert!(def.resolve_needed());
    let base = def.resolution_base(&[dep("sinatra", &[])]);
    assert!(base.base.is_empty());
    assert!(base.requirements.is_empty());
    assert_eq!(base.versions["sinatra"].to_string(), "3.2.0");
}

#[test]
fn unchanged_gems_get_floors_and_a_newly_named_one_a_pin() {
    let deps = [dep("sinatra", &[]), dep("rack", &[])];
    let def = definition(LOCK, &deps, None);
    assert!(def.resolve_needed());
    assert_eq!(def.changed_dependencies, ["rack"]);
    assert!(def.gems_to_unlock.is_empty());
    let base = def.resolution_base(&deps);
    assert_eq!(names(&base.base), ["rack-2.2.25", "sinatra-3.2.0"]);
    assert_eq!(base.requirements["sinatra"].to_string(), ">= 3.2.0");
    assert_eq!(base.requirements["rack"].to_string(), "= 2.2.25");
}

#[test]
fn a_requirement_the_locked_version_misses_unlocks_that_gem() {
    let deps = [dep("sinatra", &["~> 4.0"])];
    let mut def = definition(LOCK, &deps, None);
    assert!(def.resolve_needed());
    assert_eq!(def.gems_to_unlock, ["sinatra"]);
    let base = def.resolution_base(&deps);
    assert!(base.base.is_empty());
    assert!(!base.requirements.contains_key("sinatra"));
    assert_eq!(base.requirements["rack"].to_string(), ">= 2.2.25");
    assert!(base.unlock.contains("sinatra"));

    let request = request(&def, deps.to_vec());
    let specs = def.start_resolution(&request, &index()).unwrap();
    assert_eq!(names(&specs), ["rack-3.0.0", "sinatra-4.0.0"]);
}

#[test]
fn a_loosened_requirement_the_locked_version_meets_changes_nothing() {
    let deps = [dep("sinatra", &[">= 3.0"])];
    let def = definition(LOCK, &deps, None);
    assert!(!def.resolve_needed());
    assert!(def.changed_dependencies.is_empty());
    let request = request(&def, deps.to_vec());
    let specs = def.unchanged(&deps);
    let written = def.lockfile(REMOTE, &request, None, &specs).write();
    assert_eq!(
        written,
        LOCK.replace("  sinatra\n\nCHECKSUMS", "  sinatra (>= 3.0)\n\nCHECKSUMS")
    );
}

#[test]
fn a_gem_taken_out_takes_what_only_it_needed() {
    let text = LOCK.replace(
        "DEPENDENCIES\n  sinatra\n",
        "DEPENDENCIES\n  rack\n  sinatra\n",
    );
    let deps = [dep("rack", &[])];
    let def = definition(&text, &deps, None);
    assert!(!def.resolve_needed());
    assert_eq!(names(&def.unchanged(&deps)), ["rack-2.2.25"]);
}

/// `LOCK` as Bundler 4.0.10 wrote it: its `BUNDLED WITH`, and a checksum
/// line for itself.
fn lock_from_4_0_10() -> String {
    LOCK.replace(
        "CHECKSUMS\n",
        "CHECKSUMS\n  bundler (4.0.10) sha256=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\n",
    )
    .replace("BUNDLED WITH\n  4.0.20\n", "BUNDLED WITH\n  4.0.10\n")
}

fn write_lock(text: &str, deps: &[GemfileDependency]) -> String {
    let def = definition(text, deps, None);
    let request = request(&def, deps.to_vec());
    let specs = def.unchanged(deps);
    def.write_lock(text, REMOTE, &request, None, &specs)
}

#[test]
fn a_lockfile_from_an_earlier_bundler_4_is_left_as_it_was() {
    let text = lock_from_4_0_10();
    assert_eq!(write_lock(&text, &[dep("sinatra", &[])]), text);
}

#[test]
fn a_lockfile_with_no_bundled_with_or_an_unknown_section_is_left_as_it_was() {
    let text = LOCK.replace("\nBUNDLED WITH\n  4.0.20\n", "\nSOMETHING NEW\n  a value\n");
    assert_eq!(write_lock(&text, &[dep("sinatra", &[])]), text);
}

#[test]
fn a_lockfile_from_an_earlier_bundler_that_changes_is_written_for_this_one() {
    let written = write_lock(&lock_from_4_0_10(), &[dep("sinatra", &[">= 3.0"])]);
    assert_eq!(
        written,
        LOCK.replace("  sinatra\n\nCHECKSUMS", "  sinatra (>= 3.0)\n\nCHECKSUMS")
    );
}

#[test]
fn a_lockfile_from_an_earlier_major_bundler_is_written_for_this_one() {
    let text = lock_from_4_0_10().replace("4.0.10", "2.6.9");
    assert_eq!(write_lock(&text, &[dep("sinatra", &[])]), LOCK);
}

fn updating(text: &str, deps: &[GemfileDependency], unlock: Unlock) -> Definition {
    Definition::unlocking(
        &lock(text),
        REMOTE,
        deps,
        None,
        &platform("x86_64-linux"),
        &[],
        &unlock,
    )
    .unwrap()
}

/// `bundle update sinatra`: Sinatra and the Rack it locks are unlocked
/// (`@locked_specs.for(eager_unlock)`), the base holds neither, and Sinatra
/// is pinned to what a full update gives it, so both move.
#[test]
fn updating_a_gem_unlocks_it_and_what_it_reaches() {
    let deps = [dep("sinatra", &[])];
    let mut def = updating(LOCK, &deps, Unlock::Gems(vec!["sinatra".to_owned()]));
    assert!(def.resolve_needed());
    assert_eq!(def.gems_to_unlock, ["sinatra", "rack"]);
    let base = def.resolution_base(&deps);
    assert!(base.base.is_empty(), "{:?}", base.base);
    assert_eq!(base.requirements["rack"].to_string(), ">= 2.2.25");
    let request = request(&def, deps.to_vec());
    let index = index();
    let specs = def.start_resolution(&request, &index).unwrap();
    assert_eq!(names(&specs), ["rack-3.0.0", "sinatra-4.0.0"]);
    let written = def.write_lock(LOCK, REMOTE, &request, Some(&index), &specs);
    assert!(written.contains("    sinatra (4.0.0)\n"), "{written}");
}

/// `bundle update rack`: Rack alone is unlocked and Sinatra stays in the
/// base at 3.2.0, but Rack is pinned to the 3.0.0 a full update gives it.
/// The conflict loosens Rack's pin (a gem the Gemfile does not name), then
/// Sinatra's, and both move, as Bundler 4.0.20 moves them for this index.
#[test]
fn updating_a_dependency_forces_it_up_past_what_holds_it() {
    let deps = [dep("sinatra", &[])];
    let mut def = updating(LOCK, &deps, Unlock::Gems(vec!["rack".to_owned()]));
    assert_eq!(def.gems_to_unlock, ["rack"]);
    let base = def.resolution_base(&deps);
    assert_eq!(names(&base.base), ["sinatra-3.2.0"]);
    let request = request(&def, deps.to_vec());
    let specs = def.start_resolution(&request, &index()).unwrap();
    assert_eq!(names(&specs), ["rack-3.0.0", "sinatra-4.0.0"]);
}

/// `bundle update`: no base, every gem unlocked, the floors kept.
#[test]
fn updating_everything_holds_only_the_floors() {
    let deps = [dep("sinatra", &[])];
    let mut def = updating(LOCK, &deps, Unlock::All);
    assert!(def.resolve_needed());
    let base = def.resolution_base(&deps);
    assert!(base.unlock_all);
    assert!(base.base.is_empty());
    assert_eq!(base.requirements["sinatra"].to_string(), ">= 3.2.0");
    assert_eq!(base.requirements["rack"].to_string(), ">= 2.2.25");
    let request = request(&def, deps.to_vec());
    let index = index();
    let specs = def.start_resolution(&request, &index).unwrap();
    assert_eq!(names(&specs), ["rack-3.0.0", "sinatra-4.0.0"]);
    // Nothing moving still writes the lockfile Bundler writes, not the one
    // on disk: `write_lock` keeps nothing when unlocking.
    let def = updating(LOCK, &deps, Unlock::All);
    let request = self::request(&def, deps.to_vec());
    let old = LOCK.replace("BUNDLED WITH\n  4.0.20\n", "BUNDLED WITH\n  4.0.10\n");
    let specs = def.unchanged(&deps);
    assert_eq!(def.write_lock(&old, REMOTE, &request, None, &specs), LOCK);
}
