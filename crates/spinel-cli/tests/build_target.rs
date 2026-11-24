//! `spinel build --target <triple>` (#102, docs/build.md "Cross-compiling"):
//! the payload is written into the `spinel` release binary of this version
//! for the target, fetched from `SPINEL_RELEASES_URL` (here a `file://`
//! directory standing in for the GitHub release) and kept under
//! `$SPINEL_HOME/targets/<version>/<triple>/spinel`, so a second build
//! fetches nothing.
//!
//! The host's "release" is a copy of the `spinel` under test, so its app
//! runs; another platform's is a crafted ELF header, which proves the
//! payload is written after the target's bytes and that the binary's
//! platform is checked before it is used.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Variables a test's `spinel` must not inherit from the shell running it.
const SCRUBBED: [&str; 10] = [
    "GEM_HOME",
    "GEM_PATH",
    "BUNDLE_GEMFILE",
    "BUNDLE_PATH",
    "BUNDLE_APP_CONFIG",
    "BUNDLE_FROZEN",
    "RUBYOPT",
    "RUBYLIB",
    "XDG_CACHE_HOME",
    "SPINEL_RELEASES_URL",
];

/// The platform this test runs on, as `--target` spells it.
fn host() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// A Linux platform other than this one, which any host can build for.
fn foreign() -> &'static str {
    if std::env::consts::ARCH == "x86_64" && std::env::consts::OS == "linux" {
        "aarch64-linux"
    } else {
        "x86_64-linux"
    }
}

/// The first bytes of a 64-bit little-endian ELF executable for `triple`'s
/// CPU, then filler: no program, but every byte `--target` checks.
fn elf_for(triple: &str) -> Vec<u8> {
    let machine: u16 = if triple.starts_with("aarch64") {
        0xb7
    } else {
        0x3e
    };
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2; // ELFCLASS64
    bytes[5] = 1; // ELFDATA2LSB
    bytes[6] = 1; // EV_CURRENT
    bytes[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes.extend(
        format!("a stand-in spinel for {triple}\n")
            .bytes()
            .cycle()
            .take(4000),
    );
    bytes
}

/// A fresh copy of `fixtures/app` for one test, with its path gem installed,
/// removed again when it drops.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("build_target")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app"),
            &root,
        );
        fs::create_dir_all(root.join("releases").join(format!("v{VERSION}")))
            .expect("the release directory is made");
        let sandbox = Sandbox(fs::canonicalize(&root).expect("the copy exists"));
        ok(&sandbox.spinel(&["install"], &[]), "spinel install");
        sandbox
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    /// Where the stand-in release keeps `triple`'s binary.
    fn release(&self, triple: &str) -> PathBuf {
        self.0
            .join("releases")
            .join(format!("v{VERSION}"))
            .join(format!("spinel-{triple}"))
    }

    fn publish(&self, triple: &str, bytes: &[u8]) {
        let path = self.release(triple);
        fs::write(&path, bytes).expect("the release binary is written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// Where a build keeps `triple`'s binary.
    fn cached(&self, triple: &str) -> PathBuf {
        self.0
            .join("home/targets")
            .join(VERSION)
            .join(triple)
            .join("spinel")
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

    /// `spinel`, in the project, with the stand-in release as its
    /// `SPINEL_RELEASES_URL` unless `env` names another.
    fn spinel(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = self.command(Path::new(SPINEL), &self.project());
        command.env(
            "SPINEL_RELEASES_URL",
            format!("file://{}", self.0.join("releases").display()),
        );
        for (key, value) in env {
            command.env(key, value);
        }
        command.args(args).output().expect("spinel runs")
    }

    fn build(&self, out: &str, target: &str) -> Output {
        self.spinel(
            &[
                "build",
                "--compile",
                "bin/mytool",
                "-o",
                out,
                "--target",
                target,
            ],
            &[],
        )
    }

    fn run(&self, app: &Path, args: &[&str]) -> Output {
        self.command(app, &self.0)
            .env("XDG_CACHE_HOME", self.0.join("cache"))
            .args(args)
            .output()
            .expect("the app runs")
    }

    /// The names in the target's cache directory: the binary and nothing
    /// half-written beside it.
    fn cache_dir(&self, triple: &str) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.cached(triple).parent().expect("a directory"))
            .map(|dir| {
                dir.flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
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

#[test]
fn the_target_binary_is_fetched_once_and_the_app_runs() {
    let sandbox = Sandbox::new("host");
    let host = host();
    let spinel = fs::read(SPINEL).expect("the spinel under test reads");
    sandbox.publish(&host, &spinel);

    ok(&sandbox.build("out/mytool", &host), "spinel build --target");
    assert_eq!(
        fs::read(sandbox.cached(&host)).expect("the binary is cached"),
        spinel
    );
    assert_eq!(sandbox.cache_dir(&host), ["spinel"]);
    let app = sandbox.project().join("out/mytool");
    let out = ok(&sandbox.run(&app, &["one"]), "the app");
    assert!(out.contains("shout: ONE"), "{out}");

    // The release is gone, so a second build that works fetched nothing.
    // Same file name, another directory: on macOS `codesign` takes the
    // app's identifier from its file name, so only then are the bytes equal.
    fs::remove_file(sandbox.release(&host)).expect("the release is removed");
    ok(
        &sandbox.build("again/mytool", &host),
        "spinel build --target from the cache",
    );
    let again = sandbox.project().join("again/mytool");
    let out = ok(&sandbox.run(&again, &["two"]), "the second app");
    assert!(out.contains("shout: TWO"), "{out}");
    assert_eq!(
        fs::read(&app).expect("the app reads"),
        fs::read(&again).expect("the second app reads"),
        "the same project builds the same app from the cache"
    );
}

#[test]
fn a_foreign_target_gets_the_payload_after_its_binary() {
    let sandbox = Sandbox::new("foreign");
    let triple = foreign();
    let binary = elf_for(triple);
    sandbox.publish(triple, &binary);

    let out = ok(
        &sandbox.build("out/mytool", triple),
        "spinel build --target",
    );
    let app = fs::read(sandbox.project().join("out/mytool")).expect("the app reads");
    assert!(
        app.starts_with(&binary),
        "the app is the target's binary first"
    );

    // `[ binary ][ payload ][ sha256 ][ length LE ][ "SPINEL01" ]`
    let trailer = &app[app.len() - 48..];
    assert_eq!(&trailer[40..], b"SPINEL01");
    let len = u64::from_le_bytes(trailer[32..40].try_into().expect("8 bytes"));
    assert_eq!(app.len() as u64, binary.len() as u64 + len + 48);
    assert!(out.contains(&format!("{len} byte payload")), "{out}");
    let mode = fs::metadata(sandbox.project().join("out/mytool"))
        .expect("the app is there")
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "the app is executable");
}

#[test]
fn a_binary_for_another_platform_is_refused_and_not_kept() {
    let sandbox = Sandbox::new("wrong");
    let triple = foreign();
    let other = if triple == "aarch64-linux" {
        "x86_64-linux"
    } else {
        "aarch64-linux"
    };
    sandbox.publish(triple, &elf_for(other));
    let err = failed(&sandbox.build("out/mytool", triple), "a wrong binary");
    assert!(
        err.contains(&format!(
            "the spinel for {triple} is an executable for {other}"
        )),
        "{err}"
    );

    sandbox.publish(triple, b"#!/bin/sh\necho not spinel\n");
    let err = failed(&sandbox.build("out/mytool", triple), "a script");
    assert!(
        err.contains(&format!(
            "the spinel for {triple} is not a Linux or macOS executable"
        )),
        "{err}"
    );
    assert!(!sandbox.cached(triple).exists());
    assert!(
        sandbox.cache_dir(triple).is_empty(),
        "{:?}",
        sandbox.cache_dir(triple)
    );
    assert!(!sandbox.project().join("out/mytool").exists());
}

#[test]
fn a_missing_release_names_the_target_and_the_version() {
    let sandbox = Sandbox::new("missing");
    let triple = foreign();
    let err = failed(&sandbox.build("out/mytool", triple), "no release");
    assert!(
        err.contains(&format!("no spinel {VERSION} for {triple}")),
        "{err}"
    );
    assert!(
        err.contains(&format!("v{VERSION}/spinel-{triple}")),
        "{err}"
    );
    assert!(!sandbox.project().join("out/mytool").exists());
    assert!(sandbox.cache_dir(triple).is_empty());
}

#[test]
fn an_unknown_target_and_a_plain_http_mirror_are_refused() {
    let sandbox = Sandbox::new("refused");
    let err = failed(
        &sandbox.build("out/mytool", "arm64-darwin"),
        "an unknown target",
    );
    assert!(
        err.contains("--target arm64-darwin: not a target; one of x86_64-linux, aarch64-linux, x86_64-macos, aarch64-macos"),
        "{err}"
    );

    let err = failed(
        &sandbox.spinel(
            &[
                "build",
                "--compile",
                "bin/mytool",
                "-o",
                "out/mytool",
                "--target",
                foreign(),
            ],
            &[("SPINEL_RELEASES_URL", "http://127.0.0.1:9/releases")],
        ),
        "an http mirror",
    );
    assert!(err.contains("HTTPS only"), "{err}");
    assert!(!sandbox.project().join("out/mytool").exists());
}

#[test]
fn a_gem_with_a_native_extension_stops_a_cross_build() {
    let sandbox = Sandbox::new("native");
    fs::write(
        sandbox.0.join("gems/shout/lib/shout_ext.so"),
        b"built for this machine",
    )
    .expect("the extension is written");
    let triple = foreign();
    sandbox.publish(triple, &elf_for(triple));

    let err = failed(&sandbox.build("out/mytool", triple), "a native gem");
    assert!(
        err.contains("shout-1.0.0 has a native extension") && err.contains("shout_ext.so"),
        "{err}"
    );
    assert!(err.contains(&format!("--target {triple}")), "{err}");
    assert!(!sandbox.project().join("out/mytool").exists());

    // Without `--target` the build is what it was: for this machine.
    ok(
        &sandbox.spinel(
            &["build", "--compile", "bin/mytool", "-o", "out/mytool"],
            &[],
        ),
        "spinel build",
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn an_app_for_macos_is_built_on_macos() {
    let sandbox = Sandbox::new("macos");
    let err = failed(
        &sandbox.build("out/mytool", "aarch64-macos"),
        "a macOS target from Linux",
    );
    assert!(err.contains("built on macOS"), "{err}");
    assert!(!sandbox.cached("aarch64-macos").exists());
}
