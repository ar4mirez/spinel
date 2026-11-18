//! Built-in gems at install time (#83, docs/package-manager.md, section 6,
//! step 1).
//!
//! Spinel ships some default gems inside the binary: their Ruby halves are
//! upstream's files, vendored in `stdlib/` or embedded from `lib/`, and their
//! C halves are Spinel primitives (engine.md, "Built-in gems"). When a
//! lockfile pins one of them, `spinel install` uses the built-in copy rather
//! than the gem: the gem is neither fetched nor unpacked, so its `ext/` is
//! never built and nothing compiled is written. The lockfile is left as
//! Bundler wrote it, still naming the gem's own version; only what is
//! installed changes.
//!
//! The GEM_HOME gets the locked identity, as a Spinel-native substitute's
//! would: `specifications/<full name>.gemspec` with the locked name, version
//! and platform and the lockfile's runtime dependencies, no extensions and no
//! executables, and `gems/<full name>` a symlink to a directory of Spinel's
//! own whose `lib/` is empty. `Bundler.setup` therefore finds the gem the lock
//! names and activates it, and `require` finds the built-in where it always
//! does: an embedded `lib/<name>.rb` before the load path, the standard
//! library's on the load path.
//!
//! The version Spinel ships and the version the lock pins may differ ([`skew`]):
//! a different patch version is silent, a different minor version warns, and
//! a different major version fails the install before anything is written,
//! because the gem's Ruby half may expect a different primitive surface.

use std::path::{Path, PathBuf};

use crate::gem_version::{Segment, Version};
use crate::resolver::Spec;

/// A gem Spinel ships as a built-in, and the version it ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub version: &'static str,
}

/// The built-in gems a lockfile's pin is substituted for, each at the version
/// its Ruby source claims (an integration test, `tests/builtin_gem.rs`,
/// requires each one and checks its runtime constant).
///
/// ponytail: only the built-ins whose source names a gem version are listed.
/// `monitor` (#46) declares none, `socket` and `objspace` are not gems, and
/// `bigdecimal` (#109), `psych`, `openssl` (#106) and `racc` are not built-ins
/// yet, so a lock pinning one of them installs the gem itself, as before.
/// Upgrade: add each one here with its version when it lands.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "date",
        version: "3.5.1",
    },
    Builtin {
        name: "digest",
        version: "3.2.1",
    },
    Builtin {
        name: "etc",
        version: "1.4.6",
    },
    Builtin {
        name: "fcntl",
        version: "1.3.0",
    },
    Builtin {
        name: "fiddle",
        version: "1.1.8",
    },
    Builtin {
        name: "io-console",
        version: "0.8.2",
    },
    Builtin {
        name: "json",
        version: "2.18.0",
    },
    Builtin {
        name: "stringio",
        version: "3.2.0",
    },
    Builtin {
        name: "strscan",
        version: "3.1.6",
    },
    Builtin {
        name: "zlib",
        version: "3.2.3",
    },
];

/// The built-in gem named `name`, if Spinel ships one.
pub fn find(name: &str) -> Option<Builtin> {
    BUILTINS.iter().copied().find(|b| b.name == name)
}

/// How far a locked version is from the built-in's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skew {
    /// The same major and minor version: the built-in is used, silently.
    Patch,
    /// The same major version, another minor: the built-in is used, with a
    /// warning.
    Minor,
    /// Another major version: the install fails.
    Major,
}

/// The `i`th release segment of `version`, `0` past its end, as RubyGems
/// pads a shorter version when it compares two.
fn segment(version: &Version, i: usize) -> Segment {
    version
        .canonical_segments()
        .get(i)
        .cloned()
        .unwrap_or_else(|| Segment::Number("0".to_owned()))
}

/// The skew between `locked` and the built-in's `shipped` version: their
/// first segments are the major version, their second the minor.
pub fn skew(locked: &Version, shipped: &Version) -> Skew {
    if segment(locked, 0) != segment(shipped, 0) {
        Skew::Major
    } else if segment(locked, 1) != segment(shipped, 1) {
        Skew::Minor
    } else {
        Skew::Patch
    }
}

/// The directory `gems/<full name>` leads to for a built-in: Spinel's own,
/// under its home, with an empty `lib/`, so the gem's require path is there
/// and adds nothing to what `require` finds.
pub fn gem_dir(home: &Path, builtin: Builtin) -> PathBuf {
    home.join("builtin")
        .join(format!("{}-{}", builtin.name, builtin.version))
}

/// Make [`gem_dir`] and its `lib/`.
pub fn make_gem_dir(home: &Path, builtin: Builtin) -> std::io::Result<PathBuf> {
    let dir = gem_dir(home, builtin);
    std::fs::create_dir_all(dir.join("lib"))?;
    Ok(dir)
}

/// A YAML string, single-quoted, as Psych quotes one that would otherwise
/// read as something else.
fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn requirement_yaml(out: &mut String, indent: &str, list: &[(String, String)]) {
    out.push_str(&format!(
        "{indent}!ruby/object:Gem::Requirement\n{indent}  requirements:\n"
    ));
    for (op, version) in list {
        out.push_str(&format!(
            "{indent}  - - {}\n{indent}    - !ruby/object:Gem::Version\n{indent}      version: {}\n",
            quoted(op),
            quoted(version)
        ));
    }
}

/// The gemspec, as YAML, that stands for `spec` in the GEM_HOME: its locked
/// name, version, platform and runtime dependencies, nothing to build and
/// nothing to run.
pub fn gemspec_yaml(spec: &Spec, builtin: Builtin) -> String {
    let mut out = String::from("--- !ruby/object:Gem::Specification\n");
    out.push_str(&format!("name: {}\n", quoted(&spec.name)));
    out.push_str(&format!(
        "version: !ruby/object:Gem::Version\n  version: {}\n",
        quoted(spec.version.as_str())
    ));
    out.push_str(&format!(
        "platform: {}\n",
        quoted(&spec.platform.to_string())
    ));
    out.push_str("authors: []\nbindir: bin\ncert_chain: []\n");
    out.push_str("date: 1980-01-02 00:00:00.000000000 Z\n");
    if spec.dependencies.is_empty() {
        out.push_str("dependencies: []\n");
    } else {
        out.push_str("dependencies:\n");
        for (name, requirement) in &spec.dependencies {
            let list: Vec<(String, String)> = if requirement.requirements().is_empty() {
                vec![(">=".to_owned(), "0".to_owned())]
            } else {
                requirement
                    .requirements()
                    .iter()
                    .map(|(op, v)| (op.as_str().to_owned(), v.as_str().to_owned()))
                    .collect()
            };
            out.push_str(&format!(
                "- !ruby/object:Gem::Dependency\n  name: {}\n  requirement: ",
                quoted(name)
            ));
            requirement_yaml(&mut out, "    ", &list);
            out.push_str("  type: :runtime\n  prerelease: false\n  version_requirements: ");
            requirement_yaml(&mut out, "    ", &list);
        }
    }
    out.push_str("executables: []\nextensions: []\nextra_rdoc_files: []\nfiles: []\n");
    out.push_str("licenses: []\nmetadata: {}\nrequire_paths:\n- lib\n");
    let any = [(">=".to_owned(), "0".to_owned())];
    out.push_str("required_ruby_version: ");
    requirement_yaml(&mut out, "  ", &any);
    out.push_str("required_rubygems_version: ");
    requirement_yaml(&mut out, "  ", &any);
    out.push_str("requirements: []\nspecification_version: 4\n");
    out.push_str(&format!(
        "summary: {}\ntest_files: []\n",
        quoted(&format!(
            "Spinel's built-in {} {}",
            builtin.name, builtin.version
        ))
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gem_platform::GemPlatform;
    use crate::gem_requirement::Requirement;

    fn v(s: &str) -> Version {
        Version::new(s).unwrap()
    }

    #[test]
    fn skew_is_judged_on_major_then_minor() {
        assert_eq!(skew(&v("2.18.0"), &v("2.18.0")), Skew::Patch);
        assert_eq!(skew(&v("2.18.5"), &v("2.18.0")), Skew::Patch);
        assert_eq!(skew(&v("2.18"), &v("2.18.0")), Skew::Patch);
        assert_eq!(skew(&v("2.18.0.pre"), &v("2.18.0")), Skew::Patch);
        assert_eq!(skew(&v("2.19.0"), &v("2.18.0")), Skew::Minor);
        assert_eq!(skew(&v("2.17.9"), &v("2.18.0")), Skew::Minor);
        assert_eq!(skew(&v("2"), &v("2.18.0")), Skew::Minor);
        assert_eq!(skew(&v("3.0.0"), &v("2.18.0")), Skew::Major);
        assert_eq!(skew(&v("1.18.0"), &v("2.18.0")), Skew::Major);
    }

    #[test]
    fn only_the_listed_gems_are_built_ins() {
        assert_eq!(find("json").map(|b| b.version), Some("2.18.0"));
        assert_eq!(find("bigdecimal"), None);
    }

    #[test]
    fn the_gemspec_is_the_locked_identity_with_nothing_to_build() {
        let spec = Spec {
            name: "json".to_owned(),
            version: v("2.19.1"),
            platform: GemPlatform::Ruby,
            dependencies: vec![(
                "base64".to_owned(),
                Requirement::new(["~> 0.2", "!= 0.2.1"]).unwrap(),
            )],
            required_ruby_version: Requirement::default(),
            required_rubygems_version: Requirement::default(),
            checksum: None,
        };
        let yaml = gemspec_yaml(&spec, find("json").unwrap());
        let loaded = crate::gem_spec::from_yaml(yaml.as_bytes()).unwrap();
        loaded.verify().unwrap();
        assert_eq!(loaded.full_name().unwrap(), "json-2.19.1");
        assert_eq!(loaded.require_paths().unwrap(), ["lib"]);
        assert!(loaded.extensions().unwrap().is_empty());
        assert!(loaded.executables().unwrap().is_empty());
        let ruby = loaded.to_ruby_for_cache().unwrap();
        assert!(ruby.contains("# stub: json 2.19.1 ruby lib\n"), "{ruby}");
        assert!(
            ruby.contains(
                "s.add_runtime_dependency(%q<base64>.freeze, [\"~> 0.2\".freeze, \"!= 0.2.1\".freeze])"
            ),
            "{ruby}"
        );
    }
}
