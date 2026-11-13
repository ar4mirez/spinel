//! Work out `RUBY_PLATFORM` for the target being built, as CRuby does.
//!
//! CRuby's `configure` bakes the platform in at build time, and RubyGems reads it
//! back through `RbConfig::CONFIG["arch"]` to decide which native gems fit
//! (`Gem::Platform.local`). So Spinel spells it the same way and also decides it
//! when it is built: `x86_64-linux`, `aarch64-linux`, `x86_64-linux-musl`,
//! `arm64-darwin24`. The result reaches `lib.rs` as `SPINEL_RUBY_PLATFORM`.
//!
//! It also fingerprints the compiler for the bytecode cache (#40,
//! `src/sbc.rs`): a hash of every source that decides what a file compiles
//! to, as `SPINEL_BYTECODE_FINGERPRINT`. A cache file written by a build with
//! another fingerprint is stale and is parsed again.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let var = |name: &str| env::var(name).unwrap_or_default();
    let arch = var("CARGO_CFG_TARGET_ARCH");
    let os = var("CARGO_CFG_TARGET_OS");
    let target_env = var("CARGO_CFG_TARGET_ENV");
    let native = var("HOST") == var("TARGET");
    println!(
        "cargo:rustc-env=SPINEL_RUBY_PLATFORM={}",
        platform(&arch, &os, &target_env, native)
    );
    println!(
        "cargo:rustc-env=SPINEL_BYTECODE_FINGERPRINT={}",
        bytecode_fingerprint()
    );
}

/// What a file compiles to depends on the lowering (`spinel-parse`, and
/// through its lock entry the Prism it wraps), on the tree it lowers to
/// (`spinel-ast`), and on the compiler and the instruction set (this crate).
/// So the fingerprint hashes all of their sources, by path and contents, and
/// the lock file. Coarser than it has to be — a change to the interpreter
/// alone also invalidates every cache — but never finer, which is the
/// direction that would run stale bytecode.
///
/// A source tree without the sibling crates (a packaged build) hashes what
/// it has: this crate's own sources still pin the instruction set.
fn bytecode_fingerprint() -> String {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let crates = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut inputs = vec![
        manifest.join("src"),
        crates.join("spinel-parse/src"),
        crates.join("spinel-parse/Cargo.toml"),
        crates.join("spinel-ast/src"),
        crates.join("spinel-ast/Cargo.toml"),
    ];
    if let Some(root) = crates.parent() {
        inputs.push(root.join("Cargo.lock"));
    }
    let mut files = Vec::new();
    for input in &inputs {
        println!("cargo:rerun-if-changed={}", input.display());
        collect(input, &mut files);
    }
    files.sort();
    let mut hasher = Sha256::new();
    hasher.update(env::var("CARGO_PKG_VERSION").unwrap_or_default().as_bytes());
    for file in &files {
        let Ok(contents) = std::fs::read(file) else {
            continue;
        };
        let name = file.strip_prefix(&crates).unwrap_or(file);
        let name = name.to_string_lossy();
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(&contents);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Every file at or under `path`.
fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            collect(&entry.path(), files);
        }
    }
}

fn platform(arch: &str, os: &str, target_env: &str, native: bool) -> String {
    match os {
        // CRuby on Linux names the CPU as `uname -m` does and adds the libc only
        // when it is not glibc.
        "linux" if target_env == "musl" => format!("{arch}-linux-musl"),
        "linux" => format!("{arch}-linux"),
        // Apple calls aarch64 `arm64`, and CRuby keeps the Darwin kernel's major
        // version: `arm64-darwin24` on macOS 15.
        "macos" => {
            let cpu = if arch == "aarch64" { "arm64" } else { arch };
            // ponytail: the Darwin version comes from the build host's `uname -r`,
            // which is what CRuby does too, so a cross build from another OS gets a
            // bare `darwin`. Gem::Platform still parses that, matching any version.
            // Upgrade path: derive it from MACOSX_DEPLOYMENT_TARGET (macOS N is
            // Darwin N + 9 from macOS 11 on) when cross-building.
            match native.then(darwin_major).flatten() {
                Some(major) => format!("{cpu}-darwin{major}"),
                None => format!("{cpu}-darwin"),
            }
        }
        // ponytail: other targets keep Rust's names (`x86_64-freebsd`), which RubyGems
        // parses but may not spell as CRuby does there. Spinel's CI builds Linux and
        // macOS; a new target adds its row here with the platform its CRuby reports.
        os => format!("{arch}-{os}"),
    }
}

/// The major version of the running Darwin kernel, from `uname -r` (`24.6.0`).
fn darwin_major() -> Option<u32> {
    let out = Command::new("uname").arg("-r").output().ok()?;
    let release = String::from_utf8(out.stdout).ok()?;
    release.trim().split('.').next()?.parse().ok()
}
