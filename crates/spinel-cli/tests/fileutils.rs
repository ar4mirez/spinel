//! The vendored `fileutils` (#49): making, copying, moving, linking,
//! chmod, install, touch and removing trees, the verbose and noop modes,
//! and the option tables. ruby/spec has no `library/fileutils`, so this
//! is the check. Expected output is what ruby 4.0.9 prints for the same
//! script.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

const EXPECTED: &str = r#"mkdir_p: ["a/b/c"]
mkdir: ["d"]
touch: ["a/x.txt", "a/b/y.txt"]
exists: [true, true]
write: 5
cp: nil
read cp: "hello"
compare: true
identical?: true
uptodate?: true
cp_r: nil
cp_r tree: ["e/b", "e/b/c", "e/b/y.txt", "e/x.txt"]
mv: 0
mv ls: ["x3.txt"]
ln: 0
ln_s: 0
ln_sf: 0
readlink: "../a/x.txt"
chmod: 384
chmod sym: 480
chmod_R: 493
install: 420
copy_entry: ["f/b", "f/b/c", "f/b/y.txt", "f/x.txt"]
pwd: true
cd: true
rm: ["d/x3.txt"]
rm_f: ["nope"]
rmdir: ["a/b/c"]
rm_r: ["e"]
rm_rf: ["f"]
remove_entry_secure: nil
mkdir -p v
verbose: ["v"]
rm -rf a
noop: true
noop opt: true
touch v/z
Verbose module: ["v/z"]
options: true
options_of: ["force", "noop", "verbose"]
have_option?: true
cp preserve: true
rm missing: Errno::ENOENT: No such file or directory @ apply2files - nope
mkdir exists: Errno::EEXIST: File exists @ dir_s_mkdir - v
ln_sr: "s/t/f"
cp_lr: 2
mkdir_p mode: 448
ln_s dir: 0
rm_r symlink dir: [false, true]
cp_r deref: true
copy_file: "22"
copy_stream: "22"
cp to self: ArgumentError: same file: s/g and s/g
mv dir into dir: ["m"]
mv list: ["m", "p1", "p2"]
rmdir parents: false
rmdir nonempty: Errno::ENOTEMPTY: Directory not empty @ dir_s_rmdir - s
touch mtime: 1000000000
chown nil: ["tm"]
chown_R nil: ["s"]
remove_dir: false
remove_file: false
remove_entry: 0
compare_stream: true
install dir: 420
collect_method: ["chmod", "chmod_R", "chown"]
commands: true
private: false
include: ["inc"]
NoWrite: false
ln force: "22"
rm_rf glob: []
mkdir_p pathname: ["pn/x"]
final: ["a", "a/b", "a/b/y.txt", "a/x.txt", "cs", "dest", "dest/g", "dest/m", "dest/m/n", "dest/p1", "dest/p2", "inc", "lr", "pn", "pn/x", "s", "s/g", "s/lf", "s/t", "s/t/f", "tm", "v", "v/p", "v/z"]
"#;

#[test]
fn fileutils_works_on_a_scratch_tree() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = std::env::temp_dir().join(format!(
        "spinel-fileutils-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("the scratch directory is made");

    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fileutils/fileutils.rb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&root)
        .output()
        .expect("spinel runs");
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}
