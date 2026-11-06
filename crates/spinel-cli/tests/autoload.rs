//! `autoload` edges ruby/spec has no example for, on the `spinel` binary
//! (#709). Each expectation is CRuby 4.0.9's output for the same fixture.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn run(fixture: &str) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/autoload")
        .join(fixture);
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_qualified_const_get_does_not_reach_an_autoload_on_object() {
    assert_eq!(
        run("qualified_object.rb"),
        "NameError: uninitialized constant Baz::Bar\n"
    );
}

#[test]
fn a_throw_out_of_an_autoloaded_file_leaves_nothing_marked_loading() {
    assert_eq!(run("throw_out.rb"), ":thrown\n");
}
