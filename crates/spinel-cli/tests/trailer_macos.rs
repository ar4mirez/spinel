//! On macOS a compiled app keeps its payload and trailer in a
//! `__SPINEL,__payload` section and is signed again ad hoc (#99, docs/build.md
//! "Format"), because appending to a signed Mach-O breaks its signature. The
//! app `spinel pack` writes must pass `codesign --verify --strict`, execute,
//! and find its payload; the section's bytes must be exactly what Linux
//! appends.
//!
//! Gatekeeper's first-run prompt is for downloaded, quarantined files and
//! cannot be raised here: a valid signature and a binary that runs are the
//! evidence.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// A fresh empty directory for one test, removed again when it drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "spinel-trailer-macos-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A payload larger than a 16 KiB page, so writing it moves `__LINKEDIT`.
fn payload() -> Vec<u8> {
    (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect()
}

/// The trailer exactly as docs/build.md lays it out, built here independently
/// of the binary's own writer: sha256, length as u64 little-endian, magic.
fn trailer(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Sha256::digest(payload).to_vec();
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(b"SPINEL01");
    bytes
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `spinel pack` `payload` into `dir/mytool`.
fn pack(dir: &Path, payload: &[u8]) -> PathBuf {
    let input = dir.join("payload");
    fs::write(&input, payload).expect("write payload");
    let exe = dir.join("mytool");
    let out = Command::new(SPINEL)
        .arg("pack")
        .arg(&input)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("spinel pack runs");
    assert!(out.status.success(), "pack: {}", stderr(&out));
    exe
}

fn codesign(args: &[&str], path: &Path) -> Output {
    Command::new("codesign")
        .args(args)
        .arg(path)
        .output()
        .expect("codesign runs")
}

fn verify_strict(path: &Path) {
    let out = codesign(&["--verify", "--strict", "--verbose=2"], path);
    assert!(
        out.status.success(),
        "codesign --verify --strict {}: {}",
        path.display(),
        stderr(&out)
    );
}

/// Replace `path` with `bytes` as a new file, keeping it executable: macOS
/// remembers a signed binary's checks by file, so one edited in place may be
/// killed at its next start.
fn rewrite(path: &Path, bytes: &[u8]) {
    let mode = fs::metadata(path).expect("stat app").permissions();
    fs::remove_file(path).expect("remove app");
    fs::write(path, bytes).expect("write app");
    fs::set_permissions(path, mode).expect("chmod app");
}

/// The file offset and size of `__SPINEL,__payload`, read from the load
/// commands here rather than by the binary's own reader.
fn payload_section(file: &[u8]) -> (usize, usize) {
    let u32_at = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(file[at..at + 8].try_into().unwrap());
    assert_eq!(u32_at(0), 0xfeed_facf, "a thin 64-bit Mach-O");
    let mut at = 32;
    for _ in 0..u32_at(16) {
        let (cmd, size) = (u32_at(at), u32_at(at + 4) as usize);
        if cmd == 0x19 && file[at + 8..at + 24].starts_with(b"__SPINEL\0") {
            assert_eq!(u32_at(at + 64), 1, "one section");
            let section = at + 72;
            assert!(file[section..section + 16].starts_with(b"__payload\0"));
            return (u32_at(section + 48) as usize, u64_at(section + 40) as usize);
        }
        at += size;
    }
    panic!("no __SPINEL segment");
}

#[test]
fn a_plain_spinel_reserves_the_section_and_runs_as_before() {
    let file = fs::read(SPINEL).expect("read spinel");
    let (offset, size) = payload_section(&file);
    assert_eq!(&file[offset..offset + size], &[0; 16]);
    let out = Command::new(SPINEL)
        .args(["-e", "puts 6 * 7"])
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(out.stdout, b"42\n");
}

#[test]
fn a_packed_app_passes_codesign_verify_and_runs() {
    let scratch = Scratch::new("valid");
    let payload = payload();
    let exe = pack(&scratch.0, &payload);
    verify_strict(&exe);

    let out = Command::new(&exe)
        .env("XDG_CACHE_HOME", exe.with_file_name("cache"))
        .arg("--version")
        .output()
        .expect("the app executes");
    let err = stderr(&out);
    assert!(err.contains("is a compiled app"), "{err}");
    assert!(err.contains("100000 byte payload"), "{err}");
    assert!(err.contains(&sha256_hex(&payload)), "{err}");
    // Its payload is not a `payload.tar.zst`, so there is nothing to run.
    assert!(err.contains("cannot be extracted"), "{err}");
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn the_section_holds_the_bytes_linux_appends() {
    let scratch = Scratch::new("layout");
    let payload = payload();
    let file = fs::read(pack(&scratch.0, &payload)).expect("read app");
    let (offset, size) = payload_section(&file);
    let mut expected = payload.clone();
    expected.extend_from_slice(&trailer(&payload));
    assert_eq!(size, expected.len());
    assert!(file[offset..offset + size] == expected[..]);
    // The file itself ends in the signature, not in a trailer.
    assert_ne!(&file[file.len() - 8..], b"SPINEL01");
}

#[test]
fn a_corrupt_payload_in_the_section_is_refused() {
    // Flipped after signing, then signed again, so the binary runs and its
    // own check, not the kernel's, is what refuses it.
    let scratch = Scratch::new("corrupt");
    let payload = payload();
    let exe = pack(&scratch.0, &payload);
    let mut file = fs::read(&exe).expect("read app");
    let (offset, _) = payload_section(&file);
    file[offset + 3] ^= 0x20;
    rewrite(&exe, &file);
    let signed = codesign(&["--force", "--sign", "-"], &exe);
    assert!(signed.status.success(), "{}", stderr(&signed));
    verify_strict(&exe);

    let out = Command::new(&exe)
        .env("XDG_CACHE_HOME", exe.with_file_name("cache"))
        .output()
        .expect("the app executes");
    let err = stderr(&out);
    assert!(err.contains("payload is corrupt"), "{err}");
    assert!(
        err.contains(&format!("the trailer says {}", sha256_hex(&payload))),
        "{err}"
    );
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn a_length_longer_than_the_section_is_refused_as_truncated() {
    let scratch = Scratch::new("truncated");
    let payload = payload();
    let exe = pack(&scratch.0, &payload);
    let mut file = fs::read(&exe).expect("read app");
    let (offset, size) = payload_section(&file);
    let length = offset + size - 16;
    file[length..length + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    rewrite(&exe, &file);
    let signed = codesign(&["--force", "--sign", "-"], &exe);
    assert!(signed.status.success(), "{}", stderr(&signed));

    let out = Command::new(&exe)
        .env("XDG_CACHE_HOME", exe.with_file_name("cache"))
        .output()
        .expect("the app executes");
    let err = stderr(&out);
    assert!(err.contains("payload is truncated"), "{err}");
    // What the section holds before its trailer, not the whole file.
    assert!(
        err.contains(&format!("holds only {} before it", size - 48)),
        "{err}"
    );
    assert_eq!(out.status.code(), Some(1), "{err}");
}
