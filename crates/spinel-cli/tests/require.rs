//! `require`'s feature lookup on the `spinel` binary (#39): `$LOADED_FEATURES`
//! is asked before the disk, against the load path as it is now, and holds
//! what CRuby provides at boot. Each expectation is CRuby 4.0.9's output
//! for the same fixture.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn run(fixture: &str) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/require")
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
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "no `already initialized constant` warning: the file ran once"
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_feature_is_not_loaded_again_after_a_directory_is_put_ahead_on_the_load_path() {
    assert_eq!(
        run("double_load.rb"),
        "stdlib copy\ntrue\nfalse\nfalse\n:stdlib\n[\"stdlib/double_load.rb\"]\n"
    );
}

#[test]
fn provided_features_answer_false_and_a_c_extension_is_refused_without_a_path() {
    assert_eq!(
        run("provided.rb"),
        "[\"enumerator.so\", \"thread.rb\", \"fiber.so\", \"rational.so\", \"complex.so\", \
         \"pathname.so\", \"ruby2_keywords.rb\", \"set.rb\"]\n\
         false\nfalse\nfalse\ntrue\ntrue\n[true, nil]\n"
    );
}
