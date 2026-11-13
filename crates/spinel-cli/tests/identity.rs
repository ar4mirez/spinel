//! How Spinel identifies itself to Ruby code (#65): `RUBY_ENGINE`, `RUBY_VERSION`,
//! `RUBY_PLATFORM` and the rest, with the values `docs/cli.md` sets, read through the
//! built `spinel` binary the way a program or a gem would read them.

use std::collections::HashMap;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// One identity constant as Ruby sees it.
#[derive(Debug)]
struct Constant {
    class: String,
    frozen: bool,
    inspect: String,
}

/// Every `RUBY_*` identity constant, keyed by name, plus whether `Ruby::NAME` (Ruby
/// 4.0's home for the same values) holds the very same object.
fn constants() -> HashMap<String, (Constant, bool)> {
    let script = r#"
        %w[ENGINE ENGINE_VERSION VERSION PLATFORM PATCHLEVEL RELEASE_DATE COPYRIGHT
           DESCRIPTION REVISION].each do |name|
          value = Object.const_get(:"RUBY_#{name}")
          same = Ruby.const_get(name).equal?(value)
          puts [name, value.class, value.frozen?, same, value.inspect].join("\t")
        end
    "#;
    let out = Command::new(SPINEL)
        .args(["-e", script])
        .output()
        .expect("spinel runs");
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    assert!(
        out.status.success(),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.splitn(5, '\t').collect();
            let [name, class, frozen, same, inspect] = fields[..] else {
                panic!("unexpected line: {line:?}")
            };
            let constant = Constant {
                class: class.to_owned(),
                frozen: frozen == "true",
                inspect: inspect.to_owned(),
            };
            (name.to_owned(), (constant, same == "true"))
        })
        .collect()
}

/// A frozen String constant's value, unquoted.
fn string<'a>(constants: &'a HashMap<String, (Constant, bool)>, name: &str) -> &'a str {
    let (constant, _) = &constants[name];
    assert_eq!(constant.class, "String", "RUBY_{name}: {constant:?}");
    assert!(constant.frozen, "RUBY_{name} is frozen, as in CRuby");
    constant
        .inspect
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or_else(|| panic!("RUBY_{name}: {constant:?}"))
}

/// What CRuby, and so RubyGems' `Gem::Platform.local`, calls this host.
fn host_platform() -> Option<String> {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "linux" if cfg!(target_env = "musl") => Some(format!("{arch}-linux-musl")),
        "linux" => Some(format!("{arch}-linux")),
        "macos" => {
            let cpu = if arch == "aarch64" { "arm64" } else { arch };
            let out = Command::new("uname").arg("-r").output().ok()?;
            let release = String::from_utf8(out.stdout).ok()?;
            let major = release.trim().split('.').next()?;
            Some(format!("{cpu}-darwin{major}"))
        }
        _ => None,
    }
}

#[test]
fn identity_constants_hold_the_values_in_cli_md() {
    let constants = constants();
    assert_eq!(constants.len(), 9, "{constants:?}");
    for (name, (_, same)) in &constants {
        assert!(same, "Ruby::{name} is the object RUBY_{name} holds");
    }

    assert_eq!(string(&constants, "ENGINE"), "spinel");
    assert_eq!(
        string(&constants, "ENGINE_VERSION"),
        env!("CARGO_PKG_VERSION")
    );
    // The language version ruby/spec's `ruby_version_is` guards read.
    assert_eq!(string(&constants, "VERSION"), "4.0.0");

    let (patchlevel, _) = &constants["PATCHLEVEL"];
    assert_eq!(patchlevel.class, "Integer", "{patchlevel:?}");
    assert_eq!(patchlevel.inspect, "-1");

    let date = string(&constants, "RELEASE_DATE");
    let parts: Vec<&str> = date.split('-').collect();
    assert!(
        parts.iter().map(|p| p.len()).eq([4, 2, 2])
            && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())),
        "RUBY_RELEASE_DATE is YYYY-MM-DD: {date}"
    );
    assert!(
        string(&constants, "COPYRIGHT").starts_with("spinel - Copyright (C) "),
        "Spinel's copyright, not CRuby's"
    );
    assert!(!string(&constants, "REVISION").is_empty());
}

#[test]
fn ruby_platform_is_spelled_as_rubygems_spells_the_host() {
    let constants = constants();
    let platform = string(&constants, "PLATFORM");
    if let Some(host) = host_platform() {
        assert_eq!(platform, host);
    }
    let (cpu, os) = platform.split_once('-').expect("cpu-os");
    assert!(!cpu.is_empty() && !os.is_empty(), "{platform}");

    // `Gem::Platform.local` is `Gem::Platform.new(RbConfig::CONFIG["arch"])`.
    let out = Command::new(SPINEL)
        .args([
            "-e",
            r#"print RbConfig::CONFIG.values_at("arch", "host_cpu", "host_os").join(" ")"#,
        ])
        .output()
        .expect("spinel runs");
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf-8"),
        format!("{platform} {cpu} {os}")
    );
}

#[test]
fn ruby_description_is_the_version_banner() {
    let constants = constants();
    let description = string(&constants, "DESCRIPTION");
    let expected = format!(
        "spinel {} (ruby {}) [{}]",
        string(&constants, "ENGINE_VERSION"),
        string(&constants, "VERSION"),
        string(&constants, "PLATFORM"),
    );
    assert_eq!(description, expected);

    let out = Command::new(SPINEL)
        .arg("--version")
        .output()
        .expect("spinel runs");
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf-8").trim_end(),
        description
    );
}
