//! Running a compiled app (#101, docs/build.md "Runtime behavior of a
//! compiled app"): an app built by `spinel build --compile` extracts its
//! payload once to `$XDG_CACHE_HOME/spinel/apps/<sha256>/` and runs its entry
//! point there, its whole command line its own. A second run is a cache hit
//! that writes nothing, and a `spinel` with no payload is the plain CLI.
//!
//! Each test copies `fixtures/app` into a directory of its own, installs its
//! path gem offline, builds the app, and runs it with a cache directory of
//! the test's own.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// Variables a test's `spinel` must not inherit from the shell running it.
const SCRUBBED: [&str; 9] = [
    "GEM_HOME",
    "GEM_PATH",
    "BUNDLE_GEMFILE",
    "BUNDLE_PATH",
    "BUNDLE_APP_CONFIG",
    "BUNDLE_FROZEN",
    "RUBYOPT",
    "RUBYLIB",
    "XDG_CACHE_HOME",
];

/// A fresh directory for one test, under Cargo's scratch directory, removed
/// again when it drops: each app is a copy of the whole `spinel`.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("app")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app"),
            &root,
        );
        Sandbox(fs::canonicalize(&root).expect("the copy exists"))
    }

    /// The fixture installed and built into `project/out/mytool`, with
    /// `--env GREETING=hello` and `--from-manifest` after `--`; and the
    /// payload's sha256, which names the directory it is extracted to.
    fn built(name: &str) -> (Sandbox, String) {
        let sandbox = Sandbox::new(name);
        let project = sandbox.project();
        ok(&sandbox.spinel(&project, &["install"]), "spinel install");
        let out = ok(
            &sandbox.spinel(
                &project,
                &[
                    "build",
                    "--compile",
                    "bin/mytool",
                    "-o",
                    "out/mytool",
                    "--env",
                    "GREETING=hello",
                    "--",
                    "--from-manifest",
                ],
            ),
            "spinel build",
        );
        let hash = out
            .trim_end()
            .rsplit("sha256 ")
            .next()
            .expect("the build names the payload's sha256")
            .to_owned();
        assert_eq!(hash.len(), 64, "{out}");
        (sandbox, hash)
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    fn app(&self) -> PathBuf {
        self.project().join("out/mytool")
    }

    /// Where runs keep their cache: `XDG_CACHE_HOME`.
    fn cache(&self) -> PathBuf {
        self.0.join("cache")
    }

    /// `<cache>/spinel/apps/`, as its real path.
    fn apps(&self) -> PathBuf {
        fs::canonicalize(self.cache())
            .expect("the cache exists")
            .join("spinel/apps")
    }

    fn command(&self, program: &Path, dir: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(dir)
            .env("SPINEL_HOME", self.0.join("home"))
            .env("HOME", self.0.join("user"));
        for var in SCRUBBED {
            command.env_remove(var);
        }
        command
    }

    fn spinel(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(Path::new(SPINEL), dir)
            .args(args)
            .output()
            .expect("spinel runs")
    }

    /// The app, run from the sandbox rather than its project, so nothing of
    /// the project around it is what it finds.
    fn run(&self, args: &[&str]) -> Output {
        self.command(&self.app(), &self.0)
            .env("XDG_CACHE_HOME", self.cache())
            .args(args)
            .output()
            .expect("the app runs")
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

/// The names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Every file and directory under `dir`, by path, with what would change if
/// it were written again: its inode, length and modification time.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let meta = fs::symlink_metadata(&at).unwrap();
        out.insert(
            at.strip_prefix(dir).unwrap().to_path_buf(),
            (meta.ino(), meta.len(), meta.modified().unwrap()),
        );
        if meta.is_dir() {
            for entry in fs::read_dir(&at).unwrap() {
                stack.push(entry.unwrap().path());
            }
        }
    }
    out
}

/// The payload bytes of the app at `path`, found by its trailer: from the
/// end of the file on Linux, from the `__SPINEL,__payload` section on macOS.
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

/// The payload's files, by path.
fn payload_files(app: &Path) -> BTreeMap<String, Vec<u8>> {
    let bytes = payload(app);
    let mut archive = tar::Archive::new(zstd::Decoder::new(&bytes[..]).unwrap());
    let mut files = BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.header().entry_type().is_file() {
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            files.insert(path, content);
        }
    }
    files
}

#[test]
fn the_app_runs_its_entry_point_and_its_command_line_is_its_own() {
    let (sandbox, hash) = Sandbox::built("run");

    // `--help` is the app's: its usage, not Spinel's.
    let help = ok(&sandbox.run(&["--help"]), "mytool --help");
    assert_eq!(
        help,
        "Usage: mytool [--help] [WORD...]\nPrints its arguments, shouted.\n"
    );
    // So is `--version`, which the app does not know.
    let version = ok(&sandbox.run(&["--version", "two"]), "mytool --version");
    assert!(
        version.starts_with("argv: [\"--from-manifest\", \"--version\", \"two\"]\n"),
        "{version}"
    );

    let root = sandbox.apps().join(&hash);
    let out = ok(&sandbox.run(&["one", "two"]), "mytool one two");
    let at = |path: &str| root.join(path).display().to_string();
    assert_eq!(
        out,
        format!(
            "argv: [\"--from-manifest\", \"one\", \"two\"]\n\
             program: {}\n\
             file: {}\n\
             dir: {}\n\
             shout: --FROM-MANIFEST ONE TWO\n\
             gem_home: {}\n\
             greeting: hello\n\
             pwd: {}\n",
            at("app/bin/mytool"),
            at("app/lib/mytool.rb"),
            at("app/lib"),
            at("gem_home"),
            sandbox.0.display(),
        )
    );
}

#[test]
fn the_first_run_extracts_once_and_the_second_is_a_cache_hit() {
    let (sandbox, hash) = Sandbox::built("once");
    let first = ok(&sandbox.run(&["a"]), "the first run");

    // One directory, named by the payload's hash, and no staging left.
    let apps = sandbox.apps();
    assert_eq!(names(&apps), std::slice::from_ref(&hash));
    let root = apps.join(&hash);
    assert_eq!(
        names(&root),
        ["app", "bytecode", "gem_home", "manifest.json"]
    );
    // What the payload holds, byte for byte, but for its bytecode cache.
    let payload = payload_files(&sandbox.app());
    for (path, content) in &payload {
        if !path.starts_with("bytecode/") {
            assert_eq!(&fs::read(root.join(path)).unwrap(), content, "{path}");
        }
    }

    // The payload's bytecode is rebased onto the directory it landed in, so
    // the first run's `require` finds it: each entry is the one the VM looks
    // up, for the path it requires the file by, and reading it is a hit.
    let rebased = payload
        .iter()
        .filter(|(path, _)| path.starts_with("bytecode/"))
        .collect::<Vec<_>>();
    assert_eq!(rebased.len(), 2, "lib/mytool.rb and shout's lib/shout.rb");
    for source_path in [
        "app/lib/mytool.rb",
        "gem_home/gems/shout-1.0.0/lib/shout.rb",
    ] {
        let source = &payload[source_path];
        let name = format!(
            "bytecode/{}.sbc",
            Sha256::digest(source)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let expected = spinel_vm::sbc::rebase(&payload[&name], &root.to_string_lossy())
            .expect("the payload's entry rebases");
        assert_eq!(fs::read(root.join(&name)).unwrap(), expected, "{name}");
        let required = root.join(source_path);
        assert!(
            spinel_vm::sbc::read(
                &root.join("bytecode"),
                &required.to_string_lossy(),
                source,
                None
            )
            .is_some(),
            "{name} is a hit for {}",
            required.display()
        );
    }

    // The second run reads nothing new and writes nothing: every file the
    // first run left, the cache entries the VM wrote included, is the same
    // file, unchanged, and the output is the same.
    let before = snapshot(&root);
    let second = ok(&sandbox.run(&["a"]), "the second run");
    assert_eq!(second, first);
    assert_eq!(snapshot(&root), before);
    assert_eq!(names(&apps), [hash]);
}

#[test]
fn racing_first_runs_leave_one_copy() {
    let (sandbox, hash) = Sandbox::built("race");
    let children: Vec<_> = (0..4)
        .map(|_| {
            sandbox
                .command(&sandbox.app(), &sandbox.0)
                .env("XDG_CACHE_HOME", sandbox.cache())
                .arg("x")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("the app starts")
        })
        .collect();
    let outputs: Vec<String> = children
        .into_iter()
        .map(|child| ok(&child.wait_with_output().unwrap(), "a racing run"))
        .collect();
    assert!(
        outputs.windows(2).all(|pair| pair[0] == pair[1]),
        "{outputs:?}"
    );
    assert_eq!(names(&sandbox.apps()), [hash]);
}

#[test]
fn without_xdg_cache_home_the_cache_is_under_home() {
    let (sandbox, hash) = Sandbox::built("home");
    // Unset, and relative, which the XDG spec says to ignore.
    for xdg in [None, Some("relative/cache")] {
        let mut command = sandbox.command(&sandbox.app(), &sandbox.0);
        if let Some(xdg) = xdg {
            command.env("XDG_CACHE_HOME", xdg);
        }
        ok(&command.arg("--help").output().unwrap(), "mytool --help");
    }
    assert_eq!(names(&sandbox.0.join("user/.cache/spinel/apps")), [hash]);
    assert!(!sandbox.0.join("relative").exists());
}

#[test]
fn a_spinel_without_a_payload_is_the_plain_cli() {
    let sandbox = Sandbox::new("plain");
    let out = sandbox
        .command(Path::new(SPINEL), &sandbox.0)
        .env("XDG_CACHE_HOME", sandbox.cache())
        .arg("--help")
        .output()
        .unwrap();
    let help = ok(&out, "spinel --help");
    assert!(help.contains("Usage: spinel"), "{help}");
    let out = sandbox
        .command(Path::new(SPINEL), &sandbox.0)
        .env("XDG_CACHE_HOME", sandbox.cache())
        .args(["-e", "puts 6 * 7"])
        .output()
        .unwrap();
    assert_eq!(ok(&out, "spinel -e"), "42\n");
    // A plain `spinel` extracts nothing.
    assert!(!sandbox.cache().exists());
}

/// One entry of a hand-made archive: a path written into the header as
/// given, unchecked, a kind, and a link target or contents.
type Member<'a> = (&'a str, tar::EntryType, &'a [u8]);

/// A zstd-compressed tar of `entries`.
fn archive(entries: &[Member<'_>]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, kind, data) in entries {
        let mut header = tar::Header::new_gnu();
        let name = &mut header.as_gnu_mut().unwrap().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(*kind);
        header.set_mode(0o644);
        if kind.is_symlink() || kind.is_hard_link() {
            header
                .set_link_name(std::str::from_utf8(data).unwrap())
                .unwrap();
            header.set_size(0);
            header.set_cksum();
            tar.append(&header, &[][..]).unwrap();
        } else {
            header.set_size(data.len() as u64);
            header.set_cksum();
            tar.append(&header, *data).unwrap();
        }
    }
    zstd::encode_all(&tar.into_inner().unwrap()[..], 0).unwrap()
}

#[test]
fn a_payload_entry_that_reaches_outside_is_refused() {
    let sandbox = Sandbox::new("unsafe");
    let manifest: &[u8] = br#"{"entry":"main.rb","argv":[],"env":{}}"#;
    let cases: [(&str, Vec<Member<'_>>); 5] = [
        (
            "a path outside the payload",
            vec![("../escaped.txt", tar::EntryType::Regular, b"out")],
        ),
        (
            "a path outside the payload",
            vec![("app/../../escaped.txt", tar::EntryType::Regular, b"out")],
        ),
        (
            "a path outside the payload",
            vec![("/spinel-app-escaped.txt", tar::EntryType::Regular, b"out")],
        ),
        (
            "neither a file nor a directory",
            vec![("app", tar::EntryType::Symlink, b"/tmp")],
        ),
        (
            "neither a file nor a directory",
            vec![("app/main.rb", tar::EntryType::Link, b"/etc/passwd")],
        ),
    ];
    for (n, (what, entries)) in cases.into_iter().enumerate() {
        let mut all = vec![("manifest.json", tar::EntryType::Regular, manifest)];
        all.extend(entries);
        let payload = sandbox.0.join(format!("payload-{n}"));
        fs::write(&payload, archive(&all)).unwrap();
        let app = sandbox.0.join(format!("unsafe-{n}"));
        ok(
            &sandbox.spinel(
                &sandbox.0,
                &[
                    "pack",
                    &payload.to_string_lossy(),
                    "-o",
                    &app.to_string_lossy(),
                ],
            ),
            "spinel pack",
        );
        let out = sandbox
            .command(&app, &sandbox.0)
            .env("XDG_CACHE_HOME", sandbox.cache())
            .output()
            .unwrap();
        let err = failed(&out, what);
        assert!(err.contains("cannot be extracted"), "{err}");
        assert!(err.contains(what), "{err}");
        assert!(out.stdout.is_empty());
        // Nothing escaped, and nothing of the attempt is left.
        assert!(!sandbox.0.join("escaped.txt").exists());
        assert!(!sandbox.cache().join("spinel/escaped.txt").exists());
        assert!(!Path::new("/spinel-app-escaped.txt").exists());
        assert_eq!(names(&sandbox.apps()), Vec::<String>::new(), "{what}");
    }
}

// On macOS a byte flipped in the section breaks the signature, so the
// kernel, not the app, refuses it; `trailer_macos.rs` signs it again first.
#[test]
#[cfg(not(target_os = "macos"))]
fn a_corrupt_payload_is_refused_and_nothing_is_kept() {
    let (sandbox, _) = Sandbox::built("corrupt");
    // A byte of the payload flipped: the app still finds its trailer.
    let mut bytes = fs::read(sandbox.app()).unwrap();
    let payload = payload(&sandbox.app());
    let at = bytes
        .windows(payload.len())
        .rposition(|window| window == payload)
        .expect("the payload is in the app")
        + payload.len() / 2;
    bytes[at] ^= 0x01;
    let corrupt = sandbox.0.join("corrupt");
    fs::write(&corrupt, &bytes).unwrap();
    fs::set_permissions(&corrupt, fs::metadata(sandbox.app()).unwrap().permissions()).unwrap();
    let out = sandbox
        .command(&corrupt, &sandbox.0)
        .env("XDG_CACHE_HOME", sandbox.cache())
        .output()
        .unwrap();
    let err = failed(&out, "a corrupt app");
    assert!(err.contains("payload is corrupt"), "{err}");
    assert_eq!(names(&sandbox.apps()), Vec::<String>::new());
}
