//! The built-in `openssl` (#106), phase 1: ruby/openssl 4.0.2's `lib/`
//! vendored in `stdlib/`, over its C half in Ruby (`lib/openssl/spinel.rb`)
//! over the system's OpenSSL 3 (`crates/spinel-vm/src/openssl.rs`).
//!
//! ruby/spec's `library/openssl` checks the API with a handful of inputs.
//! `fixtures/openssl/openssl.rb` checks the bytes against the vectors their
//! standards publish — SHA-1 and SHA-2 (FIPS 180's examples), MD5 (RFC 1321),
//! HMAC-SHA256 (RFC 4231's seven test cases), HMAC-SHA1 and HMAC-MD5 (RFC
//! 2202) — and walks `OpenSSL::Digest`, `OpenSSL::HMAC`, `OpenSSL::Random`
//! and the errors, and its output must be what ruby 4.0.9 prints, which is
//! `fixtures/openssl/openssl.out` (regenerate it with
//! `ruby fixtures/openssl/openssl.rb > fixtures/openssl/openssl.out`). Nothing
//! it prints depends on the OpenSSL release, so it holds on every CI leg.
//! Everything runs offline.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openssl")
}

fn spinel(script: &str) -> String {
    let output = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn openssl_matches_the_published_vectors_and_ruby() {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join("openssl.rb"))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8_lossy(&output.stdout);
    // The vectors are checked inside the fixture against their standards,
    // so a wrong byte reads MISMATCH whatever Ruby generated the `.out`.
    let mismatches: Vec<&str> = actual
        .lines()
        .filter(|line| line.starts_with("MISMATCH"))
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    let expected =
        std::fs::read_to_string(fixtures().join("openssl.out")).expect("openssl.out is readable");
    if actual != expected {
        let first = expected
            .lines()
            .zip(actual.lines())
            .position(|(e, a)| e != a)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "output differs from ruby 4.0.9's at line {}:\n  ruby:   {:?}\n  spinel: {:?}",
            first + 1,
            expected.lines().nth(first),
            actual.lines().nth(first)
        );
    }
}

/// The issue's own vectors, independent of the fixture: SHA-256, SHA-1 and
/// MD5 of "abc", HMAC-SHA256 from RFC 4231 test case 2, and random bytes of
/// the length asked, BINARY Strings.
#[test]
fn known_answers_and_random_bytes() {
    let out = spinel(
        r#"require "openssl"
puts OpenSSL::Digest::SHA256.hexdigest("abc")
puts OpenSSL::Digest.hexdigest("SHA1", "abc")
puts OpenSSL::Digest::MD5.new.update("a").update("bc").hexdigest
puts OpenSSL::HMAC.hexdigest("SHA256", "Jefe", "what do ya want for nothing?")
r = OpenSSL::Random.random_bytes(32)
p [r.class, r.bytesize, r.encoding, r == OpenSSL::Random.random_bytes(32)]"#,
    );
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "a9993e364706816aba3e25717850c26c9cd0d89d",
            "900150983cd24fb0d6963f7d28e17f72",
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            "[String, 32, #<Encoding:BINARY (ASCII-8BIT)>, false]",
        ],
        "{out}"
    );
}

/// A digest's or HMAC's OpenSSL context is freed with the object: 25,000
/// rounds of four contexts each, copied and finished, leave the process no
/// bigger than the first 5,000 rounds did. With a finalizer that frees
/// nothing, the same rounds grow it by some 90 MB.
#[cfg(target_os = "linux")]
#[test]
fn contexts_are_freed_with_their_objects() {
    let out = spinel(
        r#"require "openssl"
def rss = File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
def round(n)
  n.times do
    OpenSSL::Digest.new("sha256", "x").dup.digest
    h = OpenSSL::HMAC.new("key", "sha256")
    h.dup.update("y").digest
  end
  GC.start
end
round(5_000)
before = rss
round(25_000)
grown = rss - before
puts grown < 16_384 ? "bounded" : "grew #{grown} KB""#,
    );
    assert_eq!(out, "bounded\n");
}

/// What the phases so far do not have is not there: `require "openssl"`
/// loads, phase 2's keys, certificates, TLS, big numbers and ASN.1 are
/// defined (#107, `tests/openssl_tls.rs`), and the classes of later phases
/// are undefined rather than stood in for.
#[test]
fn later_phases_are_absent_not_faked() {
    let out = spinel(
        r#"require "openssl"
p %i[Cipher PKey X509 SSL BN KDF ASN1].map { |c| OpenSSL.const_defined?(c, false) }
p 1.respond_to?(:to_bn)"#,
    );
    assert_eq!(out, "[false, true, true, true, true, false, true]\ntrue\n");
}
