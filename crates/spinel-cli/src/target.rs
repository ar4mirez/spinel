//! `spinel build --target <triple>` (#102, docs/build.md "Cross-compiling"):
//! the `spinel` release binary for another platform, which the payload is
//! written into in place of the running one.
//!
//! The binary for a target is the release asset
//! `<base>/v<version>/spinel-<triple>`, `<version>` being this `spinel`'s, so
//! the app runs the engine that compiled its bytecode. `<base>` is the
//! project's GitHub releases (`DEFAULT_BASE`), or `SPINEL_RELEASES_URL` for a
//! mirror or a test; it is fetched over HTTPS, or read from a `file://` base.
//! A fetched binary is checked to be an executable for the target before it
//! is kept, under `$SPINEL_HOME/targets/<version>/<triple>/spinel`, written
//! beside its place and renamed into it, so a cached binary is always whole
//! and a later build reuses it without the network.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use reqwest::{StatusCode, Url};

/// Where release assets are fetched from when `SPINEL_RELEASES_URL` is unset.
pub const DEFAULT_BASE: &str = "https://github.com/ar4mirez/spinel/releases/download";

/// The variable that replaces `DEFAULT_BASE`.
pub const BASE_VAR: &str = "SPINEL_RELEASES_URL";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arch {
    X86_64,
    Aarch64,
}

/// A platform `spinel build` can write an app for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// The name `--target` takes and the release asset is named by.
    pub name: &'static str,
    pub os: Os,
    arch: Arch,
}

/// Every target, as `--target` spells it.
pub const TARGETS: [Target; 4] = [
    Target {
        name: "x86_64-linux",
        os: Os::Linux,
        arch: Arch::X86_64,
    },
    Target {
        name: "aarch64-linux",
        os: Os::Linux,
        arch: Arch::Aarch64,
    },
    Target {
        name: "x86_64-macos",
        os: Os::Macos,
        arch: Arch::X86_64,
    },
    Target {
        name: "aarch64-macos",
        os: Os::Macos,
        arch: Arch::Aarch64,
    },
];

impl Target {
    /// The target `--target` names.
    pub fn parse(name: &str) -> Result<Target, String> {
        TARGETS
            .iter()
            .find(|target| target.name == name)
            .copied()
            .ok_or_else(|| {
                let known: Vec<&str> = TARGETS.iter().map(|target| target.name).collect();
                format!("--target {name}: not a target; one of {}", known.join(", "))
            })
    }

    /// The `spinel` for this target at `version`: the cached copy under
    /// `home` when there is one, otherwise fetched from `base` and kept
    /// there. Either way it is checked to be an executable for the target.
    pub fn binary(self, version: &str, base: &str, home: &Path) -> Result<PathBuf, String> {
        let dir = home.join("targets").join(version).join(self.name);
        let path = dir.join("spinel");
        if path.is_file() {
            self.check(&path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            return Ok(path);
        }
        let url = asset_url(base, version, self.name)?;
        fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let partial = dir.join(format!(".spinel.{}.partial", std::process::id()));
        let fetched = fetch(&url, &partial)
            .map_err(|err| match err {
                Fetch::Missing => {
                    format!("no spinel {version} for {}: {url} is not there", self.name)
                }
                Fetch::Failed(message) => format!(
                    "cannot fetch spinel {version} for {} from {url}: {message}",
                    self.name
                ),
            })
            .and_then(|()| {
                fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))
                    .map_err(|err| format!("{}: {err}", partial.display()))
            })
            .and_then(|()| self.check(&partial).map_err(|err| format!("{url}: {err}")))
            .and_then(|()| {
                fs::rename(&partial, &path).map_err(|err| format!("{}: {err}", path.display()))
            });
        if fetched.is_err() {
            let _ = fs::remove_file(&partial);
        }
        fetched.map(|()| path)
    }

    /// Whether the file at `path` is an executable for this target: a 64-bit
    /// little-endian ELF executable for its CPU on Linux, a thin 64-bit
    /// Mach-O executable for its CPU on macOS.
    pub fn check(self, path: &Path) -> Result<(), String> {
        let mut head = Vec::with_capacity(64);
        File::open(path)
            .and_then(|file| file.take(64).read_to_end(&mut head))
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let found = Executable::of(&head);
        if found == Some((self.os, self.arch)) {
            return Ok(());
        }
        let what = match found {
            Some((os, arch)) => format!("an executable for {}", name_of(os, arch)),
            None => "not a Linux or macOS executable".to_owned(),
        };
        Err(format!("the spinel for {} is {what}", self.name))
    }
}

fn name_of(os: Os, arch: Arch) -> &'static str {
    TARGETS
        .iter()
        .find(|target| target.os == os && target.arch == arch)
        .map_or("an unknown platform", |target| target.name)
}

/// The base release assets are fetched from: `SPINEL_RELEASES_URL` when set
/// and not empty, else `DEFAULT_BASE`.
pub fn base() -> String {
    std::env::var(BASE_VAR)
        .ok()
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_owned())
}

/// `<base>/v<version>/spinel-<triple>`, for an `https` or `file` base.
fn asset_url(base: &str, version: &str, triple: &str) -> Result<Url, String> {
    let url = Url::parse(&format!(
        "{}/v{version}/spinel-{triple}",
        base.trim_end_matches('/')
    ))
    .map_err(|err| format!("{BASE_VAR}={base}: {err}"))?;
    match url.scheme() {
        "https" | "file" => Ok(url),
        _ => Err(format!(
            "{BASE_VAR}={base}: not an https or file URL; release binaries are fetched over HTTPS only"
        )),
    }
}

enum Fetch {
    /// The asset is not there: a 404, or no such file.
    Missing,
    Failed(String),
}

/// The asset at `url`, streamed to `to`.
fn fetch(url: &Url, to: &Path) -> Result<(), Fetch> {
    if url.scheme() == "file" {
        let from = url
            .to_file_path()
            .map_err(|()| Fetch::Failed("not a local path".to_owned()))?;
        let mut source = File::open(&from).map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => Fetch::Missing,
            _ => Fetch::Failed(err.to_string()),
        })?;
        let mut out = File::create(to).map_err(|err| Fetch::Failed(err.to_string()))?;
        io::copy(&mut source, &mut out).map_err(|err| Fetch::Failed(err.to_string()))?;
        return out.sync_all().map_err(|err| Fetch::Failed(err.to_string()));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| Fetch::Failed(err.to_string()))?;
    runtime.block_on(download(url, to))
}

async fn download(url: &Url, to: &Path) -> Result<(), Fetch> {
    let failed = |err: reqwest::Error| Fetch::Failed(err.to_string());
    // A GitHub release asset redirects to its storage host; `https_only`
    // holds every hop to HTTPS.
    let client = reqwest::Client::builder()
        .user_agent(concat!("spinel/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .build()
        .map_err(failed)?;
    let mut response = client.get(url.clone()).send().await.map_err(failed)?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND => return Err(Fetch::Missing),
        status => return Err(Fetch::Failed(format!("HTTP {status}"))),
    }
    let mut out =
        io::BufWriter::new(File::create(to).map_err(|err| Fetch::Failed(err.to_string()))?);
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        out.write_all(&chunk)
            .map_err(|err| Fetch::Failed(err.to_string()))?;
    }
    out.into_inner()
        .map_err(|err| Fetch::Failed(err.error().to_string()))?
        .sync_all()
        .map_err(|err| Fetch::Failed(err.to_string()))
}

/// What a file's first bytes say it is.
struct Executable;

impl Executable {
    /// The platform `head`, a file's first 64 bytes or fewer, is an
    /// executable for, or `None` when it is no executable this knows.
    fn of(head: &[u8]) -> Option<(Os, Arch)> {
        let u16_at = |at: usize| Some(u16::from_le_bytes(head.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |at: usize| Some(u32::from_le_bytes(head.get(at..at + 4)?.try_into().ok()?));
        if head.starts_with(b"\x7fELF") {
            // ELFCLASS64, ELFDATA2LSB; `e_type` ET_EXEC or ET_DYN (a PIE).
            if head.get(4..6) != Some(&[2, 1]) || !matches!(u16_at(16)?, 2 | 3) {
                return None;
            }
            let arch = match u16_at(18)? {
                0x3e => Arch::X86_64,  // EM_X86_64
                0xb7 => Arch::Aarch64, // EM_AARCH64
                _ => return None,
            };
            return Some((Os::Linux, arch));
        }
        // MH_MAGIC_64, little-endian; `filetype` MH_EXECUTE.
        if u32_at(0)? == 0xfeed_facf && u32_at(12)? == 2 {
            let arch = match u32_at(4)? {
                0x0100_0007 => Arch::X86_64,  // CPU_TYPE_X86_64
                0x0100_000c => Arch::Aarch64, // CPU_TYPE_ARM64
                _ => return None,
            };
            return Some((Os::Macos, arch));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(machine: u16, kind: u16) -> Vec<u8> {
        let mut head = vec![0; 64];
        head[..4].copy_from_slice(b"\x7fELF");
        head[4] = 2;
        head[5] = 1;
        head[16..18].copy_from_slice(&kind.to_le_bytes());
        head[18..20].copy_from_slice(&machine.to_le_bytes());
        head
    }

    fn macho(cpu: u32, kind: u32) -> Vec<u8> {
        let mut head = vec![0; 32];
        head[..4].copy_from_slice(&0xfeed_facf_u32.to_le_bytes());
        head[4..8].copy_from_slice(&cpu.to_le_bytes());
        head[12..16].copy_from_slice(&kind.to_le_bytes());
        head
    }

    #[test]
    fn executables_are_told_apart_by_header() {
        assert_eq!(
            Executable::of(&elf(0x3e, 3)),
            Some((Os::Linux, Arch::X86_64))
        );
        assert_eq!(
            Executable::of(&elf(0xb7, 2)),
            Some((Os::Linux, Arch::Aarch64))
        );
        assert_eq!(
            Executable::of(&macho(0x0100_000c, 2)),
            Some((Os::Macos, Arch::Aarch64))
        );
        assert_eq!(
            Executable::of(&macho(0x0100_0007, 2)),
            Some((Os::Macos, Arch::X86_64))
        );
        // A relocatable object, a dylib, RISC-V, 32-bit, big-endian, a script.
        assert_eq!(Executable::of(&elf(0x3e, 1)), None);
        assert_eq!(Executable::of(&macho(0x0100_000c, 6)), None);
        assert_eq!(Executable::of(&elf(0xf3, 2)), None);
        let mut class32 = elf(0x3e, 2);
        class32[4] = 1;
        assert_eq!(Executable::of(&class32), None);
        let mut big = elf(0x3e, 2);
        big[5] = 2;
        assert_eq!(Executable::of(&big), None);
        assert_eq!(Executable::of(b"#!/bin/sh\n"), None);
        assert_eq!(Executable::of(b""), None);
        assert_eq!(Executable::of(b"\x7fELF"), None);
    }

    #[test]
    fn targets_parse_by_name_and_name_the_others_when_unknown() {
        for target in TARGETS {
            assert_eq!(Target::parse(target.name), Ok(target));
        }
        let err = Target::parse("arm64-darwin").unwrap_err();
        assert!(
            err.contains("aarch64-linux") && err.contains("x86_64-macos"),
            "{err}"
        );
    }

    #[test]
    fn asset_urls_are_https_or_file() {
        assert_eq!(
            asset_url(DEFAULT_BASE, "1.2.3", "aarch64-linux")
                .unwrap()
                .as_str(),
            "https://github.com/ar4mirez/spinel/releases/download/v1.2.3/spinel-aarch64-linux"
        );
        assert_eq!(
            asset_url("file:///mirror/", "1.2.3", "x86_64-macos")
                .unwrap()
                .as_str(),
            "file:///mirror/v1.2.3/spinel-x86_64-macos"
        );
        let err = asset_url("http://mirror.example", "1.2.3", "x86_64-linux").unwrap_err();
        assert!(err.contains("HTTPS only"), "{err}");
    }
}
