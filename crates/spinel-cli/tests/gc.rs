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
        "before: 4 size=4\nafter: 1 size=1\ntrue\nObject\nObject\ntrue\nat_exit\nfinalizer at exit\n"
    );
}

/// A String `String#-@` interned is collected once nothing refers to it
/// (#384): the fstring table holds it weakly, and holds one a frozen literal
/// answered strongly, as CRuby's iseq does.
#[test]
fn deduplicated_strings_nothing_references_are_collected() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gc/dedup.rb");
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
        "true\nbefore: 3\nafter: 2\ntrue\n[\"kept\", \"lit\"]\n"
    );
}

/// An IO the collector frees is finalized (#460): its `xml: :attr` quote is
/// closed and its descriptor with it, unless it is not to `autoclose` it,
/// or it was closed already and the number is another file's by then.
#[test]
fn an_io_the_collector_frees_is_finalized() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gc/io_finalize.rb");
    let dir = std::env::temp_dir().join(format!("spinel-io-finalize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .output()
        .expect("spinel runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "\"\\\"&lt;a\\\"\"\n\"\\\"\\x00a\\x00\\\"\\x00\"\n\"\\\"a\"\n[\"kept\", \"reused\"]\n\"ok\"\n"
    );
}
