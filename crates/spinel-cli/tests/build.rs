//! `spinel build --compile` (#100, docs/build.md): the payload a compiled app
//! carries is `manifest.json`, `app/`, `gem_home/` and `bytecode/`, and
//! nothing else — every symlink a real copy, only the gems of the lockfile's
//! `default` group and the groups asked for, and none of the standard library
//! or core image the `spinel` binary already holds.
//!
//! Each test copies `fixtures/build` into a directory of its own, installs
//! its path gems offline with `spinel install`, builds, and reads the payload
//! back out of the app through its trailer: from the end of the file on
//! Linux, from the `__SPINEL,__payload` section on macOS.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// A fresh directory for one test, under Cargo's scratch directory, removed
/// again when it drops: each app is a copy of the whole `spinel`.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("build")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/build"),
            &root,
        );
        Sandbox(fs::canonicalize(&root).expect("the copy exists"))
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    fn spinel(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(SPINEL);
        command
            .current_dir(dir)
            .env("SPINEL_HOME", self.0.join("home"))
            .args(args);
        for var in [
            "GEM_HOME",
            "GEM_PATH",
            "BUNDLE_GEMFILE",
            "BUNDLE_PATH",
            "BUNDLE_APP_CONFIG",
            "BUNDLE_FROZEN",
            "RUBYOPT",
            "RUBYLIB",
        ] {
            command.env_remove(var);
        }
        command.output().expect("spinel runs")
    }

    /// `spinel install`, then the files and symlinks the fixture cannot keep
    /// in git: an ignored directory and file, and symlinks to a file and to
    /// a directory outside the project.
    fn installed(name: &str) -> Sandbox {
        let sandbox = Sandbox::new(name);
        let project = sandbox.project();
        ok(&sandbox.spinel(&project, &["install"]), "spinel install");
        fs::create_dir_all(project.join("tmp")).unwrap();
        fs::write(project.join("tmp/keep.txt"), "kept by --include\n").unwrap();
        fs::write(project.join("tmp/scratch.txt"), "left out\n").unwrap();
        fs::write(project.join("debug.log"), "left out\n").unwrap();
        fs::create_dir_all(sandbox.0.join("shared/vendor")).unwrap();
        fs::write(
            sandbox.0.join("shared/linked.rb"),
            "LINKED = \"a file reached through a symlink\"\n",
        )
        .unwrap();
        fs::write(
            sandbox.0.join("shared/vendor/util.rb"),
            "UTIL = \"a directory reached through a symlink\"\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            sandbox.0.join("shared/linked.rb"),
            project.join("lib/linked.rb"),
        )
        .unwrap();
        std::os::unix::fs::symlink(sandbox.0.join("shared/vendor"), project.join("vendor"))
            .unwrap();
        sandbox
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("a directory is made");
    for entry in fs::read_dir(from).expect("the fixture reads") {
        let entry = entry.expect("a directory entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a file copies");
        }
    }
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed: {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn failed(out: &Output, what: &str) -> String {
    assert_eq!(
        out.status.code(),
        Some(1),
        "{what} should fail\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The payload bytes of the app at `path`, found by its trailer, read
/// without loading the whole binary.
fn payload(path: &Path) -> Vec<u8> {
    let mut file = File::open(path).expect("the app opens");
    let (start, len) = if cfg!(target_os = "macos") {
        let mut head = vec![0; 64 * 1024];
        file.read_exact(&mut head).expect("the load commands read");
        payload_section(&head)
    } else {
        (0, file.metadata().expect("stat").len())
    };
    let mut trailer = [0; 48];
    file.seek(SeekFrom::Start(start + len - 48)).unwrap();
    file.read_exact(&mut trailer).unwrap();
    assert_eq!(&trailer[40..], b"SPINEL01", "the trailer's magic");
    let payload_len = u64::from_le_bytes(trailer[32..40].try_into().unwrap());
    let mut bytes = vec![0; usize::try_from(payload_len).unwrap()];
    file.seek(SeekFrom::Start(start + len - 48 - payload_len))
        .unwrap();
    file.read_exact(&mut bytes).unwrap();
    assert_eq!(
        Sha256::digest(&bytes)[..],
        trailer[..32],
        "the trailer's sha256"
    );
    bytes
}

/// The file offset and size of `__SPINEL,__payload`, from the load commands.
fn payload_section(head: &[u8]) -> (u64, u64) {
    let u32_at = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().unwrap());
    assert_eq!(u32_at(0), 0xfeed_facf, "a thin 64-bit Mach-O");
    let mut at = 32;
    for _ in 0..u32_at(16) {
        let (cmd, size) = (u32_at(at), u32_at(at + 4) as usize);
        if cmd == 0x19 && head[at + 8..at + 24].starts_with(b"__SPINEL\0") {
            let section = at + 72;
            return (u64::from(u32_at(section + 48)), u64_at(section + 40));
        }
        at += size;
    }
    panic!("no __SPINEL segment");
}

/// One entry of the payload's archive.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir,
    File { mode: u32, bytes: Vec<u8> },
}

/// The payload's archive, by path, every entry checked to be a directory or
/// a regular file, never a link, with its mtime 0 and owner root.
fn unpack(payload: &[u8]) -> BTreeMap<String, Entry> {
    let tar = zstd::decode_all(payload).expect("the payload is zstd");
    let mut archive = tar::Archive::new(&tar[..]);
    let mut entries = BTreeMap::new();
    for entry in archive.entries().expect("a tar") {
        let mut entry = entry.expect("an entry");
        let path = entry.path().expect("a path").to_string_lossy().into_owned();
        let header = entry.header();
        assert_eq!(header.mtime().unwrap(), 0, "{path}: mtime");
        assert_eq!(
            (header.uid().unwrap(), header.gid().unwrap()),
            (0, 0),
            "{path}: owner"
        );
        let kind = header.entry_type();
        let found = if kind.is_dir() {
            Entry::Dir
        } else {
            assert!(kind.is_file(), "{path}: a {kind:?}, not a regular file");
            let mode = header.mode().unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            Entry::File { mode, bytes }
        };
        let path = path.trim_end_matches('/').to_owned();
        assert!(
            entries.insert(path.clone(), found).is_none(),
            "{path} twice"
        );
    }
    entries
}

/// The paths under `prefix/`, without it: files only.
fn files_under<'a>(entries: &'a BTreeMap<String, Entry>, prefix: &str) -> BTreeSet<&'a str> {
    entries
        .iter()
        .filter(|(_, entry)| matches!(entry, Entry::File { .. }))
        .filter_map(|(path, _)| path.strip_prefix(prefix)?.strip_prefix('/'))
        .collect()
}

fn bytes<'a>(entries: &'a BTreeMap<String, Entry>, path: &str) -> &'a [u8] {
    match entries.get(path) {
        Some(Entry::File { bytes, .. }) => bytes,
        other => panic!("{path}: {other:?}"),
    }
}

/// The entries at the top of the payload.
fn top_level(entries: &BTreeMap<String, Entry>) -> BTreeSet<&str> {
    entries
        .keys()
        .map(|path| path.split('/').next().unwrap())
        .collect()
}

/// The cache file names the payload's `.rb` files compile to: one per
/// distinct source, named by its sha256, as `.spinel/bytecode/` names them.
fn expected_bytecode(entries: &BTreeMap<String, Entry>) -> BTreeSet<String> {
    entries
        .iter()
        .filter(|(path, _)| path.ends_with(".rb"))
        .map(|(path, _)| {
            let digest = Sha256::digest(bytes(entries, path));
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("{hex}.sbc")
        })
        .collect()
}

#[test]
fn the_payload_holds_the_documented_directories_and_nothing_else() {
    let sandbox = Sandbox::installed("payload");
    let project = sandbox.project();
    let args = [
        "build",
        "--compile",
        "bin/mytool",
        "-o",
        "out/mytool",
        "--include",
        "tmp/keep.txt",
        "--exclude",
        "lib/skip.rb",
        "--env",
        "APP_ENV=production",
        "--",
        "--verbose",
        "serve",
    ];
    let printed = ok(&sandbox.spinel(&project, &args), "spinel build");
    let app = project.join("out/mytool");
    let payload = payload(&app);
    assert_eq!(
        printed,
        format!(
            "out/mytool: {} byte payload, sha256 {}\n",
            payload.len(),
            Sha256::digest(&payload)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    );
    let entries = unpack(&payload);

    // Exactly the documented directories, and no `ext/` until phase 5.
    assert_eq!(
        top_level(&entries),
        BTreeSet::from(["app", "bytecode", "gem_home", "manifest.json"])
    );
    for dir in ["app", "bytecode", "gem_home"] {
        assert_eq!(entries.get(dir), Some(&Entry::Dir), "{dir}/");
    }

    // The manifest names the entry point relative to `app/`.
    let manifest: serde_json::Value =
        serde_json::from_slice(bytes(&entries, "manifest.json")).expect("JSON");
    assert_eq!(
        manifest,
        serde_json::json!({
            "entry": "bin/mytool",
            "spinel_version": env!("CARGO_PKG_VERSION"),
            "spinel_ext_abi": 0,
            "argv": ["--verbose", "serve"],
            "env": {"APP_ENV": "production"},
        })
    );

    // What `.gitignore` keeps, `tmp/keep.txt` taken back by `--include`,
    // `lib/skip.rb` dropped by `--exclude`, and neither `.spinel/` nor the
    // app being written. Symlinks are copies of what they lead to.
    assert_eq!(
        files_under(&entries, "app"),
        BTreeSet::from([
            ".bundle/config",
            ".gitignore",
            "Gemfile",
            "Gemfile.lock",
            "README.md",
            "bin/mytool",
            "lib/greet.rb",
            "lib/linked.rb",
            "tmp/keep.txt",
            "vendor/util.rb",
        ])
    );
    assert_eq!(
        bytes(&entries, "app/lib/linked.rb"),
        b"LINKED = \"a file reached through a symlink\"\n"
    );
    assert_eq!(
        bytes(&entries, "app/vendor/util.rb"),
        b"UTIL = \"a directory reached through a symlink\"\n"
    );
    assert!(
        matches!(
            entries.get("app/bin/mytool"),
            Some(Entry::File { mode: 0o755, .. })
        ),
        "the entry point keeps its executable bit"
    );
    assert!(matches!(
        entries.get("app/lib/greet.rb"),
        Some(Entry::File { mode: 0o644, .. })
    ));

    // `alpha` is in the default group and `gamma` is its dependency; `beta`
    // is only in `development`. `gems/` held symlinks to the path gems'
    // directories: the payload holds their files.
    assert_eq!(
        files_under(&entries, "gem_home"),
        BTreeSet::from([
            "bin/alpha",
            "gems/alpha-1.0.0/alpha.gemspec",
            "gems/alpha-1.0.0/exe/alpha",
            "gems/alpha-1.0.0/lib/alpha.rb",
            "gems/gamma-1.0.0/gamma.gemspec",
            "gems/gamma-1.0.0/lib/gamma.rb",
            "specifications/alpha-1.0.0.gemspec",
            "specifications/gamma-1.0.0.gemspec",
        ])
    );
    assert_eq!(
        bytes(&entries, "gem_home/gems/alpha-1.0.0/lib/alpha.rb"),
        fs::read(sandbox.0.join("gems/alpha/lib/alpha.rb")).unwrap()
    );
    for dir in [
        "bin",
        "build_info",
        "cache",
        "doc",
        "extensions",
        "gems",
        "plugins",
        "specifications",
    ] {
        assert_eq!(
            entries.get(&format!("gem_home/{dir}")),
            Some(&Entry::Dir),
            "gem_home/{dir}/"
        );
    }

    // A cache entry for each `.rb` file the payload holds, and none other:
    // nothing of the standard library or the core image.
    let cached: BTreeSet<String> = files_under(&entries, "bytecode")
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(cached, expected_bytecode(&entries));
    assert_eq!(cached.len(), 5);

    // The app is this `spinel` with the payload, and runs its entry point
    // (#101; `tests/app.rs` has the rest).
    let ran = Command::new(&app)
        .env("XDG_CACHE_HOME", sandbox.0.join("cache"))
        .env("SPINEL_HOME", sandbox.0.join("home"))
        .env_remove("GEM_HOME")
        .env_remove("GEM_PATH")
        .output()
        .expect("the app runs");
    assert_eq!(ok(&ran, "the app"), "hello from alpha\n");

    // The same project builds the same payload, byte for byte.
    ok(&sandbox.spinel(&project, &args), "spinel build again");
    assert!(
        self::payload(&app) == payload,
        "the payload is deterministic"
    );
    // Nothing is left beside the app but the app.
    let left: Vec<_> = fs::read_dir(project.join("out"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, ["mytool"]);
}

#[test]
fn a_requested_group_brings_its_gems() {
    let sandbox = Sandbox::installed("group");
    let project = sandbox.project();
    ok(
        &sandbox.spinel(
            &project,
            &[
                "build",
                "--compile",
                "bin/mytool",
                "-o",
                "../mytool",
                "--group",
                "development",
            ],
        ),
        "spinel build --group development",
    );
    let entries = unpack(&payload(&sandbox.0.join("mytool")));
    let gems: BTreeSet<&str> = files_under(&entries, "gem_home")
        .into_iter()
        .filter(|path| path.starts_with("specifications/") || path.starts_with("bin/"))
        .collect();
    assert_eq!(
        gems,
        BTreeSet::from([
            "bin/alpha",
            "bin/beta",
            "specifications/alpha-1.0.0.gemspec",
            "specifications/beta-1.0.0.gemspec",
            "specifications/gamma-1.0.0.gemspec",
        ])
    );
    // Without `--include`, the ignored files stay out.
    let app = files_under(&entries, "app");
    assert!(
        !app.contains("tmp/keep.txt") && !app.contains("debug.log"),
        "{app:?}"
    );
}

#[test]
fn a_program_with_no_gemfile_has_an_empty_gem_home() {
    let sandbox = Sandbox::new("plain");
    let dir = sandbox.0.join("plain");
    ok(
        &sandbox.spinel(&dir, &["build", "--compile", "hello.rb", "-o", "hello"]),
        "spinel build",
    );
    let entries = unpack(&payload(&dir.join("hello")));
    let paths: Vec<&str> = entries.keys().map(String::as_str).collect();
    let hello = "hello.rb";
    let digest: String = Sha256::digest(fs::read(dir.join(hello)).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        paths,
        [
            "app",
            "app/hello.rb",
            "bytecode",
            &format!("bytecode/{digest}.sbc"),
            "gem_home",
            "manifest.json",
        ]
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(bytes(&entries, "manifest.json")).unwrap();
    assert_eq!(manifest["entry"], "hello.rb");
    assert_eq!(manifest["argv"], serde_json::json!([]));
    assert_eq!(manifest["env"], serde_json::json!({}));
}

#[test]
fn what_cannot_be_packed_is_refused_and_nothing_is_written() {
    let sandbox = Sandbox::new("refused");
    let project = sandbox.project();

    // Not installed: there is no lockfile to say which gems the app needs.
    let stderr = failed(
        &sandbox.spinel(&project, &["build", "--compile", "bin/mytool", "-o", "app"]),
        "spinel build before spinel install",
    );
    assert!(
        stderr.contains("Gemfile.lock is missing; run `spinel install` first"),
        "{stderr}"
    );

    ok(&sandbox.spinel(&project, &["install"]), "spinel install");
    // An entry point the app's files leave out.
    let stderr = failed(
        &sandbox.spinel(
            &project,
            &[
                "build",
                "--compile",
                "bin/mytool",
                "-o",
                "app",
                "--exclude",
                "bin/",
            ],
        ),
        "spinel build of an excluded entry",
    );
    assert!(
        stderr.contains("--compile bin/mytool: .gitignore or --exclude leaves it out of the app"),
        "{stderr}"
    );
    // A gem the lockfile needs that the GEM_HOME has lost.
    fs::remove_file(project.join(".spinel/gem_home/gems/gamma-1.0.0")).unwrap();
    let stderr = failed(
        &sandbox.spinel(&project, &["build", "--compile", "bin/mytool", "-o", "app"]),
        "spinel build with a gem missing",
    );
    assert!(stderr.contains("gamma not installed in"), "{stderr}");

    // Neither the app nor its staging directory is left behind.
    let left: BTreeSet<String> = fs::read_dir(&project)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!left.iter().any(|name| name.starts_with("app")), "{left:?}");
}

#[test]
fn a_gemspec_project_never_packs_its_app_or_its_ignored_files() {
    // A `gemspec` Gemfile installs the project itself as a path gem, so
    // `gem_home/gems/mytool-1.0.0` leads back to the project, where the app
    // and its staging directory are written.
    let sandbox = Sandbox::new("gemspec");
    let project = sandbox.0.join("gemspec");
    ok(&sandbox.spinel(&project, &["install"]), "spinel install");
    fs::create_dir_all(project.join("tmp")).unwrap();
    fs::write(project.join("tmp/scratch.txt"), "left out\n").unwrap();
    let args = ["build", "--compile", "exe/mytool", "-o", "mytool"];

    ok(&sandbox.spinel(&project, &args), "spinel build");
    let first = payload(&project.join("mytool"));
    let entries = unpack(&first);
    assert!(
        !entries.keys().any(|path| path.contains(".spinel-build-")),
        "the staging directory is packed: {:?}",
        entries.keys()
    );
    assert_eq!(
        files_under(&entries, "app"),
        BTreeSet::from([
            ".bundle/config",
            ".gitignore",
            "Gemfile",
            "Gemfile.lock",
            "exe/mytool",
            "lib/mytool.rb",
            "mytool.gemspec",
        ])
    );
    assert_eq!(
        files_under(&entries, "gem_home/gems/mytool-1.0.0"),
        files_under(&entries, "app"),
        "the gem holds what app/ does: no tmp/, .spinel/ or app"
    );

    // A second build, with the first app in the project, packs the same.
    ok(&sandbox.spinel(&project, &args), "spinel build again");
    assert!(
        payload(&project.join("mytool")) == first,
        "the second build's payload equals the first"
    );
}

#[test]
fn an_entry_point_an_include_takes_back_from_gitignore_builds() {
    let sandbox = Sandbox::installed("include-entry");
    let project = sandbox.project();
    fs::write(project.join("tmp/main.rb"), "puts \"from tmp\"\n").unwrap();

    // `/tmp/` is ignored, so without `--include` the entry is refused.
    let stderr = failed(
        &sandbox.spinel(
            &project,
            &["build", "--compile", "tmp/main.rb", "-o", "../app"],
        ),
        "spinel build of an ignored entry",
    );
    assert!(
        stderr.contains("--compile tmp/main.rb: .gitignore or --exclude leaves it out of the app"),
        "{stderr}"
    );

    ok(
        &sandbox.spinel(
            &project,
            &[
                "build",
                "--compile",
                "tmp/main.rb",
                "--include",
                "tmp/main.rb",
                "-o",
                "../app",
            ],
        ),
        "spinel build --include of an ignored entry",
    );
    let entries = unpack(&payload(&sandbox.0.join("app")));
    let manifest: serde_json::Value =
        serde_json::from_slice(bytes(&entries, "manifest.json")).unwrap();
    assert_eq!(manifest["entry"], "tmp/main.rb");
    assert_eq!(bytes(&entries, "app/tmp/main.rb"), b"puts \"from tmp\"\n");
}
