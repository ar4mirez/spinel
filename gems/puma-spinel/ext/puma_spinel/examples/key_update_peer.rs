//! The OpenSSL client Puma's `TestPumaServerSSLKeyUpdate` compiles from C
//! with `RbConfig::CONFIG["CC"]` (`test/test_puma_server_ssl_key_update.rb`'s
//! `PEER_SOURCE`), in Rust: Spinel builds no C and names no `CC` (#867), so
//! `crates/spinel-cli/tests/fixtures/puma/key_update_peer.rb` hands the suite
//! this one instead, on Spinel and on the CRuby run that recorded
//! `suite.txt` alike.
//!
//! ```text
//! key_update_peer HOST PORT REQUEST REQUESTED
//! ```
//!
//! It makes the C's calls in the C's order, on the same system OpenSSL 3:
//! connect to `HOST`:`PORT`, a TLS 1.3 handshake, `REQUEST` written as it
//! is, then a TLS 1.3 KeyUpdate after KeyUpdate, asking the server to
//! update its own keys too when `REQUESTED` is not 0, until a handshake
//! write fails because the server closed the connection. The exit status is
//! the C's: 0 once the server closed it, 2 for an address that is not IPv4,
//! 3 when the connection is refused, 4 when the handshake fails, 5 when the
//! request is not written whole.

use std::ffi::c_int;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::os::fd::AsRawFd;
use std::process::ExitCode;

use openssl_sys as sys;

// `SSL_key_update`'s update types, from `ssl.h`.
const SSL_KEY_UPDATE_NOT_REQUESTED: c_int = 0;
const SSL_KEY_UPDATE_REQUESTED: c_int = 1;

// What openssl-sys leaves out.
unsafe extern "C" {
    fn SSL_set_fd(s: *mut sys::SSL, fd: c_int) -> c_int;
    fn SSL_key_update(s: *mut sys::SSL, updatetype: c_int) -> c_int;
}

/// C's `atoi`: an optional sign and the leading digits, 0 for none.
fn atoi(text: &str) -> i64 {
    let text = text.trim_start();
    let (sign, rest) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit);
    sign * digits.fold(0i64, |n, d| {
        n.wrapping_mul(10).wrapping_add(i64::from(d - b'0'))
    })
}

fn main() -> ExitCode {
    // Rust's runtime ignores SIGPIPE already, as the C's first call asks.
    let args: Vec<String> = std::env::args().collect();
    let [_, host, port, request, requested] = &args[..] else {
        eprintln!("usage: key_update_peer HOST PORT REQUEST REQUESTED");
        return ExitCode::from(1);
    };
    let Ok(ip) = host.parse::<Ipv4Addr>() else {
        return ExitCode::from(2);
    };
    // `htons(atoi(port))`: the low 16 bits.
    let port = atoi(port) as u16;
    let Ok(stream) = TcpStream::connect(SocketAddrV4::new(ip, port)) else {
        return ExitCode::from(3);
    };
    let update = if atoi(requested) != 0 {
        SSL_KEY_UPDATE_REQUESTED
    } else {
        SSL_KEY_UPDATE_NOT_REQUESTED
    };
    // SAFETY: OpenSSL's own calls on objects made here, over the stream's
    // descriptor, which outlives them; the process exits after, as the
    // C's does, without freeing them.
    unsafe {
        let ctx = sys::SSL_CTX_new(sys::TLS_client_method());
        sys::SSL_CTX_set_min_proto_version(ctx, sys::TLS1_3_VERSION);
        let ssl = sys::SSL_new(ctx);
        SSL_set_fd(ssl, stream.as_raw_fd());
        if sys::SSL_connect(ssl) != 1 {
            return ExitCode::from(4);
        }
        let len = request.len() as c_int;
        if sys::SSL_write(ssl, request.as_ptr().cast(), len) != len {
            return ExitCode::from(5);
        }
        while SSL_key_update(ssl, update) == 1 && sys::SSL_do_handshake(ssl) == 1 {}
    }
    ExitCode::SUCCESS
}
