//! `Gemfile.lock` (#80, docs/package-manager.md, "Lockfile"): read as
//! Bundler 4's `LockfileParser` reads it, written as its `LockfileGenerator`
//! writes it, so a lockfile Bundler wrote reads back and writes out byte for
//! byte, and one Spinel writes is the one Bundler would have.
//!
//! What a lockfile holds is [`Lockfile`]: its sources, each with the specs
//! locked from it, then `PLATFORMS`, `DEPENDENCIES`, `CHECKSUMS` (when the
//! lockfile has the section), `RUBY VERSION` (when the Gemfile names a Ruby)
//! and `BUNDLED WITH`. Writing sorts as the generator sorts: specs by
//! `full_name` within their source, a spec's dependencies, the
//! `DEPENDENCIES` and the `CHECKSUMS` by `to_s`, the platforms by name.
//! Sources are written in the order the lockfile has them; Bundler orders a
//! new lockfile's (`SourceList#lock_sources`) git, path and plugin sources
//! by identifier, then the rubygems ones, and [`Lockfile::from_resolution`]
//! has only one.
//!
//! A new lockfile is [`Lockfile::from_resolution`]: a resolution's specs and
//! the Gemfile's dependencies, its `PLATFORMS` shaped as
//! `Definition#start_resolution` shapes them for a lockfile that does not
//! exist yet. `SpecSet#add_extra_platforms!` adds each platform some locked
//! gem has a build for that every locked gem can be installed on with what
//! is already locked (`ruby` when nothing has a native build), and drops the
//! local platform when a more specific one of the same machine came in
//! (`x86_64-linux` for `x86_64-linux-gnu`); `SpecSet#for` then keeps the
//! builds those platforms install.

use std::collections::HashSet;
use std::fmt;

use base64::Engine as _;

use crate::gem_platform::{self, GemPlatform, Platform};
use crate::gem_requirement::Requirement;
use crate::gem_version::Version;
use crate::resolver::{self, Index, Request, Resolved, Spec};

/// The Bundler a lockfile Spinel writes says it was written with, and the
/// one `bundler` resolves to: 4.0.20, whose `bundle lock` the fixtures in
/// `tests/fixtures/lockfile/` are, byte for byte, and whose resolver the
/// resolver's are measured against.
pub const BUNDLER_VERSION: &str = "4.0.20";

/// `Source::Git::DEFAULT_GLOB` and `Source::Path::DEFAULT_GLOB`.
const DEFAULT_GLOB: &str = "{,*,*/*}.gemspec";

/// A source's section header and the options written under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// `GEM`: `Source::Rubygems`, its remotes as the lockfile lists them.
    Gem { remotes: Vec<String> },
    /// `GIT`: `Source::Git#to_lock`'s options, in its order.
    Git {
        remote: String,
        revision: String,
        git_ref: Option<String>,
        branch: Option<String>,
        tag: Option<String>,
        submodules: Option<String>,
        glob: Option<String>,
    },
    /// `PATH`: `Source::Path#to_lock`'s.
    Path {
        remote: String,
        glob: Option<String>,
    },
    /// `PLUGIN SOURCE`: what a plugin's `to_lock` writes is the plugin's, so
    /// its options are kept as the lockfile has them.
    Plugin { options: Vec<(String, String)> },
}

/// A source and the specs locked from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub kind: SourceKind,
    pub specs: Vec<LockedSpec>,
}

/// A spec line under `specs:` (`LazySpecification`) and the dependency
/// lines under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedSpec {
    pub name: String,
    pub version: Version,
    pub platform: GemPlatform,
    pub dependencies: Vec<(String, Requirement)>,
}

impl LockedSpec {
    /// `full_name`: what specs sort by.
    pub fn full_name(&self) -> String {
        match &self.platform {
            GemPlatform::Ruby => format!("{}-{}", self.name, self.version),
            platform => format!("{}-{}-{platform}", self.name, self.version),
        }
    }

    /// `lock_name`: `rack (3.2.8)`, `json (3.1.0-java)`.
    pub fn lock_name(&self) -> String {
        lock_name(&self.name, &self.version, &self.platform)
    }
}

pub(crate) fn lock_name(name: &str, version: &Version, platform: &GemPlatform) -> String {
    match platform {
        GemPlatform::Ruby => format!("{name} ({version})"),
        platform => format!("{name} ({version}-{platform})"),
    }
}

/// A `DEPENDENCIES` line: a Gemfile dependency, `!` when it names a source
/// of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub requirement: Requirement,
    pub pinned: bool,
}

/// A `CHECKSUMS` line: a spec's `lock_name` and its checksums, each
/// `algo=digest`, one per algorithm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checksum {
    pub lock_name: String,
    pub checksums: Vec<(String, String)>,
}

/// A whole lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub sources: Vec<Source>,
    pub platforms: Vec<GemPlatform>,
    pub dependencies: Vec<Dependency>,
    /// `None` when there is no `CHECKSUMS` section, which Bundler then does
    /// not write either.
    pub checksums: Option<Vec<Checksum>>,
    /// `RUBY VERSION`, as written (`ruby 3.4.1`).
    pub ruby_version: Option<String>,
    /// `BUNDLED WITH`; [`BUNDLER_VERSION`] is written when it is missing.
    pub bundled_with: Option<Version>,
}

/// Why a lockfile cannot be read: `Bundler::LockfileError`, worded as
/// Bundler words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockfileError(pub String);

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LockfileError {}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

const GIT: &str = "GIT";
const GEM: &str = "GEM";
const PATH: &str = "PATH";
const PLUGIN: &str = "PLUGIN SOURCE";
const SPECS: &str = "  specs:";

/// A line `NAME_VERSION` matches: its indentation (2, 4 or 6 spaces), name,
/// version or requirements in parentheses, platform after the version's
/// first `-`, the `!` of a pinned dependency, and checksums after a space.
#[derive(Debug, PartialEq, Eq)]
struct NameVersion<'a> {
    spaces: usize,
    name: &'a str,
    version: Option<&'a str>,
    platform: Option<&'a str>,
    pinned: bool,
    checksum: Option<&'a str>,
}

/// `LockfileParser::NAME_VERSION`, matched as Ruby's regexp engine matches
/// it:
///
/// ```text
/// ^( {2}| {4}| {6})(?! )(.*?)(?: \(([^-]*)(?:-(.*))?\))?(!)?(?: ([^ ]+))?$
/// ```
///
/// The name is the shortest prefix the rest matches after; the version
/// takes as much as it can before a `-`, the platform as much as it can
/// before a `)`.
fn name_version(line: &str) -> Option<NameVersion<'_>> {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    if ![2, 4, 6].contains(&spaces) {
        return None;
    }
    let rest = &line[spaces..];
    (0..=rest.len())
        .filter(|&i| rest.is_char_boundary(i))
        .find_map(|i| {
            let (version, platform, pinned, checksum) = tail(&rest[i..])?;
            Some(NameVersion {
                spaces,
                name: &rest[..i],
                version,
                platform,
                pinned,
                checksum,
            })
        })
}

type Tail<'a> = (Option<&'a str>, Option<&'a str>, bool, Option<&'a str>);

/// `(?: \(([^-]*)(?:-(.*))?\))?(!)?(?: ([^ ]+))?$`.
fn tail(s: &str) -> Option<Tail<'_>> {
    if let Some(inner) = s.strip_prefix(" (") {
        let longest = inner.find('-').unwrap_or(inner.len());
        for j in (0..=longest).rev().filter(|&j| inner.is_char_boundary(j)) {
            let (version, after) = inner.split_at(j);
            if let Some(platform) = after.strip_prefix('-') {
                for k in (0..platform.len())
                    .rev()
                    .filter(|&k| platform[k..].starts_with(')'))
                {
                    if let Some((pinned, checksum)) = line_end(&platform[k + 1..]) {
                        return Some((Some(version), Some(&platform[..k]), pinned, checksum));
                    }
                }
            }
            if let Some(after) = after.strip_prefix(')')
                && let Some((pinned, checksum)) = line_end(after)
            {
                return Some((Some(version), None, pinned, checksum));
            }
        }
    }
    line_end(s).map(|(pinned, checksum)| (None, None, pinned, checksum))
}

/// `(!)?(?: ([^ ]+))?$`.
fn line_end(s: &str) -> Option<(bool, Option<&str>)> {
    let candidates = [s.strip_prefix('!').map(|t| (true, t)), Some((false, s))];
    candidates.into_iter().flatten().find_map(|(pinned, t)| {
        if let Some(checksum) = t.strip_prefix(' ')
            && !checksum.is_empty()
            && !checksum.contains(' ')
        {
            return Some((pinned, Some(checksum)));
        }
        t.is_empty().then_some((pinned, None))
    })
}

/// `OPTIONS`: `/^  ([a-z]+): (.*)$/i`.
fn option(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("  ")?;
    let (key, value) = rest.split_once(": ")?;
    (!key.is_empty() && key.bytes().all(|b| b.is_ascii_alphabetic())).then_some((key, value))
}

/// `Checksum.to_hexdigest`: a `sha256` digest may be written in base64,
/// and is kept as hex.
fn to_hexdigest(algo: &str, digest: &str) -> Result<String, String> {
    if algo != "sha256" {
        return Ok(digest.to_owned());
    }
    if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(digest.to_owned());
    }
    let base64_shaped = {
        let body = digest.trim_end_matches('=');
        body.len() == 43
            && digest.len() - body.len() <= 2
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_+/".contains(&b))
    };
    if base64_shaped {
        let standard: String = digest
            .chars()
            .map(|c| match c {
                '-' => '+',
                '_' => '/',
                c => c,
            })
            .collect();
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(standard.as_bytes()) {
            return Ok(bytes.iter().map(|b| format!("{b:02x}")).collect());
        }
    }
    Err(format!(
        "{digest:?} is not a valid SHA256 hex or base64 digest"
    ))
}

/// What each line is read as: `@parse_method`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Source,
    Dependencies,
    Checksums,
    Platforms,
    Ruby,
    BundledWith,
}

/// The source being read: its header, then its options until `specs:`.
struct PendingSource {
    header: String,
    options: Vec<(String, Vec<String>)>,
}

impl PendingSource {
    fn first(&self, key: &str) -> Option<String> {
        self.options
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, values)| values.first().cloned())
    }

    /// `TYPES[@type].from_lock(@opts)`: `None` for a header with no type.
    fn into_kind(self) -> Option<SourceKind> {
        let kind = match self.header.as_str() {
            GEM => {
                let mut remotes: Vec<String> = Vec::new();
                for remote in self
                    .options
                    .iter()
                    .filter(|(k, _)| k == "remote")
                    .flat_map(|(_, values)| values)
                {
                    // `add_remote`'s `normalize_uri`: a trailing `/`, and
                    // no remote twice.
                    let remote = if remote.ends_with('/') {
                        remote.clone()
                    } else {
                        format!("{remote}/")
                    };
                    if !remotes.contains(&remote) {
                        remotes.push(remote);
                    }
                }
                SourceKind::Gem { remotes }
            }
            GIT => SourceKind::Git {
                remote: self.first("remote").unwrap_or_default(),
                revision: self.first("revision").unwrap_or_default(),
                git_ref: self.first("ref"),
                branch: self.first("branch"),
                tag: self.first("tag"),
                submodules: self.first("submodules"),
                glob: self.first("glob").filter(|g| g != DEFAULT_GLOB),
            },
            PATH => SourceKind::Path {
                remote: self.first("remote").unwrap_or_default(),
                glob: self.first("glob").filter(|g| g != DEFAULT_GLOB),
            },
            PLUGIN => SourceKind::Plugin {
                options: self
                    .options
                    .into_iter()
                    .flat_map(|(k, values)| values.into_iter().map(move |v| (k.clone(), v)))
                    .collect(),
            },
            _ => return None,
        };
        Some(kind)
    }
}

impl Lockfile {
    /// `LockfileParser.new`: `name` is the lockfile as error messages name
    /// it (`Gemfile.lock`), `local` the platform `current` means.
    pub fn parse(text: &str, name: &str, local: &Platform) -> Result<Lockfile, LockfileError> {
        let conflict = ["<<<<<<<", "=======", ">>>>>>>", "|||||||"];
        if conflict.iter().any(|marker| text.contains(marker)) {
            return Err(LockfileError(format!(
                "Your {name} contains merge conflicts.\n\
                 Run `git checkout HEAD -- {name}` first to get a clean lock."
            )));
        }
        let mut parser = Parser {
            name,
            line_number: 0,
            checksum_locations: Vec::new(),
            lockfile: Lockfile {
                sources: Vec::new(),
                platforms: Vec::new(),
                dependencies: Vec::new(),
                checksums: None,
                ruby_version: None,
                bundled_with: None,
            },
            local,
            section: None,
            pending: None,
            current_source: None,
        };
        for (index, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() {
                continue;
            }
            parser.line_number = index + 1;
            parser.line(line).map_err(|error| match error {
                LineError::Unreadable => LockfileError(format!(
                    "Your lockfile is unreadable. Run `rm {name}` and then `bundle install` \
                     to generate a new lockfile. The error occurred while evaluating \
                     {name}:{}:1",
                    index + 1
                )),
                LineError::Raised(message) => LockfileError(message),
            })?;
        }
        Ok(parser.lockfile)
    }
}

/// Why a line cannot be read: an `ArgumentError`, which Bundler reports as
/// an unreadable lockfile (the reason only in its debug log), or an error it
/// raises as it is.
enum LineError {
    Unreadable,
    Raised(String),
}

impl From<String> for LineError {
    fn from(_reason: String) -> LineError {
        LineError::Unreadable
    }
}

struct Parser<'a> {
    /// The lockfile as messages name it, and the line being read.
    name: &'a str,
    line_number: usize,
    /// Where each checksum was read (`lock_name`, `algo`, `file:line:col`),
    /// for a mismatch's message.
    checksum_locations: Vec<(String, String, String)>,
    lockfile: Lockfile,
    local: &'a Platform,
    section: Option<Section>,
    pending: Option<PendingSource>,
    /// Where `specs:` lines go: an index into `lockfile.sources`.
    current_source: Option<usize>,
}

impl Parser<'_> {
    fn line(&mut self, line: &str) -> Result<(), LineError> {
        match line {
            GIT | GEM | PATH | PLUGIN => {
                self.section = Some(Section::Source);
                self.current_source = None;
                self.pending = Some(PendingSource {
                    header: line.to_owned(),
                    options: Vec::new(),
                });
            }
            "DEPENDENCIES" => self.section = Some(Section::Dependencies),
            "CHECKSUMS" => {
                self.section = Some(Section::Checksums);
                self.lockfile.checksums.get_or_insert_with(Vec::new);
            }
            "PLATFORMS" => self.section = Some(Section::Platforms),
            "RUBY VERSION" => self.section = Some(Section::Ruby),
            "BUNDLED WITH" => self.section = Some(Section::BundledWith),
            _ if !line.starts_with(char::is_whitespace) => self.section = None,
            _ => match self.section {
                Some(Section::Source) => self.source_line(line)?,
                Some(Section::Dependencies) => self.dependency(line)?,
                Some(Section::Checksums) => self.checksum(line)?,
                Some(Section::Platforms) => {
                    if let Some(platform) = line.strip_prefix("  ") {
                        self.lockfile
                            .platforms
                            .push(GemPlatform::new(Some(platform.trim()), self.local));
                    }
                }
                Some(Section::Ruby) => self.lockfile.ruby_version = Some(line.trim().to_owned()),
                Some(Section::BundledWith) => {
                    let version = line.trim();
                    if Version::correct(version) {
                        self.lockfile.bundled_with = Some(
                            Version::new(version).map_err(|e| LineError::from(e.to_string()))?,
                        );
                    }
                }
                None => {}
            },
        }
        Ok(())
    }

    /// `parse_source`.
    fn source_line(&mut self, line: &str) -> Result<(), String> {
        if line == SPECS {
            let Some(pending) = self.pending.take() else {
                return Ok(());
            };
            if let Some(kind) = pending.into_kind() {
                self.lockfile.sources.push(Source {
                    kind,
                    specs: Vec::new(),
                });
                self.current_source = Some(self.lockfile.sources.len() - 1);
            }
            return Ok(());
        }
        if let Some((key, value)) = option(line)
            && let Some(pending) = &mut self.pending
        {
            match pending.options.iter_mut().find(|(k, _)| k == key) {
                Some((_, values)) => values.push(value.to_owned()),
                None => pending
                    .options
                    .push((key.to_owned(), vec![value.to_owned()])),
            }
            return Ok(());
        }
        self.spec(line)
    }

    /// `parse_spec`: a spec at 4 spaces, one of its dependencies at 6.
    fn spec(&mut self, line: &str) -> Result<(), String> {
        let Some(m) = name_version(line) else {
            return Ok(());
        };
        let Some(source) = self.current_source else {
            return Ok(());
        };
        match m.spaces {
            4 => {
                let version = Version::new(m.version.unwrap_or("")).map_err(|e| e.to_string())?;
                let spec = LockedSpec {
                    name: m.name.to_owned(),
                    version,
                    platform: GemPlatform::new(m.platform, self.local),
                    dependencies: Vec::new(),
                };
                // `@specs[full_name] = spec`: a spec locked twice is the
                // last one, under the last one's source.
                let full_name = spec.full_name();
                for source in &mut self.lockfile.sources {
                    source.specs.retain(|s| s.full_name() != full_name);
                }
                self.lockfile.sources[source].specs.push(spec);
            }
            6 => {
                let requirement = requirement(m.version)?;
                if let Some(spec) = self.lockfile.sources[source].specs.last_mut() {
                    spec.dependencies.push((m.name.to_owned(), requirement));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn find_spec(&self, full_name: &str) -> Option<&LockedSpec> {
        self.lockfile
            .sources
            .iter()
            .flat_map(|s| &s.specs)
            .find(|s| s.full_name() == full_name)
    }

    /// `parse_dependency`: `!` keeps the dependency pinned to the source of
    /// the spec locked for it, when there is one.
    fn dependency(&mut self, line: &str) -> Result<(), String> {
        let Some(m) = name_version(line).filter(|m| m.spaces == 2) else {
            return Ok(());
        };
        let requirement = requirement(m.version)?;
        let pinned = m.pinned
            && m.name != "bundler"
            && self
                .lockfile
                .sources
                .iter()
                .flat_map(|s| &s.specs)
                .any(|s| s.name == m.name);
        let dependency = Dependency {
            name: m.name.to_owned(),
            requirement,
            pinned,
        };
        // `@dependencies[dep.name] = dep`: the last of a name, in the
        // first one's place.
        match self
            .lockfile
            .dependencies
            .iter_mut()
            .find(|d| d.name == m.name)
        {
            Some(existing) => *existing = dependency,
            None => self.lockfile.dependencies.push(dependency),
        }
        Ok(())
    }

    /// `parse_checksum`: kept only for a spec the lockfile locks, or for
    /// `bundler`. Two digests of one algorithm for one gem are Bundler's
    /// `ChecksumMismatchError`.
    fn checksum(&mut self, line: &str) -> Result<(), LineError> {
        let Some(m) = name_version(line).filter(|m| m.spaces == 2) else {
            return Ok(());
        };
        let version = Version::new(m.version.unwrap_or("")).map_err(|e| e.to_string())?;
        let platform = GemPlatform::new(m.platform, self.local);
        let full_name = match &platform {
            GemPlatform::Ruby => format!("{}-{version}", m.name),
            platform => format!("{}-{version}-{platform}", m.name),
        };
        let lock_name = match self.find_spec(&full_name) {
            Some(spec) => spec.lock_name(),
            None if m.name == "bundler" => lock_name(m.name, &version, &platform),
            None => return Ok(()),
        };
        let store = self.lockfile.checksums.get_or_insert_with(Vec::new);
        let entry = match store.iter().position(|c| c.lock_name == lock_name) {
            Some(i) => &mut store[i],
            None => {
                store.push(Checksum {
                    lock_name: lock_name.clone(),
                    checksums: Vec::new(),
                });
                store.last_mut().expect("just pushed")
            }
        };
        for lock_checksum in m.checksum.into_iter().flat_map(|c| c.split(',')) {
            let column = line.find(lock_checksum).unwrap_or(0) + 1;
            let location = format!("{}:{}:{column}", self.name, self.line_number);
            let (algo, digest) = lock_checksum
                .trim()
                .split_once('=')
                .ok_or_else(|| format!("{lock_checksum:?} is not a checksum"))?;
            let digest = to_hexdigest(algo, digest)?;
            match entry.checksums.iter().find(|(a, _)| a == algo) {
                Some((_, existing)) if *existing != digest => {
                    let first = self
                        .checksum_locations
                        .iter()
                        .find(|(l, a, _)| *l == lock_name && a == algo)
                        .map_or("", |(_, _, at)| at.as_str());
                    return Err(LineError::Raised(format!(
                        "Bundler found mismatched checksums. This is a potential security risk.\n  \
                         {lock_name} {algo}={existing}\n    \
                         from the lockfile CHECKSUMS at {first}\n  \
                         {lock_name} {algo}={digest}\n    \
                         from the lockfile CHECKSUMS at {location}"
                    )));
                }
                Some(_) => {}
                None => {
                    entry.checksums.push((algo.to_owned(), digest));
                    self.checksum_locations
                        .push((lock_name.clone(), algo.to_owned(), location));
                }
            }
        }
        Ok(())
    }
}

/// A requirement as the lockfile writes it: `version.split(",")`, each
/// stripped.
fn requirement(list: Option<&str>) -> Result<Requirement, String> {
    match list {
        Some(list) => Requirement::new(list.split(',').map(str::trim)).map_err(|e| e.to_string()),
        None => Ok(Requirement::default()),
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// `Gem::Dependency#to_s`: what dependencies sort by.
fn dependency_to_s(name: &str, requirement: &Requirement) -> String {
    format!("{name} ({requirement})")
}

/// `Gem::Dependency#to_lock`, less its indentation: the name, then the
/// requirements, each `op version`, sorted and reversed, unless it is
/// `>= 0`.
fn dependency_to_lock(name: &str, requirement: &Requirement) -> String {
    if requirement.none() {
        return name.to_owned();
    }
    let mut reqs: Vec<String> = requirement
        .requirements()
        .iter()
        .map(|(op, version)| format!("{} {version}", op.as_str()))
        .collect();
    reqs.sort();
    reqs.reverse();
    format!("{name} ({})", reqs.join(", "))
}

impl SourceKind {
    /// `to_gemfile`: a git source's URI with the options it was given, a
    /// path source's path, a rubygems source's `identifier`.
    pub fn to_gemfile(&self) -> String {
        match self {
            SourceKind::Git {
                remote,
                git_ref,
                branch,
                tag,
                submodules,
                glob,
                ..
            } => {
                let specifiers: Vec<String> = [
                    ("ref", git_ref),
                    ("branch", branch),
                    ("tag", tag),
                    ("submodules", submodules),
                    ("glob", glob),
                ]
                .into_iter()
                .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}: {v}")))
                .collect();
                if specifiers.is_empty() {
                    remote.clone()
                } else {
                    format!("{remote} ({})", specifiers.join(", "))
                }
            }
            SourceKind::Path { remote, .. } => remote.clone(),
            // `Source::Rubygems#identifier`: its remotes, which it holds in
            // the reverse of the lockfile's order.
            SourceKind::Gem { remotes } if remotes.is_empty() => {
                "locally installed gems".to_owned()
            }
            SourceKind::Gem { remotes } => {
                let names: Vec<&str> = remotes.iter().rev().map(String::as_str).collect();
                format!("rubygems repository {}", names.join(", "))
            }
            SourceKind::Plugin { options } => options
                .iter()
                .find(|(k, _)| k == "remote")
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
        }
    }

    /// `to_lock`: the header and the options, up to `specs:`.
    fn to_lock(&self, out: &mut String) {
        match self {
            SourceKind::Gem { remotes } => {
                out.push_str("GEM\n");
                for remote in remotes {
                    out.push_str(&format!("  remote: {remote}\n"));
                }
            }
            SourceKind::Git {
                remote,
                revision,
                git_ref,
                branch,
                tag,
                submodules,
                glob,
            } => {
                out.push_str("GIT\n");
                out.push_str(&format!("  remote: {remote}\n"));
                out.push_str(&format!("  revision: {revision}\n"));
                for (key, value) in [
                    ("ref", git_ref),
                    ("branch", branch),
                    ("tag", tag),
                    ("submodules", submodules),
                    ("glob", glob),
                ] {
                    if let Some(value) = value {
                        out.push_str(&format!("  {key}: {value}\n"));
                    }
                }
            }
            SourceKind::Path { remote, glob } => {
                out.push_str("PATH\n");
                out.push_str(&format!("  remote: {remote}\n"));
                if let Some(glob) = glob {
                    out.push_str(&format!("  glob: {glob}\n"));
                }
            }
            SourceKind::Plugin { options } => {
                out.push_str("PLUGIN SOURCE\n");
                for (key, value) in options {
                    out.push_str(&format!("  {key}: {value}\n"));
                }
            }
        }
        out.push_str(SPECS);
        out.push('\n');
    }
}

impl Lockfile {
    /// `LockfileGenerator#generate!`.
    pub fn write(&self) -> String {
        let mut out = String::new();
        for (i, source) in self.sources.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            source.kind.to_lock(&mut out);
            let mut specs: Vec<&LockedSpec> = source
                .specs
                .iter()
                .filter(|s| s.name != "bundler")
                .collect();
            specs.sort_by_cached_key(|s| s.full_name());
            for spec in specs {
                out.push_str(&format!("    {}\n", spec.lock_name()));
                let mut deps: Vec<&(String, Requirement)> = spec.dependencies.iter().collect();
                deps.sort_by_cached_key(|(name, req)| dependency_to_s(name, req));
                for (name, req) in deps {
                    out.push_str(&format!("      {}\n", dependency_to_lock(name, req)));
                }
            }
        }

        let platforms: Vec<String> = self.platforms.iter().map(ToString::to_string).collect();
        section(&mut out, "PLATFORMS", platforms);

        out.push_str("\nDEPENDENCIES\n");
        let mut deps: Vec<&Dependency> = self.dependencies.iter().collect();
        deps.sort_by_cached_key(|d| dependency_to_s(&d.name, &d.requirement));
        let mut handled: HashSet<&str> = HashSet::new();
        for dep in deps {
            if handled.insert(&dep.name) {
                let pin = if dep.pinned { "!" } else { "" };
                out.push_str(&format!(
                    "  {}{pin}\n",
                    dependency_to_lock(&dep.name, &dep.requirement)
                ));
            }
        }

        if let Some(checksums) = &self.checksums {
            section(&mut out, "CHECKSUMS", self.checksum_lines(checksums));
        }
        if let Some(ruby) = &self.ruby_version {
            section(&mut out, "RUBY VERSION", vec![ruby.clone()]);
        }
        let bundled_with = self
            .bundled_with
            .as_ref()
            .map_or_else(|| BUNDLER_VERSION.to_owned(), ToString::to_string);
        section(&mut out, "BUNDLED WITH", vec![bundled_with]);
        out
    }

    /// `add_checksums`: a line for every locked spec, with its checksums
    /// when the lockfile has them, and `bundler`'s when it has one.
    fn checksum_lines(&self, checksums: &[Checksum]) -> Vec<String> {
        let line = |lock_name: &str| {
            let mut sums: Vec<String> = checksums
                .iter()
                .find(|c| c.lock_name == lock_name)
                .map(|c| {
                    c.checksums
                        .iter()
                        .map(|(algo, digest)| format!("{algo}={digest}"))
                        .collect()
                })
                .unwrap_or_default();
            sums.sort();
            if sums.is_empty() {
                lock_name.to_owned()
            } else {
                format!("{lock_name} {}", sums.join(","))
            }
        };
        let mut lines: Vec<String> = self
            .sources
            .iter()
            .flat_map(|s| &s.specs)
            .filter(|s| s.name != "bundler")
            .map(|s| line(&s.lock_name()))
            .collect();
        lines.extend(
            checksums
                .iter()
                .filter(|c| c.lock_name.starts_with("bundler ("))
                .map(|c| line(&c.lock_name)),
        );
        lines
    }
}

/// `add_section` with an Array: each value, sorted, indented.
fn section(out: &mut String, name: &str, mut values: Vec<String>) {
    out.push('\n');
    out.push_str(name);
    out.push('\n');
    values.sort();
    for value in values {
        out.push_str("  ");
        out.push_str(&value);
        out.push('\n');
    }
}

// ---------------------------------------------------------------------------
// A new lockfile
// ---------------------------------------------------------------------------

/// A git or path source as a lockfile writes it: its section's options,
/// `identifier` (what `SourceList#lock_other_sources` sorts by), the gems
/// whose specs it has (`SourceMap#all_requirements`), and the Gemfile gems
/// pinned to it, `!` in `DEPENDENCIES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockSource {
    pub kind: SourceKind,
    pub identifier: String,
    pub names: Vec<String>,
    pub pinned: Vec<String>,
}

/// The remotes of the `GEM` section for the rubygems source at `remote`:
/// no credentials, a trailing `/`. An empty `remote` is a Gemfile with no
/// rubygems source, whose `GEM` section Bundler writes with no remote.
pub(crate) fn gem_remotes(remote: &str) -> Vec<String> {
    if remote.is_empty() {
        return Vec::new();
    }
    let mut remote = credless(remote);
    if !remote.ends_with('/') {
        remote.push('/');
    }
    vec![remote]
}

/// `SourceList#lock_sources`: the git and path sources sorted by identifier,
/// then the rubygems sources sorted by theirs: the one at `remote` (see
/// [`gem_remotes`]) and those `locals` scopes some gems to (#89), each with
/// the specs of the gems it provides, the one at `remote` the rest.
pub(crate) fn lock_sources(
    remote: &str,
    locals: &[LockSource],
    specs: Vec<LockedSpec>,
) -> Vec<Source> {
    let (gems, others): (Vec<&LockSource>, Vec<&LockSource>) = locals
        .iter()
        .partition(|l| matches!(l.kind, SourceKind::Gem { .. }));
    let mut others = others;
    others.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    let global = LockSource {
        kind: SourceKind::Gem {
            remotes: gem_remotes(remote),
        },
        identifier: rubygems_identifier(remote),
        names: Vec::new(),
        pinned: Vec::new(),
    };
    let mut rubygems: Vec<&LockSource> = gems;
    rubygems.push(&global);
    rubygems.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    let order: Vec<&LockSource> = others.iter().chain(&rubygems).copied().collect();
    let mut sources: Vec<Source> = order
        .iter()
        .map(|l| Source {
            kind: l.kind.clone(),
            specs: Vec::new(),
        })
        .collect();
    let global_at = order
        .iter()
        .position(|l| std::ptr::eq(*l, &global))
        .expect("the global source is listed");
    for spec in specs {
        match order.iter().position(|l| l.names.contains(&spec.name)) {
            Some(at) => sources[at].specs.push(spec),
            None => sources[global_at].specs.push(spec),
        }
    }
    sources
}

/// `Source::Rubygems#identifier` of the source at `remote`, as written,
/// with the trailing `/` Bundler gives it: what lockfile sections are
/// sorted by.
pub(crate) fn rubygems_identifier(remote: &str) -> String {
    if remote.is_empty() {
        return "locally installed gems".to_owned();
    }
    if remote.ends_with('/') {
        format!("rubygems repository {remote}")
    } else {
        format!("rubygems repository {remote}/")
    }
}

/// The `DEPENDENCIES` of a Gemfile's dependencies, `!` on those pinned to
/// a source of their own (`Dependency#to_lock`).
pub(crate) fn lock_dependencies(
    dependencies: &[resolver::GemfileDependency],
    locals: &[LockSource],
) -> Vec<Dependency> {
    dependencies
        .iter()
        .map(|d| Dependency {
            name: d.name.clone(),
            requirement: d.requirement.clone(),
            pinned: locals.iter().any(|l| l.pinned.contains(&d.name)),
        })
        .collect()
}

/// `Source::Rubygems#remove_auth`: a remote without its user and password.
pub(crate) fn credless(remote: &str) -> String {
    let Some((scheme, rest)) = remote.split_once("://") else {
        return remote.to_owned();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => remote.to_owned(),
    }
}

/// The specs a set has of one gem, as `SpecSet#lookup[name]` holds them.
fn named<'a>(set: &'a [Spec], name: &'a str) -> impl Iterator<Item = &'a Spec> + 'a {
    set.iter().filter(move |s| s.name == name)
}

/// `MatchPlatform.select_best_platform_match` over `candidates`, in their
/// order; `force_ruby` takes the `ruby` build only.
fn select_best_platform_match(
    candidates: &[Spec],
    platform: &GemPlatform,
    force_ruby: bool,
) -> Vec<Spec> {
    let target = if force_ruby {
        &GemPlatform::Ruby
    } else {
        platform
    };
    let all: Vec<usize> = (0..candidates.len()).collect();
    resolver::best_platform_match(&all, candidates, target)
        .into_iter()
        .map(|i| candidates[i].clone())
        .collect()
}

/// What `SpecSet#add_extra_platforms!` and `SpecSet#for` read of the
/// resolution: the Ruby and RubyGems resolved for, and the index the
/// resolution's builds came from.
struct Shaping<'a> {
    request: &'a Request,
    index: &'a Index,
}

impl Shaping<'_> {
    /// `SpecSet#valid?`: the build's Ruby and RubyGems requirements hold,
    /// and each of its dependencies but `bundler` names a gem the set has a
    /// version of that satisfies it.
    fn valid(&self, spec: &Spec, set: &[Spec]) -> bool {
        spec.required_ruby_version
            .satisfied_by(&self.request.ruby_version)
            && spec
                .required_rubygems_version
                .satisfied_by(&self.request.rubygems_version)
            && spec.dependencies.iter().all(|(name, req)| {
                name == "bundler" || named(set, name).any(|s| req.satisfied_by(&s.version))
            })
    }

    /// `Index#search([name, version])`: every build of a version.
    fn builds(&self, name: &str, version: &Version) -> Vec<Spec> {
        self.index
            .get(name)
            .map(|specs| {
                specs
                    .iter()
                    .filter(|s| s.version == *version)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `SpecSet#complete_platform`: whether every gem of the set has a
    /// build of its version for `platform` that is valid with what the set
    /// has; if so the builds it lacks are added.
    fn complete_platform(&self, set: &mut Vec<Spec>, platform: &GemPlatform) -> bool {
        let mut names: Vec<&str> = Vec::new();
        for spec in set.iter() {
            if !names.contains(&spec.name.as_str()) {
                names.push(&spec.name);
            }
        }
        let mut new_specs: Vec<Spec> = Vec::new();
        for name in names {
            let first = named(set, name).next().expect("named from the set");
            let matching = self.builds(name, &first.version);
            let best = select_best_platform_match(&matching, platform, false);
            let Some(build) = best.into_iter().find(|s| self.valid(s, set)) else {
                return false;
            };
            if !named(set, name).any(|s| s.full_name() == build.full_name()) {
                new_specs.push(build);
            }
        }
        set.extend(new_specs);
        true
    }

    /// `SpecSet#add_extra_platforms!`.
    fn add_extra_platforms(&self, set: &mut Vec<Spec>, platforms: &mut Vec<GemPlatform>) {
        if set.is_empty() {
            if !platforms.contains(&GemPlatform::Ruby) {
                platforms.push(GemPlatform::Ruby);
            }
            return;
        }
        let local = platforms[0].clone();
        let mut all_platforms: Vec<GemPlatform> = Vec::new();
        for spec in set.iter() {
            for build in self.builds(&spec.name, &spec.version) {
                if !all_platforms.contains(&build.platform) {
                    all_platforms.push(build.platform);
                }
            }
        }
        let mut new_platforms: Vec<GemPlatform> = Vec::new();
        for platform in all_platforms {
            if platforms.contains(&platform)
                || gem_platform::generic(&platform) != GemPlatform::Ruby
            {
                continue;
            }
            if self.complete_platform(set, &platform) {
                new_platforms.push(platform);
            }
        }
        if new_platforms.is_empty() {
            return;
        }
        platforms.extend(new_platforms.iter().cloned());
        if new_platforms.contains(&local) {
            return;
        }
        // `local === platform && platform === local`: the same machine,
        // named more specifically.
        let less_specific = new_platforms
            .iter()
            .any(|platform| match (&local, platform) {
                (GemPlatform::Platform(l), GemPlatform::Platform(p)) => {
                    l.matches(p) && p.matches(l)
                }
                _ => false,
            });
        if less_specific {
            platforms.retain(|p| *p != local);
        }
    }
}

/// `SpecSet#for(dependencies, platforms)`: the builds of the set each
/// platform installs, from the Gemfile's dependencies down.
pub(crate) fn spec_set_for(
    set: &[Spec],
    dependencies: &[resolver::GemfileDependency],
    platforms: &[GemPlatform],
) -> Vec<Spec> {
    materialize_dependencies(set, dependencies, platforms).0
}

/// `SpecSet#incomplete_for_platform?`: whether some gem the Gemfile's
/// dependencies reach has no build in the set for `platform`.
pub(crate) fn incomplete_for_platform(
    set: &[Spec],
    dependencies: &[resolver::GemfileDependency],
    platform: &GemPlatform,
) -> bool {
    !set.is_empty() && materialize_dependencies(set, dependencies, std::slice::from_ref(platform)).1
}

/// `SpecSet#materialize_dependencies`: the builds, and whether a gem was
/// reached that has none for its platform.
fn materialize_dependencies(
    set: &[Spec],
    dependencies: &[resolver::GemfileDependency],
    platforms: &[GemPlatform],
) -> (Vec<Spec>, bool) {
    let mut handled: HashSet<(String, String)> = platforms
        .iter()
        .map(|p| ("bundler".to_owned(), p.to_string()))
        .collect();
    let mut queue: std::collections::VecDeque<(String, bool, GemPlatform)> = dependencies
        .iter()
        .flat_map(|d| {
            platforms
                .iter()
                .map(|p| (d.name.clone(), d.force_ruby_platform, p.clone()))
        })
        .collect();
    let mut out: Vec<Spec> = Vec::new();
    let mut incomplete = false;
    while let Some((name, force_ruby, platform)) = queue.pop_front() {
        if !handled.insert((name.clone(), platform.to_string())) {
            continue;
        }
        let candidates: Vec<Spec> = named(set, &name).cloned().collect();
        let specs = select_best_platform_match(&candidates, &platform, force_ruby);
        match specs.first() {
            Some(first) => {
                for (dep, _) in &first.dependencies {
                    queue.push_back((dep.clone(), false, platform.clone()));
                }
            }
            None => incomplete = true,
        }
        for spec in specs {
            if !out.iter().any(|s| s.full_name() == spec.full_name()) {
                out.push(spec);
            }
        }
    }
    (out, incomplete)
}

impl Lockfile {
    /// The lockfile `bundle lock` writes when there is none yet, for a
    /// resolution of `request` against `index` from one rubygems source at
    /// `remote`. The first of the request's platforms is the local one.
    /// `ruby_directive` is whether the Gemfile has a `ruby` line, which
    /// locks the Ruby resolved for.
    pub fn from_resolution(
        remote: &str,
        request: &Request,
        index: &Index,
        resolved: &[Resolved],
        ruby_directive: bool,
        locals: &[LockSource],
    ) -> Lockfile {
        let shaping = Shaping { request, index };
        let mut set: Vec<Spec> = resolved
            .iter()
            .filter_map(|r| {
                index
                    .get(&r.name)?
                    .iter()
                    .find(|s| s.version.as_str() == r.version.as_str() && s.platform == r.platform)
            })
            .cloned()
            .collect();
        let mut platforms: Vec<GemPlatform> = Vec::new();
        for platform in &request.platforms {
            if !platforms.contains(platform) {
                platforms.push(platform.clone());
            }
        }
        // `should_add_extra_platforms?`: a lockfile that does not exist yet,
        // on a machine whose generic platform is `ruby`.
        if platforms
            .first()
            .is_some_and(|local| gem_platform::generic(local) == GemPlatform::Ruby)
        {
            shaping.add_extra_platforms(&mut set, &mut platforms);
        }
        let mut materialize_for = platforms.clone();
        if !materialize_for.contains(&GemPlatform::Ruby) {
            materialize_for.push(GemPlatform::Ruby);
        }
        let specs = spec_set_for(&set, &request.dependencies, &materialize_for);

        let checksums = specs
            .iter()
            .map(|s| Checksum {
                lock_name: lock_name(&s.name, &s.version, &s.platform),
                checksums: s
                    .checksum
                    .iter()
                    .map(|c| ("sha256".to_owned(), c.clone()))
                    .collect(),
            })
            .collect();
        let locked: Vec<LockedSpec> = specs
            .into_iter()
            .map(|s| LockedSpec {
                name: s.name,
                version: s.version,
                platform: s.platform,
                dependencies: s.dependencies,
            })
            .collect();
        Lockfile {
            sources: lock_sources(remote, locals, locked),
            platforms,
            dependencies: lock_dependencies(&request.dependencies, locals),
            checksums: Some(checksums),
            // `RubyVersion.system`: the engine's version, for `ruby`, is the
            // Ruby version; the lockfile never names Spinel.
            ruby_version: ruby_directive.then(|| format!("ruby {}", request.ruby_version)),
            bundled_with: Some(Version::new(BUNDLER_VERSION).expect("a version")),
        }
    }
}

#[cfg(test)]
mod tests;
