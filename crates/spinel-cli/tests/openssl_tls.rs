//! The built-in `openssl`, phase 2 (#107): keys, certificates, stores and
//! TLS, ruby/openssl 4.0.2's `lib/` over its C half in Ruby
//! (`lib/openssl/spinel/*.rb`) over the system's OpenSSL 3
//! (`crates/spinel-vm/src/openssl/tls.rs`).
//!
//! `fixtures/openssl_tls/tls.rb` makes an RSA CA and a server certificate
//! signed by it, for `localhost` and `127.0.0.1`, and with them runs TLS
//! servers on 127.0.0.1 in threads: a handshake and a round trip, a
//! certificate no store trusts, a host name the certificate does not name,
//! `verify_callback` seeing, accepting and rejecting, `connect_nonblock`,
//! what nio4r 2.7.5's SSLSocket spec does on TLS 1.2 and 1.3, and
//! `net/https` against the CA file and without it. Its output must be what
//! ruby 4.0.9 prints, `fixtures/openssl_tls/tls.out` (regenerate it with
//! `ruby fixtures/openssl_tls/tls.rb > fixtures/openssl_tls/tls.out`).
//! Nothing it prints depends on the keys, the clock or the port, and it
//! never leaves 127.0.0.1.
//!
//! `net_https_fetches_a_real_page` is the one test that reaches the
//! internet, and only where `SPINEL_TLS_NET_TEST_URL` names a page (CI sets
//! it); elsewhere it says so and passes.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openssl_tls")
}

fn spinel(script: &str) -> String {
    let output = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn tls_over_loopback_matches_ruby() {
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(fixtures().join("tls.rb"))
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "spinel failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8_lossy(&output.stdout);
    let expected =
        std::fs::read_to_string(fixtures().join("tls.out")).expect("tls.out is readable");
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

/// Verification is on by default exactly where CRuby has it on:
/// `set_params` asks for the peer's certificate and its host name, against
/// the system's trust store; a bare context asks for neither.
#[test]
fn verification_defaults_are_cruby_s() {
    let out = spinel(
        r#"require "openssl"
c = OpenSSL::SSL::SSLContext.new
p [c.verify_mode, c.verify_hostname]
c.set_params
p [c.verify_mode == OpenSSL::SSL::VERIFY_PEER, c.verify_hostname, c.cert_store.equal?(OpenSSL::SSL::SSLContext::DEFAULT_CERT_STORE)]
require "net/http"
http = Net::HTTP.new("example.com", 443)
http.use_ssl = true
p http.verify_mode"#,
    );
    assert_eq!(out, "[0, false]\n[true, true, true]\nnil\n");
}

/// A certificate, key, store and connection are freed with their objects:
/// 3,000 rounds of a key read, a certificate signed and verified against a
/// store, and an SSL socket made, leave the process no bigger than the
/// first 500 rounds did.
#[cfg(target_os = "linux")]
#[test]
fn objects_are_freed_with_their_objects() {
    let out = spinel(
        r#"require "openssl"
def rss = File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
KEY = OpenSSL::PKey::RSA.new(2048)
PEM = KEY.to_pem
def round(n)
  n.times do
    key = OpenSSL::PKey.read(PEM)
    cert = OpenSSL::X509::Certificate.new
    cert.version = 2
    cert.serial = 1
    cert.subject = cert.issuer = OpenSSL::X509::Name.parse("/CN=x")
    cert.public_key = key
    cert.not_before = Time.now
    cert.not_after = Time.now + 60
    cert.sign(key, "SHA256")
    store = OpenSSL::X509::Store.new
    store.add_cert(cert)
    store.verify(cert)
    r, w = IO.pipe
    OpenSSL::SSL::SSLSocket.new(r, OpenSSL::SSL::SSLContext.new)
    r.close
    w.close
  end
  GC.start
end
round(500)
before = rss
round(3_000)
grown = rss - before
puts grown < 16_384 ? "bounded" : "grew #{grown} KB""#,
    );
    assert_eq!(out, "bounded\n");
}

/// The issue's own check: `net/https` fetches a real page. It is the one
/// test that leaves the machine, so it runs only where
/// `SPINEL_TLS_NET_TEST_URL` names an HTTPS page (CI sets it), with
/// verification as `Net::HTTP` sets it up: the peer's certificate against
/// the system's trust store, and its host name.
#[test]
fn net_https_fetches_a_real_page() {
    let Ok(url) = std::env::var("SPINEL_TLS_NET_TEST_URL") else {
        eprintln!("skipped: set SPINEL_TLS_NET_TEST_URL to an https:// page to fetch it");
        return;
    };
    let out = spinel(&format!(
        r#"require "net/https"
uri = URI({url:?})
http = Net::HTTP.new(uri.host, uri.port)
http.use_ssl = true
http.verify_mode = OpenSSL::SSL::VERIFY_PEER
verified = nil
res = http.start do |h|
  verified = h.instance_variable_get(:@socket).io.verify_result
  h.get(uri.request_uri)
end
p [res.code, res.body.bytesize > 0, verified]"#
    ));
    assert_eq!(out, "[\"200\", true, 0]\n");
}
