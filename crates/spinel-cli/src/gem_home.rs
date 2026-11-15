//! A project's GEM_HOME, linked from the global store (#82,
//! docs/package-manager.md, "Global store").
//!
//! `.spinel/gem_home/` is laid out as `gem install --install-dir` lays one
//! out, so `Gem::Specification`, `Bundler.setup` and every gem-aware library
//! read it unchanged:
//!
//! ```text
//! build_info/ cache/ doc/ extensions/ plugins/   Gem.ensure_gem_subdirectories
//! gems/<full name>               a symlink to the store entry's gem/
//! specifications/<full name>.gemspec
//!                                a real file: Gem::Installer#write_spec's text
//! bin/<executable>               Gem::Installer#app_script_text, run by spinel
//! ```
//!
//! Linking is idempotent: what is already right is left alone (a warm
//! install writes nothing), and what a previous link made for a gem no
//! longer asked for is removed. Nothing it did not make is touched. Every
//! file and symlink is made beside its place and renamed into it, so a
//! reader never sees half of one. Every gem's gemspec is read before
//! anything is written, so a gem RubyGems would refuse leaves the GEM_HOME
//! as it was.
//!
//! Two differences from `gem install`, both because the store already holds
//! what they would: `cache/` is left empty, since the `.gem` is not kept once
//! unpacked, and a gem's own files are shared through the symlink rather than
//! copied. And extensions are not built (#83 substitutes the built-in gems,
//! phase 5 the native ones): a gem that has some is linked with a warning,
//! and RubyGems will say its extensions are not built.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gem_spec;
use crate::gem_store::{self, GemId};

/// `Gem::REPOSITORY_SUBDIRECTORIES`, which `gem install` makes in every
/// GEM_HOME, and `bin/`, where it puts the executables.
const SUBDIRECTORIES: [&str; 8] = [
    "bin",
    "build_info",
    "cache",
    "doc",
    "extensions",
    "gems",
    "plugins",
    "specifications",
];

/// Names a file or symlink is made under before it is renamed into place.
const TMP_PREFIX: &str = ".spinel-tmp-";

#[derive(Debug)]
pub enum Error {
    /// The stored gemspec is one RubyGems would refuse to load.
    Spec {
        gem: String,
        message: String,
    },
    /// The store entry holds a different gem than the one asked for.
    Mismatch {
        gem: String,
        metadata: String,
    },
    /// Something not made by a link is where a link would go.
    Occupied(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spec { gem, message } => write!(f, "{gem}: its gemspec: {message}"),
            Error::Mismatch { gem, metadata } => {
                write!(f, "{gem}: the stored gemspec is for {metadata}")
            }
            Error::Occupied(path) => write!(
                f,
                "{}: already there, and not a link into the store",
                path.display()
            ),
            Error::Io(path, e) => write!(f, "{}: {e}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

/// What linking one gem did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Something was written: the link, the gemspec or a bin stub.
    Linked,
    /// Everything was already in place.
    Current,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Outcome::Linked => "linked",
            Outcome::Current => "current",
        })
    }
}

#[derive(Debug, Default)]
pub struct Report {
    /// Each gem asked for, by full name, in order.
    pub linked: Vec<(String, Outcome)>,
    /// Gems a previous link made that were not asked for this time.
    pub removed: Vec<String>,
    /// What RubyGems would warn about, and what Spinel does not do yet.
    pub warnings: Vec<String>,
}

/// One gem ready to link: everything read and generated, nothing written.
struct Plan {
    full_name: String,
    gem_dir: PathBuf,
    gemspec: String,
    stubs: Vec<(String, String)>,
}

/// Link each stored gem, given with its store entry, into `home`, and remove
/// what an earlier link made for gems not given. `store_gems` is the store's
/// `gems/` directory; a symlink into it, or into another Spinel home's store,
/// counts as made by a link.
pub fn link(home: &Path, store_gems: &Path, gems: &[(GemId, PathBuf)]) -> Result<Report, Error> {
    let mut report = Report::default();
    let mut plans = Vec::new();
    for (id, entry) in gems {
        let spec = read_spec(id, entry)?;
        let gem = id.full_name();
        let spec_error = |message| Error::Spec {
            gem: gem.clone(),
            message,
        };
        spec.verify()
            .map_err(|e| spec_error(format!("RubyGems would refuse it: {e}")))?;
        let full_name = spec.full_name().map_err(spec_error)?;
        if full_name != gem {
            return Err(Error::Mismatch {
                gem,
                metadata: full_name,
            });
        }
        let gemspec = spec.to_ruby_for_cache().map_err(spec_error)?;
        let gem_dir = entry.join("gem");
        let name = spec.name().map_err(spec_error)?;
        let bindir = spec.bindir().map_err(spec_error)?;
        let mut stubs = Vec::new();
        for exe in spec.executables().map_err(spec_error)? {
            let bin_path = match &bindir {
                Some(dir) => gem_dir.join(dir).join(&exe),
                None => gem_dir.join(&exe),
            };
            // `Gem::Installer#generate_bin` skips an executable the gem does
            // not have.
            if !bin_path.exists() || !inside(&bin_path, &gem_dir) {
                continue;
            }
            make_executable(&bin_path)?;
            stubs.push((exe.clone(), app_script_text(&name, &exe)));
        }
        if !spec.extensions().map_err(spec_error)?.is_empty() {
            report.warnings.push(format!(
                "{full_name} has extensions, which Spinel does not build: RubyGems will not activate it"
            ));
        }
        plans.push(Plan {
            full_name,
            gem_dir,
            gemspec,
            stubs,
        });
    }

    for sub in SUBDIRECTORIES {
        let dir = home.join(sub);
        fs::create_dir_all(&dir).map_err(|e| Error::Io(dir.clone(), e))?;
        sweep(&dir);
    }
    let mut wanted: HashSet<String> = HashSet::new();
    let mut stub_owner: HashMap<String, String> = HashMap::new();
    for plan in &plans {
        let mut changed = false;
        let link = home.join("gems").join(&plan.full_name);
        changed |= place_symlink(&link, &plan.gem_dir, store_gems)?;
        let gemspec = home
            .join("specifications")
            .join(format!("{}.gemspec", plan.full_name));
        changed |= place_file(&gemspec, plan.gemspec.as_bytes(), None)?;
        for (exe, text) in &plan.stubs {
            if let Some(other) = stub_owner.insert(exe.clone(), plan.full_name.clone())
                && other != plan.full_name
            {
                report.warnings.push(format!(
                    "bin/{exe}: {} and {other} both have it; {} wins",
                    plan.full_name, plan.full_name
                ));
            }
            changed |= place_file(&home.join("bin").join(exe), text.as_bytes(), Some(0o755))?;
        }
        wanted.insert(plan.full_name.clone());
        let outcome = if changed {
            Outcome::Linked
        } else {
            Outcome::Current
        };
        report.linked.push((plan.full_name.clone(), outcome));
    }
    let stubs: HashMap<&str, &str> = plans
        .iter()
        .flat_map(|p| p.stubs.iter().map(|(e, t)| (e.as_str(), t.as_str())))
        .collect();
    report.removed = prune(home, store_gems, &wanted, &stubs)?;
    Ok(report)
}

fn read_spec(id: &GemId, entry: &Path) -> Result<gem_spec::Spec, Error> {
    let path = entry.join("metadata");
    let yaml = fs::read(&path).map_err(|e| Error::Io(path, e))?;
    gem_spec::from_yaml(&yaml).map_err(|message| Error::Spec {
        gem: id.full_name(),
        message,
    })
}

/// Whether `path` leads, through any symlinks, to somewhere inside `dir`.
fn inside(path: &Path, dir: &Path) -> bool {
    match (fs::canonicalize(path), fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(&d) && p != d,
        _ => false,
    }
}

/// `Gem::Installer#generate_bin` makes each executable executable: its mode
/// gains `0o111`. The file is the store's, and the change is the same for
/// every project that links the gem.
fn make_executable(path: &Path) -> Result<(), Error> {
    let meta = fs::metadata(path).map_err(|e| Error::Io(path.to_owned(), e))?;
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o111 != 0o111 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o111))
            .map_err(|e| Error::Io(path.to_owned(), e))?;
    }
    Ok(())
}

/// The bin stub `Gem::Installer#app_script_text` writes for `exe` of the gem
/// `name`, with `--env-shebang`'s first line naming `spinel`, which is
/// `RbConfig::CONFIG["ruby_install_name"]` on Spinel (docs/cli.md).
pub fn app_script_text(name: &str, exe: &str) -> String {
    let escaped: String = exe
        .chars()
        .flat_map(|c| match c {
            '\\' | '\'' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    let gemdeps_load = if name == "bundler" {
        ""
    } else {
        "\nGem.use_gemdeps\n"
    };
    let explicit_version_requirement = if name == "bundler" {
        "version = str\nENV['BUNDLER_VERSION'] = str\n"
    } else {
        "version = str"
    };
    format!(
        "#!/usr/bin/env spinel
#
# This file was generated by RubyGems.
#
# The application '{name}' is installed as part of a gem, and
# this file is here to facilitate running it.
#

require 'rubygems'
{gemdeps_load}
version = \">= 0.a\"

str = ARGV.first
if str
  str = str.b[/\\A_(.*)_\\z/, 1]
  if str and Gem::Version.correct?(str)
    {explicit_version_requirement}
    ARGV.shift
  end
end

if Gem.respond_to?(:activate_and_load_bin_path)
  Gem.activate_and_load_bin_path('{name}', '{escaped}', version)
else
  load Gem.activate_bin_path('{name}', '{escaped}', version)
end
"
    )
}

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A name beside `path` to make its replacement under.
fn tmp_beside(path: &Path) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{TMP_PREFIX}{}-{n}-{name}", std::process::id()))
}

/// Whether a symlink in `gems/` leads where a link makes them: into this
/// store's `gems/`, or to `<anything>/store/gems/<entry>/gem`, an entry of
/// the store under another Spinel home, as when `SPINEL_HOME` changed.
fn made_by_link(target: &Path, store_gems: &Path) -> bool {
    if target.starts_with(store_gems) {
        return true;
    }
    let mut up = target.components().rev().map(|c| c.as_os_str());
    target.is_absolute()
        && up.next().is_some_and(|c| c == "gem")
        && up.next().is_some()
        && up.next().is_some_and(|c| c == "gems")
        && up.next().is_some_and(|c| c == "store")
}

/// Make `link` a symlink to `target`, unless it already is one. Whatever is
/// there must be a symlink: a link replaces only what a link made.
fn place_symlink(link: &Path, target: &Path, store_gems: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let current = fs::read_link(link).map_err(|e| Error::Io(link.to_owned(), e))?;
            if current == target {
                return Ok(false);
            }
            if !made_by_link(&current, store_gems) {
                return Err(Error::Occupied(link.to_owned()));
            }
        }
        Ok(_) => return Err(Error::Occupied(link.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(link.to_owned(), e)),
    }
    let tmp = tmp_beside(link);
    std::os::unix::fs::symlink(target, &tmp).map_err(|e| Error::Io(tmp.clone(), e))?;
    fs::rename(&tmp, link).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::Io(link.to_owned(), e)
    })?;
    Ok(true)
}

/// Make `path` a file holding `contents` (with `mode`, else the umask's),
/// unless it already is one.
fn place_file(path: &Path, contents: &[u8], mode: Option<u32>) -> Result<bool, Error> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.is_file()
        && mode.is_none_or(|m| meta.permissions().mode() & 0o7777 == m)
        && fs::read(path).is_ok_and(|current| current == contents)
    {
        return Ok(false);
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        return Err(Error::Occupied(path.to_owned()));
    }
    let tmp = tmp_beside(path);
    let written = fs::write(&tmp, contents).and_then(|()| match mode {
        Some(mode) => fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)),
        None => Ok(()),
    });
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(Error::Io(path.to_owned(), e));
    }
    Ok(true)
}

/// Remove what a `spinel` that is no longer running left half made in `dir`.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let me = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.strip_prefix(TMP_PREFIX))
            .and_then(|n| n.split('-').next())
            .and_then(|p| p.parse::<u32>().ok());
        if let Some(pid) = pid
            && pid != me
            && !gem_store::running(pid)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Remove each gem an earlier link made that is not `wanted`: its symlink
/// into the store, its gemspec, and the bin stubs it would have, when they
/// are still the ones a link writes. Returns their full names, sorted.
fn prune(
    home: &Path,
    store_gems: &Path,
    wanted: &HashSet<String>,
    stubs: &HashMap<&str, &str>,
) -> Result<Vec<String>, Error> {
    let gems = home.join("gems");
    let entries = fs::read_dir(&gems).map_err(|e| Error::Io(gems.clone(), e))?;
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let Some(full_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if wanted.contains(&full_name) || full_name.starts_with(TMP_PREFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(target) = fs::read_link(&path) else {
            continue;
        };
        if !made_by_link(&target, store_gems) {
            continue;
        }
        // The entry's gemspec names the stubs the link wrote for it.
        let spec = target
            .parent()
            .and_then(|entry| fs::read(entry.join("metadata")).ok())
            .and_then(|yaml| gem_spec::from_yaml(&yaml).ok());
        fs::remove_file(&path).map_err(|e| Error::Io(path.clone(), e))?;
        let gemspec = home
            .join("specifications")
            .join(format!("{full_name}.gemspec"));
        match fs::remove_file(&gemspec) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(gemspec, e)),
        }
        if let Some(spec) = spec
            && let (Ok(name), Ok(exes)) = (spec.name(), spec.executables())
        {
            for exe in exes {
                if stubs.contains_key(exe.as_str()) {
                    continue;
                }
                let stub = home.join("bin").join(&exe);
                if fs::read(&stub).is_ok_and(|t| t == app_script_text(&name, &exe).as_bytes()) {
                    let _ = fs::remove_file(&stub);
                }
            }
        }
        removed.push(full_name);
    }
    removed.sort();
    Ok(removed)
}

#[cfg(test)]
mod tests;
