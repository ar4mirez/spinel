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
    assert_eq!(app_script_text("alpha", "alpha"), fixture("bin/alpha"));
    // Bundler's own stub sets BUNDLER_VERSION and skips `Gem.use_gemdeps`;
    // the executable's name is escaped for its single quotes.
    assert_eq!(
        app_script_text("bundler", "it's\\x"),
        fixture("bundler-stub")
    );
}
