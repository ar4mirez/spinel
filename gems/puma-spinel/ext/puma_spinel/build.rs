//! What `mini_ssl.c` takes from the OpenSSL headers it is compiled against:
//! the version text (`OPENSSL_VERSION_TEXT`) and which protocols the build
//! left out (`OPENSSL_NO_SSL3`, `OPENSSL_NO_TLS1`, `OPENSSL_NO_TLS1_1` and
//! their `_METHOD`s), read from the headers openssl-sys found
//! (`DEP_OPENSSL_INCLUDE`) as the C preprocessor reads them, for
//! `src/minissl.rs`'s constants.

use std::path::PathBuf;

fn main() {
    let include = std::env::var_os("DEP_OPENSSL_INCLUDE")
        .map(PathBuf::from)
        .expect("openssl-sys names the headers it found");
    let header = |name: &str| {
        let path = include.join("openssl").join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    };

    let opensslv = header("opensslv.h");
    let text = opensslv
        .lines()
        .filter_map(|line| define(line, "OPENSSL_VERSION_TEXT"))
        .find_map(|value| {
            value
                .strip_prefix('"')?
                .strip_suffix('"')
                .map(str::to_owned)
        })
        .expect("opensslv.h defines OPENSSL_VERSION_TEXT as a string");
    println!("cargo:rustc-env=PUMA_SPINEL_OPENSSL_VERSION_TEXT={text}");

    let configuration = header("configuration.h");
    let defined = |name: &str| {
        configuration
            .lines()
            .any(|line| define(line, name).is_some())
    };
    for (env, names) in [
        ("SSL3", ["OPENSSL_NO_SSL3", "OPENSSL_NO_SSL3_METHOD"]),
        ("TLS1", ["OPENSSL_NO_TLS1", "OPENSSL_NO_TLS1_METHOD"]),
        ("TLS1_1", ["OPENSSL_NO_TLS1_1", "OPENSSL_NO_TLS1_1_METHOD"]),
    ] {
        let off = names.iter().any(|name| defined(name));
        println!(
            "cargo:rustc-env=PUMA_SPINEL_OPENSSL_NO_{env}={}",
            u8::from(off)
        );
    }
    println!("cargo:rerun-if-env-changed=DEP_OPENSSL_INCLUDE");
}

/// The value of `#define NAME value` on this line, "" for none; `None` when
/// the line defines something else.
fn define<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("define")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(name)?;
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '(') {
        return None;
    }
    Some(rest.trim())
}
