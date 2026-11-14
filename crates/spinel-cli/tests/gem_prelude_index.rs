//! Where RubyGems puts an activated gem on the `spinel` binary (#39): the
//! CLI marks its own default `$LOAD_PATH` entries, `sitelibdir` onward, with
//! `@gem_prelude_index` as `ruby.c` does, so `Gem.load_path_insert_index`
//! puts a gem after the `-I` entries and an app's own file still wins. The
//! expectation is CRuby 4.0.9's output for the same fixture.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn an_activated_gem_goes_after_the_dash_i_entries() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gem_prelude_index");
    // No installed gems to resolve against: the fixture activates its own.
    let empty = dir.join("no_gems");
    let output = Command::new(SPINEL)
        .args(["-I", "lib", "activate.rb"])
        .current_dir(&dir)
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .env("GEM_HOME", &empty)
        .env("GEM_PATH", &empty)
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "true\n1\ntrue\n\
         true\n\
         true\ntrue\nfalse\ntrue\ntrue\n\
         greet 1.0\n:app\n"
    );
}
