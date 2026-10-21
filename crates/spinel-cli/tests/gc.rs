//! The collector as Ruby sees it (#36), on the `spinel` binary: a weak map
//! lets go of what nothing else holds once `GC.start` runs, and a finalizer
//! runs after its object is collected, handed the id rather than the
//! object, and at exit for one that never was.
//!
//! ruby/spec cannot say any of this — `define_finalizer_spec.rb` explains
//! why: CRuby's collector is conservative, so whether an object dies is not
//! something a spec can promise. Spinel's is precise, so here it can.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

#[test]
fn weak_references_clear_and_finalizers_run_after_collection() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gc/weak_and_finalizers.rb");
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
        String::from_utf8_lossy(&output.stdout),
        "before: 4 size=4\nafter: 1 size=1\ntrue\nObject\ntrue\nat_exit\nfinalizer at exit\n"
    );
}
