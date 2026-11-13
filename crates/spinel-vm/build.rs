//! Work out `RUBY_PLATFORM` for the target being built, as CRuby does.
//!
//! CRuby's `configure` bakes the platform in at build time, and RubyGems reads it
//! back through `RbConfig::CONFIG["arch"]` to decide which native gems fit
//! (`Gem::Platform.local`). So Spinel spells it the same way and also decides it
//! when it is built: `x86_64-linux`, `aarch64-linux`, `x86_64-linux-musl`,
//! `arm64-darwin24`. The result reaches `lib.rs` as `SPINEL_RUBY_PLATFORM`.

use std::env;
use std::process::Command;

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
