//! Pack the vendored pure-Ruby standard library, `stdlib/`, into the archive
//! `src/stdlib.rs` extracts on first run (#47).
//!
//! The archive is every file under `stdlib/`, sorted by path, each as a
//! little-endian `u32` path length, the path, a `u64` length and the bytes;
//! zstd-compressed. Beside it goes a digest of the uncompressed archive, which
//! names the directory it is extracted to, so a binary built from a different
//! `stdlib/` never reads another's copy.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let stdlib = manifest.join("../../stdlib");
    println!("cargo:rerun-if-changed={}", stdlib.display());

    let mut files = Vec::new();
    collect(&stdlib, &stdlib, &mut files);
    files.sort();

    let mut archive = Vec::new();
    for relative in &files {
        let bytes = std::fs::read(stdlib.join(relative)).expect("a file under stdlib/ is readable");
        let name = relative.to_string_lossy().replace('\\', "/");
        archive.extend_from_slice(&(name.len() as u32).to_le_bytes());
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        archive.extend_from_slice(&bytes);
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it"));
    let compressed = zstd::encode_all(&archive[..], 19).expect("zstd compresses in memory");
    std::fs::write(out.join("stdlib.zst"), compressed).expect("OUT_DIR is writable");
    std::fs::write(
        out.join("stdlib.digest"),
        format!("{:016x}", fnv1a(&archive)),
    )
    .expect("OUT_DIR is writable");
}

/// Every file under `dir`, as a path relative to `root`.
fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("stdlib/ is readable");
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("under the root")
                    .to_path_buf(),
            );
        }
    }
}

/// FNV-1a, 64 bits: enough to tell two `stdlib/` trees apart.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
