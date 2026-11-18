//! The native-extension loader (#104), end to end: the example gem in
//! `tests/fixtures/native-gem`, its extension built from
//! `ext/native_gem/` with cargo, installed by `spinel install` as a path gem
//! and required by a program, run by `spinel run` and as an app
//! `spinel build --compile` wrote. And `require` refusing a `.spinel` this
//! `spinel` will not load, with a `LoadError` rather than a crash.
//!
//! Nothing here touches the network: the gem comes from a path, and cargo
//! builds `spinel-ext`, which depends on nothing, `--offline`. Every build
//! and install writes under Cargo's scratch directory for tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// What a run must not inherit from the shell that runs the tests.
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

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn scratch() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("native_gem")
}

/// Copy a directory tree.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("a directory is made");
    for entry in fs::read_dir(from).expect("the fixture reads") {
        let entry = entry.expect("a directory entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a file is copied");
        }
    }
}

/// Build the extension crate at `source` as a `cdylib` named `name`,
/// against this checkout's `spinel-ext`, from a copy under the scratch
/// directory so that no lockfile or target is written in the source tree;
/// answer the library's path.
fn cargo_build(source: &Path, name: &str, spinel_ext_path: &str) -> PathBuf {
    let root = scratch().join("build");
    let crate_dir = root.join(name);
    let _ = fs::remove_dir_all(&crate_dir);
    copy_dir(source, &crate_dir);
    let spinel_ext = manifest_dir().join("../spinel-ext");
    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml"))
        .expect("the crate's manifest")
        .replace(
            &format!("path = {spinel_ext_path:?}"),
            &format!("path = {:?}", spinel_ext.display().to_string()),
        );
    fs::write(crate_dir.join("Cargo.toml"), manifest).expect("the manifest is rewritten");
    let target = root.join("target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "--offline", "--jobs", "2"])
        .arg("--manifest-path")
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building {name} failed");
    target.join("debug").join(format!(
        "{}{name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

/// The example gem's extension, built once for every test in this file.
fn native_gem_library() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        cargo_build(
            &manifest_dir().join("tests/fixtures/native-gem/ext/native_gem"),
            "native_gem",
            "../../../../../../spinel-ext",
        )
    })
}

/// `spinel-vm`'s `stale` fixture (#103): an extension whose header says ABI
/// 0, which no `spinel` loads, and whose init aborts the process.
fn stale_library() -> PathBuf {
    cargo_build(
        &manifest_dir().join("../spinel-vm/tests/fixtures/ext/stale"),
        "stale",
        "../../../../../spinel-ext",
    )
}

/// A fresh directory for one test, removed again when it drops.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = scratch().join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).expect("the project is made");
        Sandbox(fs::canonicalize(&root).expect("the sandbox exists"))
    }

    /// The example gem, beside the project, with its extension built into
    /// `lib/native_gem/native_gem.spinel` as a precompiled gem ships it.
    fn with_gem(name: &str) -> Sandbox {
        let sandbox = Sandbox::new(name);
        let gem = sandbox.0.join("native_gem");
        copy_dir(&manifest_dir().join("tests/fixtures/native-gem"), &gem);
        fs::copy(
            native_gem_library(),
            gem.join("lib/native_gem/native_gem.spinel"),
        )
        .expect("the extension is put in the gem");
        sandbox.write("Gemfile", "gem \"native_gem\", path: \"../native_gem\"\n");
        sandbox
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.project().join(name);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its directory is made");
        fs::write(path, text).expect("a project file is written");
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.0.join("home"))
            .env("HOME", self.0.join("user"))
            .env("XDG_CACHE_HOME", self.0.join("cache"));
        for var in SCRUBBED {
            if var != "XDG_CACHE_HOME" {
                command.env_remove(var);
            }
        }
        command
    }

    fn spinel(&self, args: &[&str]) -> Output {
        self.command(Path::new(SPINEL))
            .args(args)
            .output()
            .expect("spinel runs")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Standard output of a run that must succeed.
fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed ({:?}):\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).expect("UTF-8 output")
}

/// A program using every part of the gem: the Ruby half, the native
/// methods, conversions both ways, each kind of raise, and `require`'s
/// answers once the extension is loaded.
const DEMO: &str = r#"require "native_gem"
p NativeGem::VERSION
p NativeGem.reverse("spinel")
p NativeGem.reverse("wörld")
p NativeGem.mirror("ab")
p NativeGem.sum(1, 2, 39)
p NativeGem.sum(2**62, 2**62 - 1)
p NativeGem.sum
p NativeGem.palindrome?("racecar")
p NativeGem.palindrome?("ruby")
p NativeGem::Words.new.count("one two  three")
p NativeGem::Words.new.count("  ")
p NativeGem.positive!(3)
begin
  NativeGem.positive!(0)
rescue NativeGem::Error => e
  p [e.class, e.message]
end
p NativeGem::Error.superclass
begin
  NativeGem.sum(1, "2")
rescue TypeError => e
  p e.class
end
begin
  NativeGem.sum(2**62, 2**62)
rescue RangeError => e
  p e.message
end
begin
  NativeGem.reverse
rescue ArgumentError => e
  p e.message
end
p NativeGem.method(:reverse).arity
p NativeGem.method(:sum).arity
p require("native_gem/native_gem")
p require("native_gem/native_gem.so")
p $LOADED_FEATURES.select { |f| f.include?("/lib/native_gem") }.map { |f| File.basename(f) }.sort
p $LOADED_FEATURES.find { |f| f.end_with?(".spinel") } == $LOAD_PATH.resolve_feature_path("native_gem/native_gem")[1]
"#;

const DEMO_OUTPUT: &str = r#""0.1.0"
"lenips"
"dlröw"
"abba"
42
9223372036854775807
0
true
false
3
nil
3
[NativeGem::Error, "0 is not positive"]
StandardError
TypeError
"sum overflows a 64-bit integer"
"wrong number of arguments (given 0, expected 1)"
1
-1
false
false
["native_gem.rb", "native_gem.spinel", "version.rb"]
true
"#;

#[test]
fn the_example_gem_installs_and_its_extension_loads() {
    let sandbox = Sandbox::with_gem("run");
    sandbox.write("demo.rb", DEMO);
    assert_eq!(
        ok(&sandbox.spinel(&["install"]), "spinel install"),
        "+ native_gem 0.1.0\n"
    );
    assert_eq!(
        ok(&sandbox.spinel(&["run", "demo.rb"]), "spinel run"),
        DEMO_OUTPUT
    );
}

#[test]
fn a_compiled_app_carries_the_extension() {
    let sandbox = Sandbox::with_gem("app");
    sandbox.write("bin/demo", DEMO);
    ok(&sandbox.spinel(&["install"]), "spinel install");
    ok(
        &sandbox.spinel(&["build", "--compile", "bin/demo", "-o", "out/demo"]),
        "spinel build --compile",
    );
    // Run from outside the project, with the gem the project installed gone,
    // so the extension can only come from the app's own extracted payload.
    fs::remove_dir_all(sandbox.0.join("native_gem")).expect("the gem is removed");
    let out = sandbox
        .command(&sandbox.project().join("out/demo"))
        .current_dir(&sandbox.0)
        .output()
        .expect("the app runs");
    assert_eq!(ok(&out, "the app"), DEMO_OUTPUT);
}

#[test]
fn an_extension_spinel_will_not_load_is_a_load_error() {
    let sandbox = Sandbox::new("refused");
    // Its init aborts the process, so getting an answer at all is the proof
    // that a refused extension runs none of its code.
    fs::create_dir_all(sandbox.project().join("lib")).expect("lib/ is made");
    fs::copy(stale_library(), sandbox.project().join("lib/stale.spinel"))
        .expect("the stale extension is copied");
    sandbox.write("lib/garbage.spinel", "not a shared object\n");
    let program = r#"
%w[stale garbage].each do |name|
  2.times do
    require name
  rescue LoadError => e
    puts e.message.include?("lib/#{name}.spinel")
    puts e.path.inspect
  end
end
p $LOADED_FEATURES.grep(/\.spinel\z/)
"#;
    let out = sandbox.spinel(&["run", "-I", "lib", "-e", program]);
    let stdout = ok(&out, "spinel run");
    assert_eq!(
        stdout, "true\nnil\ntrue\nnil\ntrue\nnil\ntrue\nnil\n[]\n",
        "{stdout}"
    );
    // The ABI refusal says what to do about it.
    let out = sandbox.spinel(&["run", "-I", "lib", "-e", "require 'stale'"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ABI 0"), "{stderr}");
    assert!(stderr.contains("(LoadError)"), "{stderr}");
}
