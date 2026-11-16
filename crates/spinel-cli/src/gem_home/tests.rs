use std::path::Path;

use super::*;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gem_home")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn bin_stubs_are_the_ones_rubygems_writes() {
    assert_eq!(
        app_script_text("spinel", "alpha", "alpha"),
        fixture("bin/alpha")
    );
    // Bundler's own stub sets BUNDLER_VERSION and skips `Gem.use_gemdeps`;
    // the executable's name is escaped for its single quotes.
    assert_eq!(
        app_script_text("spinel", "bundler", "it's\\x"),
        fixture("bundler-stub")
    );
}

/// A store entry for `name` `version` whose gem ships `exe/shared`.
fn entry(store_gems: &Path, name: &str, version: &str) -> (GemId, std::path::PathBuf) {
    let entry = store_gems.join(format!("{name}-{version}"));
    std::fs::create_dir_all(entry.join("gem/exe")).unwrap();
    std::fs::write(entry.join("gem/exe/shared"), "puts 1\n").unwrap();
    std::fs::write(
        entry.join("metadata"),
        format!(
            "--- !ruby/object:Gem::Specification\nname: {name}\nversion: !ruby/object:Gem::Version\n  \
             version: {version}\nsummary: s\nspecification_version: 4\nbindir: exe\n\
             executables:\n- shared\n"
        ),
    )
    .unwrap();
    (GemId::new(name, version, "ruby").unwrap(), entry)
}

#[test]
fn an_executable_two_gems_share_is_written_once_for_the_later() {
    let dir = std::env::temp_dir().join(format!("spinel-gem-home-unit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store_gems = dir.join("store/gems");
    let home = dir.join("home");
    let gems = [
        entry(&store_gems, "one", "1.0"),
        entry(&store_gems, "two", "1.0"),
    ];
    let first = link(&home, &store_gems, &gems).unwrap();
    assert_eq!(
        first.warnings,
        ["bin/shared: two-1.0 and one-1.0 both have it; two-1.0 wins"]
    );
    assert_eq!(
        std::fs::read_to_string(home.join("bin/shared")).unwrap(),
        app_script_text("spinel", "two", "shared")
    );
    let again = link(&home, &store_gems, &gems).unwrap();
    let outcomes: Vec<String> = again
        .linked
        .iter()
        .map(|(gem, outcome)| format!("{gem}: {outcome}"))
        .collect();
    assert_eq!(outcomes, ["one-1.0: current", "two-1.0: current"]);
    assert_eq!(
        std::fs::read_to_string(home.join("bin/shared")).unwrap(),
        app_script_text("spinel", "two", "shared")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scope_view_links_each_subdirectory_and_keeps_what_is_not_spinels() {
    let dir = std::env::temp_dir().join(format!("spinel-gem-home-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let home = dir.join("gem_home");
    let store_gems = dir.join("store/gems");
    link(&home, &store_gems, &[entry(&store_gems, "one", "1.0")]).unwrap();
    // CRuby's Bundler, having installed into the scope itself, left a real
    // directory where one link would go.
    std::fs::create_dir_all(home.join("ruby/4.0.0/doc")).unwrap();
    // An earlier `spinel install` linked `bin` too.
    std::os::unix::fs::symlink("../../bin", home.join("ruby/4.0.0/bin")).unwrap();
    let warnings = scope_view(&home, "ruby/4.0.0").unwrap();
    assert_eq!(
        warnings,
        [format!(
            "{}: already there, and not Spinel's; Bundler reading ruby/4.0.0 will not see the gems Spinel installs",
            home.join("ruby/4.0.0/doc").display()
        )]
    );
    for sub in SUBDIRECTORIES {
        let link = home.join("ruby/4.0.0").join(sub);
        if sub == "doc" || sub == "bin" {
            assert!(!link.is_symlink());
            continue;
        }
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            Path::new("../..").join(sub)
        );
    }
    assert_eq!(
        std::fs::read_to_string(home.join("ruby/4.0.0/specifications/one-1.0.gemspec")).unwrap(),
        std::fs::read_to_string(home.join("specifications/one-1.0.gemspec")).unwrap()
    );
    // CRuby's view has CRuby's stubs; Spinel's view, Spinel's.
    assert_eq!(
        std::fs::read_to_string(home.join("ruby/4.0.0/bin/shared")).unwrap(),
        app_script_text("ruby", "one", "shared")
    );
    scope_view(&home, "spinel/4.0.0").unwrap();
    assert_eq!(
        std::fs::read_link(home.join("spinel/4.0.0/bin")).unwrap(),
        Path::new("../../bin")
    );
    // A stub of CRuby's own is kept; one for a gem no longer there goes.
    std::fs::write(home.join("ruby/4.0.0/bin/theirs"), "#!/bin/sh\n").unwrap();
    // As is the stub CRuby's `bundle install` wrote for a gem it installed
    // into the shared GEM_HOME and Spinel did not link.
    std::fs::write(home.join("specifications/foo-2.0.gemspec"), "").unwrap();
    std::fs::write(
        home.join("ruby/4.0.0/bin/foo"),
        app_script_text("ruby", "foo", "foo"),
    )
    .unwrap();
    link(&home, &store_gems, &[]).unwrap();
    scope_view(&home, "ruby/4.0.0").unwrap();
    assert!(!home.join("ruby/4.0.0/bin/shared").exists());
    assert!(home.join("ruby/4.0.0/bin/theirs").exists());
    assert!(home.join("ruby/4.0.0/bin/foo").exists());
    // A symlink leading elsewhere is put right; one already right stays.
    std::fs::remove_file(home.join("ruby/4.0.0/gems")).unwrap();
    std::os::unix::fs::symlink("/nowhere", home.join("ruby/4.0.0/gems")).unwrap();
    scope_view(&home, "ruby/4.0.0").unwrap();
    assert_eq!(
        std::fs::read_link(home.join("ruby/4.0.0/gems")).unwrap(),
        Path::new("../../gems")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
