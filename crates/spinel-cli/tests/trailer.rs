//! A compiled app is the `spinel` binary with a payload and a 48-byte trailer
//! appended (#98, docs/build.md "Format"). On Linux that is a plain append the
//! loader ignores: the result still executes, finds its trailer, and refuses a
//! payload whose sha256 does not match.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// What the trailer's sha256 field is checked against in the error text.
const PAYLOAD: &[u8] = b"a payload stands in for app.tar.zst until spinel build (#100)";

/// A fresh empty directory for one test, removed again when it drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("spinel-trailer-{name}-{}", std::process::id()));
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

/// The trailer exactly as docs/build.md lays it out, built here independently
/// of the binary's own writer: sha256, length as u64 little-endian, magic.
fn trailer(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Sha256::digest(payload).to_vec();
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(b"SPINEL01");
    bytes
}

/// A copy of the built `spinel` with `tail` appended, executable.
fn compiled(dir: &Path, tail: &[u8]) -> PathBuf {
    let out = dir.join("mytool");
    let mut bytes = fs::read(SPINEL).expect("read spinel");
    bytes.extend_from_slice(tail);
    fs::write(&out, bytes).expect("write app");
    let mode = fs::metadata(SPINEL).expect("stat spinel").permissions();
    fs::set_permissions(&out, mode).expect("chmod app");
    out
}

/// Lowercase hex, as `sha256sum` prints a digest.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn app_bytes(payload: &[u8]) -> Vec<u8> {
    let mut tail = payload.to_vec();
    tail.extend_from_slice(&trailer(payload));
    tail
}

fn run(exe: &Path, args: &[&str]) -> Output {
    Command::new(exe)
        .args(args)
        .output()
        .expect("the app executes")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_binary_with_a_valid_trailer_executes_and_reaches_the_compiled_app_path() {
    let scratch = Scratch::new("valid");
    let exe = compiled(&scratch.0, &app_bytes(PAYLOAD));
    let out = run(&exe, &["--version"]);
    let err = stderr(&out);
    let sha = sha256_hex(PAYLOAD);
    assert!(err.contains("is a compiled app"), "{err}");
    assert!(
        err.contains(&format!("{} byte payload", PAYLOAD.len())),
        "{err}"
    );
    assert!(err.contains(&sha), "{err}");
    assert!(err.contains("not built yet"), "{err}");
    // The app's command line is the app's: `--version` is not Spinel's banner.
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn a_flipped_payload_byte_is_reported_as_corrupt() {
    let scratch = Scratch::new("flipped");
    let mut tail = app_bytes(PAYLOAD);
    tail[3] ^= 0x20;
    let exe = compiled(&scratch.0, &tail);
    let out = run(&exe, &[]);
    let err = stderr(&out);
    assert!(err.contains("payload is corrupt"), "{err}");
    assert!(
        err.contains(&format!("the trailer says {}", sha256_hex(PAYLOAD))),
        "{err}"
    );
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn a_payload_missing_bytes_is_reported_as_corrupt() {
    // A payload cut short ahead of an intact trailer: the declared length now
    // reaches back into the binary, so the bytes hashed are not the payload.
    let scratch = Scratch::new("cut");
    let mut tail = app_bytes(PAYLOAD);
    tail.drain(10..20);
    let exe = compiled(&scratch.0, &tail);
    let out = run(&exe, &[]);
    let err = stderr(&out);
    assert!(err.contains("payload is corrupt"), "{err}");
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn a_length_longer_than_the_file_is_reported_as_truncated() {
    let scratch = Scratch::new("truncated");
    let mut tail = trailer(PAYLOAD);
    tail[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    let exe = compiled(&scratch.0, &tail);
    let out = run(&exe, &[]);
    let err = stderr(&out);
    assert!(err.contains("payload is truncated"), "{err}");
    assert!(
        err.contains(&format!("declares {} bytes", u64::MAX)),
        "{err}"
    );
    assert_eq!(out.status.code(), Some(1), "{err}");
}

#[test]
fn a_plain_spinel_runs_as_before() {
    let out = Command::new(SPINEL)
        .args(["-e", "puts 6 * 7"])
        .output()
        .expect("runs");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(out.stdout, b"42\n");
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
}

#[test]
fn bytes_appended_without_the_magic_leave_spinel_unchanged() {
    // Only the magic makes an app: junk past the ELF is ignored by the loader
    // and by Spinel alike.
    let scratch = Scratch::new("junk");
    let exe = compiled(&scratch.0, &[0u8; 100]);
    let out = run(&exe, &["-e", "puts 6 * 7"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(out.stdout, b"42\n");
}
