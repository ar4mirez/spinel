//! Pathname's directory listing (#326): `children`, `each_child`,
//! `each_entry`, `opendir`, and `require "pathname"`'s `find` and `rmtree`,
//! which stand on `Dir.foreach`, `Dir.open` and `Dir.children(encoding:)`.
//! Expected output is what ruby 4.0.7 prints for the same script.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

const EXPECTED: &str = r#"["a.txt", "sub"]
["a.txt", "sub"]
["a.txt", "sub"]
[".", "..", "a.txt", "sub"]
[".", "a.txt", "sub", "sub/.b"]
[Dir, true, ["a.txt", "sub"]]
String
IOError
[".", "..", "a.txt", "sub"]
[#<Encoding:BINARY (ASCII-8BIT)>]
true
false
"#;

#[test]
fn pathname_lists_finds_and_removes_a_tree() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tree = std::env::temp_dir().join(format!(
        "spinel-pathname-dir-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(tree.join("sub")).expect("the tree is made");
    std::fs::write(tree.join("a.txt"), "a").expect("a.txt is written");
    std::fs::write(tree.join("sub/.b"), "b").expect("sub/.b is written");

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pathname/dir_listing.rb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&tree)
        .output()
        .expect("spinel runs");
    let removed = !tree.exists();
    let _ = std::fs::remove_dir_all(&tree);

    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
    assert!(removed, "rmtree left the tree on disk");
}
