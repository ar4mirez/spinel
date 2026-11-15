//! The global gem store (#81, docs/package-manager.md, "Global store").
//!
//! Every gem any project installs is fetched once, verified, unpacked once,
//! and kept under `<spinel home>/store/gems/`, in a directory named by the
//! gem and the SHA-256 of its `.gem`:
//!
//! ```text
//! store/gems/<name>-<version>-<platform>-<sha256>/
//!   gem/        data.tar.gz unpacked: what a GEM_HOME's gems/<full name> links to
//!   metadata    the gemspec, as YAML, from metadata.gz
//! ```
//!
//! The SHA-256 is the one the index (or later the lockfile's `CHECKSUMS`)
//! lists, so the name says what is inside: a second project asking for the
//! same gem with the same checksum finds it and fetches nothing, and two
//! sources serving different bytes under one name never share an entry.
//!
//! An entry is built in `store/tmp/` and renamed into place whole, after the
//! `.gem` matched its checksum, its own `checksums.yaml.gz` matched its parts,
//! and every file it holds was written. A download cut off, a process killed
//! mid-unpack, an archive refused: each leaves at most a staging directory in
//! `store/tmp/`, named by the process that made it, which the next run removes
//! once that process is gone. Nothing under `store/gems/` is ever half
//! written.
//!
//! Downloads go through the compact index client's HTTP client and its limit
//! on requests in flight; unpacking runs on blocking threads, at most one per
//! CPU, beside them, so a cold install waits on the network rather than on
//! gunzip.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::GzDecoder;
use sha2::{Digest as _, Sha256, Sha512};
use tokio::sync::Semaphore;

use crate::compact_index;

/// The most a gem's gemspec may unpack to. The largest on rubygems.org are a
/// few megabytes; this only stops a compressed bomb.
const METADATA_LIMIT: u64 = 64 << 20;

/// One gem, as a lockfile and `/info` name it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GemId {
    pub name: String,
    pub version: String,
    /// `ruby` for a gem with no platform.
    pub platform: String,
}

impl GemId {
    /// A gem, checked so that its names can be put in a path and a URL: a
    /// name rubygems.org allows, a version of digits, letters and dots that
    /// starts with a digit, and a platform of letters, digits, `_`, `.` and
    /// `-`.
    pub fn new(name: &str, version: &str, platform: &str) -> Result<GemId, Error> {
        let spelled = || format!("{name} {version} {platform}");
        let version_ok = version.starts_with(|c: char| c.is_ascii_digit())
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.');
        let platform_ok = !platform.is_empty()
            && platform
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if !compact_index::valid_name(name) || !version_ok || !platform_ok {
            return Err(Error::Gem(spelled()));
        }
        Ok(GemId {
            name: name.to_owned(),
            version: version.to_owned(),
            platform: platform.to_owned(),
        })
    }

    /// `name@version` or `name@version-platform`: the version as an `/info`
    /// line's first field spells it, which Bundler splits at its first `-`.
    pub fn parse(spec: &str) -> Result<GemId, Error> {
        let (name, version) = spec
            .rsplit_once('@')
            .ok_or_else(|| Error::Gem(spec.to_owned()))?;
        let (version, platform) = version.split_once('-').unwrap_or((version, "ruby"));
        GemId::new(name, version, platform).map_err(|_| Error::Gem(spec.to_owned()))
    }

    /// RubyGems' `full_name`: `name-version`, and `-platform` unless it is
    /// `ruby`.
    pub fn full_name(&self) -> String {
        if self.platform == "ruby" {
            format!("{}-{}", self.name, self.version)
        } else {
            format!("{}-{}-{}", self.name, self.version, self.platform)
        }
    }

    /// The first field of the gem's `/info` line.
    pub fn index_version(&self) -> String {
        if self.platform == "ruby" {
            self.version.clone()
        } else {
            format!("{}-{}", self.version, self.platform)
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// Not a gem name, version and platform that can name a store entry.
    Gem(String),
    /// Not a SHA-256 in hex.
    Sha256(String),
    /// The download failed.
    Index(compact_index::Error),
    /// The `.gem` is not the one the index lists.
    Checksum {
        gem: String,
        expected: String,
        actual: String,
    },
    /// The `.gem` is not one RubyGems would install: a part missing or
    /// repeated, a part that does not match `checksums.yaml.gz`, an archive
    /// that does not read.
    Format {
        gem: String,
        message: String,
    },
    /// An archive entry that would be written outside the gem's directory.
    Path {
        gem: String,
        entry: String,
    },
    /// A symlink that would point outside the gem's directory.
    Symlink {
        gem: String,
        entry: String,
        target: String,
    },
    Io(PathBuf, io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Gem(g) => write!(f, "{g:?}: not a gem name, version and platform"),
            Error::Sha256(s) => write!(f, "{s:?}: not a SHA-256"),
            Error::Index(e) => write!(f, "{e}"),
            Error::Checksum {
                gem,
                expected,
                actual,
            } => write!(
                f,
                "{gem}: the downloaded .gem has SHA-256 {actual}, but the index lists {expected}"
            ),
            Error::Format { gem, message } => write!(f, "{gem}: {message}"),
            Error::Path { gem, entry } => write!(
                f,
                "{gem}: archive entry {entry:?} would be written outside the gem's directory"
            ),
            Error::Symlink { gem, entry, target } => write!(
                f,
                "{gem}: symlink {entry:?} points to {target:?}, outside the gem's directory"
            ),
            Error::Io(path, e) => write!(f, "{}: {e}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

/// What fetching one gem did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Downloaded, this many bytes of `.gem`, verified and unpacked.
    Fetched(u64),
    /// Already in the store: no request.
    Stored,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Fetched(n) => write!(f, "fetched {n} bytes"),
            Outcome::Stored => f.write_str("in the store"),
        }
    }
}

/// A gem in the store.
#[derive(Debug)]
pub struct Fetched {
    /// The entry: `gem/` and `metadata` are in it.
    pub path: PathBuf,
    pub outcome: Outcome,
    /// What RubyGems would warn about while installing it: a dangling symlink
    /// left out.
    pub warnings: Vec<String>,
}

/// The store under one Spinel home.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    /// Numbers this process's staging directories.
    next: Arc<AtomicU64>,
}

impl Store {
    /// The store under `home`: `<home>/store/`.
    pub fn new(home: &Path) -> Store {
        Store {
            root: home.join("store"),
            next: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the gem with this `.gem` SHA-256 is kept.
    pub fn entry(&self, gem: &GemId, sha256: &str) -> PathBuf {
        self.root.join("gems").join(format!(
            "{}-{}-{}-{sha256}",
            gem.name, gem.version, gem.platform
        ))
    }

    /// Fetch each gem, given with the SHA-256 of its `.gem` in hex, from
    /// `client`'s source into the store, as many at once as the client's
    /// limit allows. One already stored costs no request. Results come back
    /// in `wants`' order, a gem asked for twice once, as its first.
    pub async fn fetch(
        &self,
        client: &compact_index::Client,
        wants: &[(GemId, String)],
    ) -> Vec<(GemId, Result<Fetched, Error>)> {
        self.sweep();
        let mut seen = HashSet::new();
        let wants: Vec<(GemId, String)> = wants
            .iter()
            .map(|(gem, sha)| (gem.clone(), sha.to_ascii_lowercase()))
            .filter(|want| seen.insert(want.clone()))
            .collect();
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        let unpacking = Arc::new(Semaphore::new(cpus));
        let mut results: Vec<Option<Result<Fetched, Error>>> = Vec::new();
        let mut tasks = tokio::task::JoinSet::new();
        for (i, (gem, sha)) in wants.iter().enumerate() {
            results.push(None);
            if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                results[i] = Some(Err(Error::Sha256(sha.clone())));
                continue;
            }
            let path = self.entry(gem, sha);
            if path.join("gem").is_dir() {
                results[i] = Some(Ok(Fetched {
                    path,
                    outcome: Outcome::Stored,
                    warnings: Vec::new(),
                }));
                continue;
            }
            let (store, client, unpacking) = (self.clone(), client.clone(), unpacking.clone());
            let (gem, sha) = (gem.clone(), sha.clone());
            tasks.spawn(async move { (i, store.fetch_one(&client, &gem, &sha, &unpacking).await) });
        }
        while let Some(joined) = tasks.join_next().await {
            let (i, result) = joined.expect("a store task panicked");
            results[i] = Some(result);
        }
        wants
            .into_iter()
            .map(|(gem, _)| gem)
            .zip(
                results
                    .into_iter()
                    .map(|r| r.expect("every gem has a result")),
            )
            .collect()
    }

    /// Download one `.gem` into `store/tmp/`, check it, unpack it beside it,
    /// and rename the result into place. Whatever happens, the download and
    /// any staging directory are gone afterwards.
    async fn fetch_one(
        &self,
        client: &compact_index::Client,
        gem: &GemId,
        sha256: &str,
        unpacking: &Semaphore,
    ) -> Result<Fetched, Error> {
        let tmp = self.root.join("tmp");
        fs::create_dir_all(&tmp).map_err(|e| Error::Io(tmp.clone(), e))?;
        let stem = format!(
            "{}-{}",
            std::process::id(),
            self.next.fetch_add(1, Ordering::Relaxed)
        );
        let download = tmp.join(format!("{stem}.gem"));
        let staging = tmp.join(&stem);
        let result = async {
            let (actual, size) = client
                .download(&format!("{}.gem", gem.full_name()), &download)
                .await
                .map_err(Error::Index)?;
            if actual != sha256 {
                return Err(Error::Checksum {
                    gem: gem.full_name(),
                    expected: sha256.to_owned(),
                    actual,
                });
            }
            let _permit = unpacking
                .acquire()
                .await
                .expect("the semaphore is never closed");
            let warnings = {
                let (gem, download, staging) = (gem.clone(), download.clone(), staging.clone());
                tokio::task::spawn_blocking(move || unpack(&gem, &download, &staging))
                    .await
                    .expect("unpacking a gem panicked")?
            };
            let path = self.entry(gem, sha256);
            let parent = path.parent().expect("an entry is under store/gems");
            fs::create_dir_all(parent).map_err(|e| Error::Io(parent.to_owned(), e))?;
            // ponytail: nothing is fsynced, so a power cut (not a killed
            // process) just after the rename could keep an entry whose files
            // never reached the disk. Upgrade: fsync the files and the
            // staging directory before the rename, if that ever shows up.
            match fs::rename(&staging, &path) {
                Ok(()) => {}
                // Another `spinel` stored the same gem first; its entry is as
                // good as this one.
                Err(_) if path.join("gem").is_dir() => remove(&staging),
                Err(e) => return Err(Error::Io(path, e)),
            }
            Ok(Fetched {
                path,
                outcome: Outcome::Fetched(size),
                warnings,
            })
        }
        .await;
        remove(&download);
        if result.is_err() {
            remove(&staging);
        }
        result
    }

    /// Remove what a `spinel` that is no longer running left in
    /// `store/tmp/`: each name starts with the process id that made it.
    fn sweep(&self) {
        let Ok(dir) = fs::read_dir(self.root.join("tmp")) else {
            return;
        };
        let me = std::process::id();
        for entry in dir.flatten() {
            let name = entry.file_name();
            let pid = name
                .to_str()
                .and_then(|n| n.split(['-', '.']).next())
                .and_then(|p| p.parse::<u32>().ok());
            if let Some(pid) = pid
                && pid != me
                && !running(pid)
            {
                remove(&entry.path());
            }
        }
    }
}

/// Whether a process with this id exists. `kill(pid, 0)` checks without
/// sending anything; EPERM means it exists under another user.
fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        // 0 and negative ids name process groups, not a process.
        return true;
    }
    // SAFETY: signal 0 delivers nothing; this only asks whether `pid` exists.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Remove a file or a directory tree, if it is there.
fn remove(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// Unpack the `.gem` at `file` into `staging/gem/` and `staging/metadata`,
/// as `Gem::Package` verifies and extracts one, and answer RubyGems'
/// warnings.
///
/// A `.gem` is a tar of `metadata.gz` (the gemspec as YAML), `data.tar.gz`
/// (the files) and `checksums.yaml.gz` (the SHA-256 and SHA-512 of the other
/// two), with a `.sig` beside each for a signed gem. As in `Gem::Package#verify`:
/// a part named twice is refused, the gemspec and the files must both be
/// there, and every digest `checksums.yaml.gz` lists must match. Signatures
/// are not checked, as `gem install` does not check them without a
/// `--trust-policy`.
pub fn unpack(gem: &GemId, file: &Path, staging: &Path) -> Result<Vec<String>, Error> {
    let name = gem.full_name();
    let format = |message: String| Error::Format {
        gem: name.clone(),
        message,
    };
    let dest = staging.join("gem");
    fs::create_dir_all(&dest).map_err(|e| Error::Io(dest.clone(), e))?;
    let opened = fs::File::open(file).map_err(|e| Error::Io(file.to_owned(), e))?;
    let mut outer = tar::Archive::new(io::BufReader::new(opened));
    let mut parts: Vec<String> = Vec::new();
    let mut digests: HashMap<String, [String; 2]> = HashMap::new();
    let mut metadata = None;
    let mut checksums = None;
    let mut warnings = Vec::new();
    let entries = outer.entries().map_err(|e| format(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format(e.to_string()))?;
        let part = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        if parts.contains(&part) {
            return Err(format(format!(
                "duplicate files in the package: ({part:?})"
            )));
        }
        parts.push(part.clone());
        if part.ends_with(".sig") {
            continue;
        }
        let mut hashing = Hashing::new(&mut entry);
        let read_error = |e: io::Error| format(format!("{part}: {e}"));
        match part.as_str() {
            "metadata.gz" => {
                let mut gz = GzDecoder::new(&mut hashing).take(METADATA_LIMIT + 1);
                let mut yaml = Vec::new();
                gz.read_to_end(&mut yaml).map_err(read_error)?;
                if yaml.len() as u64 > METADATA_LIMIT {
                    return Err(format(format!(
                        "metadata.gz unpacks to more than {METADATA_LIMIT} bytes"
                    )));
                }
                metadata = Some(yaml);
            }
            "metadata" => {
                let mut yaml = Vec::new();
                (&mut hashing)
                    .take(METADATA_LIMIT + 1)
                    .read_to_end(&mut yaml)
                    .map_err(read_error)?;
                if yaml.len() as u64 > METADATA_LIMIT {
                    return Err(format(format!(
                        "metadata is more than {METADATA_LIMIT} bytes"
                    )));
                }
                metadata = Some(yaml);
            }
            "data.tar.gz" => {
                warnings.extend(extract(&name, GzDecoder::new(&mut hashing), &dest)?);
            }
            "checksums.yaml.gz" => {
                let mut yaml = Vec::new();
                GzDecoder::new(&mut hashing)
                    .take(METADATA_LIMIT)
                    .read_to_end(&mut yaml)
                    .map_err(read_error)?;
                checksums = Some(parse_checksums(&String::from_utf8_lossy(&yaml)));
            }
            _ => {}
        }
        // The digest is of the whole part, so read what a decoder left.
        io::copy(&mut hashing, &mut io::sink()).map_err(read_error)?;
        digests.insert(part, hashing.finish());
    }
    let Some(metadata) = metadata else {
        return Err(format("package metadata is missing".to_owned()));
    };
    if !parts.iter().any(|p| p == "data.tar.gz") {
        return Err(format(
            "package content (data.tar.gz) is missing".to_owned(),
        ));
    }
    for (algorithm, files) in checksums.unwrap_or_default() {
        // ponytail: SHA1, which RubyGems 2.0 to 2.x also wrote, is not
        // checked; those gems list a SHA512 too, and the index's SHA-256 of
        // the whole `.gem` covers every part anyway. Upgrade: a `sha1` crate.
        let which = match algorithm.as_str() {
            "SHA256" => 0,
            "SHA512" => 1,
            _ => continue,
        };
        for (part, expected) in files {
            let ok = digests
                .get(&part)
                .is_some_and(|d| d[which].eq_ignore_ascii_case(&expected));
            if !ok {
                return Err(format(format!("{algorithm} checksum mismatch for {part}")));
            }
        }
    }
    let path = staging.join("metadata");
    fs::write(&path, metadata).map_err(|e| Error::Io(path, e))?;
    Ok(warnings)
}

/// `checksums.yaml` as RubyGems writes it, a map of algorithm to a map of
/// part to hex digest, in the sorted order RubyGems checks them in:
///
/// ```yaml
/// ---
/// SHA256:
///   metadata.gz: 5e68...
///   data.tar.gz: f082...
/// ```
///
/// A line in any other shape is passed over: the parts it would have checked
/// are still covered by the index's digest of the whole `.gem`.
fn parse_checksums(yaml: &str) -> Vec<(String, Vec<(String, String)>)> {
    let unquote = |s: &str| s.trim().trim_matches(['"', '\'']).to_owned();
    let mut found: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in yaml.lines() {
        if line.trim().is_empty() || line.starts_with("---") {
            continue;
        }
        if !line.starts_with(' ') {
            if let Some(algorithm) = line.trim_end().strip_suffix(':') {
                found.push((unquote(algorithm), Vec::new()));
            }
            continue;
        }
        if let (Some(last), Some((part, digest))) = (found.last_mut(), line.split_once(": ")) {
            last.1.push((unquote(part), unquote(digest)));
        }
    }
    found.sort();
    for (_, files) in &mut found {
        files.sort();
    }
    found
}

/// Reads through, keeping the SHA-256 and SHA-512 of what it read.
struct Hashing<R> {
    inner: R,
    sha256: Sha256,
    sha512: Sha512,
}

impl<R: Read> Hashing<R> {
    fn new(inner: R) -> Self {
        Hashing {
            inner,
            sha256: Sha256::new(),
            sha512: Sha512::new(),
        }
    }

    fn finish(self) -> [String; 2] {
        [
            compact_index::hex(&self.sha256.finalize()),
            compact_index::hex(&self.sha512.finalize()),
        ]
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.sha256.update(&buf[..n]);
        self.sha512.update(&buf[..n]);
        Ok(n)
    }
}

/// Where `path`, relative to the directory `base` names, leads, by its
/// spelling: `None` when it is absolute, climbs above the gem's directory, or
/// is the directory itself. `Gem::Package#install_location`'s test, which
/// expands the name against the directory and requires the result to be
/// inside it.
fn within(base: &[Vec<u8>], path: &[u8]) -> Option<Vec<Vec<u8>>> {
    if path.starts_with(b"/") {
        return None;
    }
    let mut at = base.to_vec();
    for part in path.split(|&b| b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                at.pop()?;
            }
            part => at.push(part.to_vec()),
        }
    }
    (!at.is_empty()).then_some(at)
}

fn joined(dest: &Path, parts: &[Vec<u8>]) -> PathBuf {
    let mut path = dest.to_owned();
    for part in parts {
        path.push(OsStr::from_bytes(part));
    }
    path
}

/// Extract `data.tar.gz` into `dest` as `Gem::Package#extract_tar_gz` does:
/// every entry's name must stay inside `dest`; directories and files are
/// written, a file with its mode; symlinks are made last, so no entry is ever
/// written through one, and only when their target stays inside `dest` too;
/// any other kind of entry is passed over.
///
/// Two differences from RubyGems, both stricter. A mode keeps only its
/// permission bits, so no setuid file lands in a store every project shares.
/// And a symlink's target is checked where it physically leads, once it
/// exists, as well as by its spelling: `a/b/c/up -> ../..` is inside by
/// both, but `a/b/c/up/x -> ../../..` is inside by its spelling and outside
/// through `up`, which RubyGems' check of the spelling alone lets through.
/// A symlink may not replace an existing directory or symlink, its directory
/// must physically be inside `dest`, and every symlink is checked again once
/// all are made, so a later entry cannot redirect one checked earlier.
fn extract(gem: &str, data: impl Read, dest: &Path) -> Result<Vec<String>, Error> {
    let format = |e: io::Error| Error::Format {
        gem: gem.to_owned(),
        message: format!("data.tar.gz: {e}"),
    };
    let io_error = |path: &Path| {
        let path = path.to_owned();
        move |e| Error::Io(path, e)
    };
    let mut archive = tar::Archive::new(data);
    let mut symlinks = Vec::new();
    for entry in archive.entries().map_err(format)? {
        let mut entry = entry.map_err(format)?;
        let raw = entry.path_bytes().into_owned();
        let name = String::from_utf8_lossy(&raw).into_owned();
        let Some(parts) = within(&[], &raw) else {
            return Err(Error::Path {
                gem: gem.to_owned(),
                entry: name,
            });
        };
        let path = joined(dest, &parts);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                fs::create_dir_all(&path).map_err(io_error(&path))?;
            }
            tar::EntryType::Regular => {
                let parent = path.parent().expect("an entry is inside dest");
                fs::create_dir_all(parent).map_err(io_error(parent))?;
                remove(&path);
                let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
                let mut out = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .map_err(io_error(&path))?;
                io::copy(&mut entry, &mut out).map_err(format)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                    .map_err(io_error(&path))?;
            }
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name_bytes()
                    .map(|t| t.into_owned())
                    .unwrap_or_default();
                let escape = || Error::Symlink {
                    gem: gem.to_owned(),
                    entry: name.clone(),
                    target: String::from_utf8_lossy(&target).into_owned(),
                };
                if within(&parts[..parts.len() - 1], &target).is_none() {
                    return Err(escape());
                }
                symlinks.push((name, path, target));
            }
            _ => {}
        }
    }
    let root = fs::canonicalize(dest).map_err(io_error(dest))?;
    let inside = |real: &Path| real != root && real.starts_with(&root);
    let mut warnings = Vec::new();
    let mut made = Vec::new();
    for (name, path, target) in symlinks {
        let escape = || Error::Symlink {
            gem: gem.to_owned(),
            entry: name.clone(),
            target: String::from_utf8_lossy(&target).into_owned(),
        };
        let parent = path.parent().expect("an entry is inside dest");
        fs::create_dir_all(parent).map_err(io_error(parent))?;
        match fs::canonicalize(parent) {
            Ok(real) if real == root || inside(&real) => {}
            _ => return Err(escape()),
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() || meta.is_symlink() => return Err(escape()),
            Ok(_) => remove(&path),
            Err(_) => {}
        }
        std::os::unix::fs::symlink(OsStr::from_bytes(&target), &path).map_err(io_error(&path))?;
        match fs::canonicalize(&path) {
            Ok(real) if inside(&real) => made.push((name, path, target)),
            Ok(_) => {
                remove(&path);
                return Err(escape());
            }
            Err(_) => {
                remove(&path);
                warnings.push(format!(
                    "{gem} ships with a dangling symlink named {name} pointing to missing {} file. Ignoring",
                    String::from_utf8_lossy(&target)
                ));
            }
        }
    }
    for (name, path, target) in made {
        if !fs::canonicalize(&path).is_ok_and(|real| inside(&real)) {
            return Err(Error::Symlink {
                gem: gem.to_owned(),
                entry: name,
                target: String::from_utf8_lossy(&target).into_owned(),
            });
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests;
