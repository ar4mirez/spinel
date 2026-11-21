//! The resolver (#78, docs/package-manager.md, "Resolver"): a Gemfile's
//! dependencies and a compact index in, the gems to install out, chosen as
//! Bundler 4 chooses them, or an explanation of why there are none.
//!
//! Bundler resolves with PubGrub, its vendored pubgrub-ruby 0.5.0, and so
//! does this, with a port of it (`resolver/solver/`, #816), so that what it
//! derives, and so what it relaxes and how it explains a conflict, are
//! Bundler's by construction. What makes the choices Bundler's is the model
//! fed to it, which follows `Bundler::Resolver` and its `Package`,
//! `Candidate`, `SpecGroup` and `Strategy` step for step, and reaches the
//! solver through Bundler's source side (`resolver/source.rs`:
//! `incompatibilities_for`, `Strategy`, `find_names_to_relax`):
//!
//! - **Candidates.** A version of a gem is up to two candidates: its `ruby`
//!   build (priority -1) and its builds for the resolution platforms
//!   (priority 1), each a group of `/info` lines. The platform group is left
//!   out when it is the ruby group, and a platform build with the ruby
//!   build's dependencies joins the ruby group instead. Candidates order by
//!   version, then priority, so `= 1.0` covers both builds of 1.0 and `> 1.0`
//!   neither. A version a top-level gem has no build of for some resolution
//!   platform is not a candidate; any other gem needs a build for one.
//! - **Platform.** For each platform, the builds `Gem::Platform` says install
//!   there, narrowed to the best match as `sort_and_filter_best_platform_match`
//!   narrows them.
//! - **`required_ruby_version` and `required_rubygems_version`** are
//!   dependencies on two meta packages, `Ruby\0` and `RubyGems\0`, each with
//!   one version, the one being resolved for; the root depends on exactly
//!   those. So a gem whose Ruby requirement fails is ruled out by the solver
//!   like any other conflict, and says so in the explanation ("current Ruby
//!   version is = 2.6.10").
//! - **Prereleases** are left out of a gem's candidates unless its Gemfile
//!   requirement names one, or it has no other version. When a resolution
//!   fails through a gem with prereleases left out, along the failure's
//!   first causes as Bundler follows them (`find_names_to_relax`), it is
//!   retried with them in. Against Bundler 4.0.20 this matches on the hand
//!   cases in `tests/fixtures/resolver/oracle/` and on every random index of
//!   seeds 1 (400), 816 (1000) and 4242 (1000), each try relaxing the gems
//!   Bundler's relaxes (#816).
//! - **Order.** The next gem decided is the one with at most one candidate
//!   left, then the one with the fewest candidates above the range asked
//!   for, ties going to the one found first; its candidate is the newest,
//!   releases before prereleases unless prereleases were asked for, a
//!   platform build before the ruby build of the same version.
//!
//! A conflict is explained as Bundler explains it: pubgrub-ruby's
//! `FailureWriter`, ported in `resolver/report.rs` over the solver's own
//! incompatibilities.
//!
//! `bundler` has one candidate, the Bundler version the lockfile is written
//! for (`lockfile::BUNDLER_VERSION`), as Bundler's `Source::Metadata` gives
//! it only the version running; no index is read for it, and no resolution
//! lists it.
//!
//! **Locked versions** (#79) come in as a [`Locked`], what
//! `Definition#resolution_base` hands `Bundler::Resolver::Base` when a
//! lockfile exists and no update was asked for (`crate::locked` works it
//! out). Bundler 4 does not pin an unchanged gem to its locked version: it
//! gives every locked gem but the Gemfile's changed ones a floor of `>=` the
//! locked version (`additional_base_requirements_to_prevent_downgrades`),
//! tries the locked version first (`GemVersionPromoter#post_sort`), and
//! resolves the locked builds still good for the Gemfile from the lockfile
//! itself, its dependencies and its platform builds before the index's
//! (`@base`, `prefer_locked`). A locked prerelease lets that gem's
//! prereleases in. A gem whose floor is in a conflict loses it, and the
//! resolution is tried again (`find_names_to_relax`, `Base#unlock_names`).
//!
//! Git and path sources (#88) come in as builds in the [`Index`] under the
//! names they provide, in place of the index's (`crate::local_source`).
//! Not here yet: a rubygems source scoped to a gem.

mod report;
mod solver;
mod source;

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::gem_platform::{self, GemPlatform, Platform};
use crate::gem_requirement::{Op, Requirement};
use crate::gem_version::Version;

/// One `/info/<gem>` line: a build of one version of a gem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub name: String,
    pub version: Version,
    pub platform: GemPlatform,
    /// Runtime dependencies, as listed.
    pub dependencies: Vec<(String, Requirement)>,
    /// `>= 0` when the line has no `ruby:`, as `Gem::Specification`'s.
    pub required_ruby_version: Requirement,
    /// `>= 0` when the line has no `rubygems:`.
    pub required_rubygems_version: Requirement,
    /// The `checksum:` field: the SHA-256 of the `.gem`, as a lockfile's
    /// `CHECKSUMS` records it.
    pub checksum: Option<String>,
}

impl Spec {
    /// `full_name`: `name-version`, and `-platform` unless it is `ruby`.
    pub fn full_name(&self) -> String {
        match &self.platform {
            GemPlatform::Ruby => format!("{}-{}", self.name, self.version),
            platform => format!("{}-{}-{platform}", self.name, self.version),
        }
    }

    /// `expanded_dependencies`: the runtime dependencies, then `Ruby\0` and
    /// `RubyGems\0` for requirements that are not `>= 0`.
    fn expanded_dependencies(&self) -> Vec<(PackageName, Requirement)> {
        let mut deps: Vec<(PackageName, Requirement)> = self
            .dependencies
            .iter()
            .map(|(name, req)| (PackageName::Gem(name.clone()), req.clone()))
            .collect();
        for (meta, req) in [
            (PackageName::Ruby, &self.required_ruby_version),
            (PackageName::RubyGems, &self.required_rubygems_version),
        ] {
            if !req.none() {
                deps.push((meta, req.clone()));
            }
        }
        deps
    }
}

/// A line of `/info/<gem>` RubyGems' `GemParser` would not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoError {
    pub gem: String,
    pub line: String,
    pub reason: String,
}

impl fmt::Display for InfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "info/{}: cannot read {:?}: {}",
            self.gem, self.line, self.reason
        )
    }
}

impl std::error::Error for InfoError {}

/// Ruby's `split(" ", 2)`: leading whitespace skipped, then the first word
/// and the rest without the whitespace before it.
fn split_word(line: &str) -> (&str, &str) {
    let line = line.trim_start_matches(|c: char| c.is_ascii_whitespace());
    match line.find(|c: char| c.is_ascii_whitespace()) {
        Some(end) => (
            &line[..end],
            line[end..].trim_start_matches(|c: char| c.is_ascii_whitespace()),
        ),
        None => (line, ""),
    }
}

/// Ruby's `split(",")`: trailing empty fields dropped.
fn split_fields(s: &str) -> Vec<&str> {
    ruby_split(s, ',')
}

/// Ruby's `String#split(sep)` for a one-character `sep`: trailing empty fields
/// are dropped, so `""` splits to `[]` and `"a&&"` to `["a"]`, while an empty
/// field before a non-empty one is kept (`"&a"` is `["", "a"]`).
fn ruby_split(s: &str, sep: char) -> Vec<&str> {
    let mut fields: Vec<&str> = s.split(sep).collect();
    while fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields
}

/// `/info/<gem>`, as `Gem::Resolver::APISet::GemParser` reads each line after
/// the `---` header: `version[-platform] dep:req&req,dep:req|key:value,...`.
/// Of the keys, `ruby`, `rubygems` and `checksum` are read; `created_at` is
/// Bundler's cooldown's. Each value is split on `&` as `GemParser` does, with
/// Ruby's `split`, so an empty value (`rubygems:`, served by rubygems.org for
/// `archive-tar-minitar` 0.5.1) is `[]`: `Gem::Requirement.new([])`, `>= 0`,
/// for a requirement, and no checksum, as `Bundler::EndpointSpecification
/// #parse_metadata` reads `checksum` as the last part.
pub fn parse_info(gem: &str, data: &[u8], local: &Platform) -> Result<Vec<Spec>, InfoError> {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<&str> = text.split('\n').collect();
    let body = match lines.iter().position(|line| *line == "---") {
        Some(header) => &lines[header + 1..],
        None => &lines[..],
    };
    let mut specs = Vec::new();
    for line in body {
        if line.trim().is_empty() {
            continue;
        }
        let error = |reason: String| InfoError {
            gem: gem.to_owned(),
            line: (*line).to_owned(),
            reason,
        };
        let (version_and_platform, rest) = split_word(line);
        let (version, platform) = match version_and_platform.split_once('-') {
            Some((version, platform)) => (version, Some(platform)),
            None => (version_and_platform, None),
        };
        let version = Version::new(version).map_err(|e| error(e.to_string()))?;
        let (dependencies, requirements) = match rest.split_once('|') {
            Some((deps, reqs)) => (split_fields(deps), split_fields(reqs)),
            None => (split_fields(rest), Vec::new()),
        };
        let parse = |field: &str| -> Result<(String, Requirement), InfoError> {
            match field.split_once(':') {
                Some((name, reqs)) => Requirement::new(ruby_split(reqs, '&'))
                    .map(|req| (name.to_owned(), req))
                    .map_err(|e| error(e.to_string())),
                None => Ok((field.to_owned(), Requirement::default())),
            }
        };
        let dependencies = dependencies
            .into_iter()
            .map(parse)
            .collect::<Result<Vec<_>, _>>()?;
        let mut spec = Spec {
            name: gem.to_owned(),
            version,
            platform: GemPlatform::new(platform, local),
            dependencies,
            required_ruby_version: Requirement::default(),
            required_rubygems_version: Requirement::default(),
            checksum: None,
        };
        for field in requirements {
            let Some((key, value)) = field.split_once(':') else {
                continue;
            };
            let parts = ruby_split(value, '&');
            let read = || Requirement::new(parts.iter().copied()).map_err(|e| error(e.to_string()));
            match key {
                "ruby" => spec.required_ruby_version = read()?,
                "rubygems" => spec.required_rubygems_version = read()?,
                "checksum" => spec.checksum = parts.last().map(|sum| (*sum).to_owned()),
                _ => {}
            }
        }
        specs.push(spec);
    }
    Ok(specs)
}

/// Every gem any of `specs` depends on: what must be read next to resolve.
pub fn dependency_names(specs: &[Spec]) -> impl Iterator<Item = &str> {
    specs
        .iter()
        .flat_map(|s| s.dependencies.iter().map(|(name, _)| name.as_str()))
}

/// Every gem's builds, by name, as the index lists them.
pub type Index = HashMap<String, Vec<Spec>>;

/// A `gem` line of the Gemfile, as `shims/gemfile_eval.rb` reports it.
#[derive(Debug, Clone)]
pub struct GemfileDependency {
    pub name: String,
    pub requirement: Requirement,
    /// Bundler's platform names (`ruby`, `jruby`, `windows`, `mri_33`, ...).
    pub platforms: Vec<String>,
    pub force_ruby_platform: bool,
}

/// A lockfile's hold on a resolution (#79): `Resolver::Base` as
/// `Definition#resolution_base` makes it. The default holds nothing, which is
/// a resolution with no lockfile.
#[derive(Debug, Clone, Default)]
pub struct Locked {
    /// `Package#locked_version`: each locked gem's version.
    pub versions: HashMap<String, Version>,
    /// `@base`: the locked builds still good for the Gemfile, as the
    /// lockfile has them (its dependencies, no Ruby requirement), in the
    /// order `SpecSet#for` found them.
    pub base: Vec<Spec>,
    /// `base_requirements`: `= version` for each gem of `base`, then
    /// `>= version` for every locked gem whose Gemfile dependency did not
    /// change.
    pub requirements: HashMap<String, Requirement>,
    /// `@gems_to_unlock`: gems the lockfile does not hold, whose locked
    /// version is neither tried first nor its builds preferred.
    pub unlock: HashSet<String>,
    /// `unlock: true`, as `bundle update` with no gems (`@unlocking_all`)
    /// and its full update for the gems named (`unlocked_resolution_base`)
    /// pass it: every gem is unlocked.
    pub unlock_all: bool,
    /// Resolution platforms the lockfile does not have, on which locked
    /// builds are not preferred (`new_platforms`).
    pub new_platforms: Vec<GemPlatform>,
    /// `@base.locked_specs`: every locked build, for
    /// `SpecSet#specs_with_additional_variants_from`.
    pub locked_specs: Vec<Spec>,
}

impl Locked {
    /// `Base#unlock_names`: the gems in a conflict with their requirement.
    /// Of those pinned exactly that the Gemfile does not name, or else of
    /// all pinned exactly, the pin becomes a floor and the locked builds go
    /// from the base; with none pinned, each requirement goes.
    fn unlock_names(&mut self, names: &[String], gemfile: &[&str]) {
        let exact = |locked: &Locked, name: &String| {
            locked
                .requirements
                .get(name)
                .is_some_and(|r| matches!(r.requirements(), [(Op::Eq, _)]))
        };
        let indirect: Vec<&String> = names
            .iter()
            .filter(|n| exact(self, n) && !gemfile.contains(&n.as_str()))
            .collect();
        let pins: Vec<&String> = if indirect.is_empty() {
            names.iter().filter(|n| exact(self, n)).collect()
        } else {
            indirect
        };
        if pins.is_empty() {
            for name in names {
                self.requirements.remove(name);
            }
            return;
        }
        for name in pins {
            let version = self.requirements[name].requirements()[0].1.clone();
            let floor = Requirement::new([format!(">= {version}").as_str()])
                .expect("a version makes a floor");
            self.requirements.insert(name.clone(), floor);
            self.base.retain(|s| &s.name != name);
        }
    }
}

/// What to resolve, and for what.
#[derive(Debug, Clone)]
pub struct Request {
    /// The root's name in explanations: the Gemfile's base name.
    pub gemfile: String,
    /// The source, as explanations name it.
    pub source: String,
    pub dependencies: Vec<GemfileDependency>,
    /// The resolution platforms (the lockfile's `PLATFORMS`, or the local one).
    pub platforms: Vec<GemPlatform>,
    /// The Ruby resolved for: `Ruby\0`'s one version.
    pub ruby_version: Version,
    /// The RubyGems resolved for: `RubyGems\0`'s one version.
    pub rubygems_version: Version,
}

/// One gem build chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub version: Version,
    pub platform: GemPlatform,
}

impl Resolved {
    /// `full_name`: `name-version`, and `-platform` unless it is `ruby`.
    pub fn full_name(&self) -> String {
        match &self.platform {
            GemPlatform::Ruby => format!("{}-{}", self.name, self.version),
            platform => format!("{}-{}-{platform}", self.name, self.version),
        }
    }
}

/// As a lockfile's `specs:` lists it: `rack (3.2.8)`, `json (3.1.0-java)`.
impl fmt::Display for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.platform {
            GemPlatform::Ruby => write!(f, "{} ({})", self.name, self.version),
            platform => write!(f, "{} ({}-{platform})", self.name, self.version),
        }
    }
}

/// Why there is no resolution, worded as Bundler words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// A Gemfile dependency no version of the gem satisfies
    /// (`Bundler::GemNotFound`).
    NotFound(String),
    /// The requirements conflict (`Bundler::SolveFailure`): the solver's
    /// derivation, written as pubgrub-ruby's `FailureWriter` writes it.
    Conflict(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::NotFound(message) | Failure::Conflict(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Failure {}

// ---------------------------------------------------------------------------
// The model: packages and candidates.
// ---------------------------------------------------------------------------

/// A package's name: a gem, or one of the meta packages for Ruby and
/// RubyGems (`Ruby\0`, `RubyGems\0`, as Bundler names them).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PackageName {
    Gem(String),
    Ruby,
    RubyGems,
}

impl PackageName {
    /// The name as Bundler spells it: a gem's, `Ruby\0` or `RubyGems\0`.
    fn bundler_name(&self) -> String {
        match self {
            PackageName::Gem(name) => name.clone(),
            PackageName::Ruby => "Ruby\0".to_owned(),
            PackageName::RubyGems => "RubyGems\0".to_owned(),
        }
    }
}

/// What the solver resolves: the root (the Gemfile) or a [`PackageName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Package {
    Root(String),
    Name(PackageName),
}

impl Package {
    fn is_meta(&self) -> bool {
        matches!(
            self,
            Package::Name(PackageName::Ruby | PackageName::RubyGems)
        )
    }
}

/// `Package.root?`, for pubgrub-ruby's solver.
impl solver::Package for Package {
    fn is_root(&self) -> bool {
        matches!(self, Package::Root(_))
    }
}

/// `Package#to_s`: the name without the `\0`; the root by its Gemfile.
impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Package::Root(gemfile) => f.write_str(gemfile),
            Package::Name(PackageName::Gem(name)) => f.write_str(name),
            Package::Name(PackageName::Ruby) => f.write_str("Ruby"),
            Package::Name(PackageName::RubyGems) => f.write_str("RubyGems"),
        }
    }
}

/// `Bundler::Resolver::Candidate`: a version and whether it stands for the
/// `ruby` build (-1) or the platform builds (1).
#[derive(Debug, Clone)]
pub struct Candidate {
    version: Version,
    priority: i8,
}

impl Candidate {
    fn new(version: Version, priority: i8) -> Candidate {
        Candidate { version, priority }
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Candidate) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

/// `hash`: the version's and the priority's, so candidates `==` hash alike.
impl std::hash::Hash for Candidate {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.version.hash(state);
        self.priority.hash(state);
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Candidate) -> Ordering {
        self.version
            .cmp(&other.version)
            .then(self.priority.cmp(&other.priority))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Candidate) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The version alone, as Bundler prints a candidate.
impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.version.fmt(f)
    }
}

/// A set of candidates, as pubgrub-ruby's ranges hold them.
type VersionSet = solver::RangeSet<Candidate>;

/// `to_dependency_hash`: each dependency as its package and range, the
/// ranges on one package intersected, in the order first seen.
fn dependency_hash<R>(
    deps: impl IntoIterator<Item = (Package, R)>,
    intersect: impl Fn(&R, &R) -> R,
) -> Vec<(Package, R)> {
    let mut out: Vec<(Package, R)> = Vec::new();
    for (package, range) in deps {
        match out.iter_mut().find(|(p, _)| *p == package) {
            Some((_, existing)) => *existing = intersect(existing, &range),
            None => out.push((package, range)),
        }
    }
    out
}

/// How a package treats prereleases (`Package#ignores_prereleases?` and
/// `#prerelease_specified?`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prereleases {
    Ignore,
    ConsiderFirst,
    ConsiderLast,
}

/// `Resolver::SpecGroup`: the builds a candidate stands for.
#[derive(Debug, Clone)]
struct SpecGroup {
    /// Indices into the package's builds.
    specs: Vec<usize>,
    /// `dependencies`: every build's expanded dependencies, `uniq`, sorted
    /// by name.
    dependencies: Vec<(PackageName, Requirement)>,
}

impl SpecGroup {
    fn new(specs: Vec<usize>, all: &[Spec]) -> SpecGroup {
        let mut dependencies: Vec<(PackageName, Requirement)> = Vec::new();
        for &i in &specs {
            for dep in all[i].expanded_dependencies() {
                if !dependencies.contains(&dep) {
                    dependencies.push(dep);
                }
            }
        }
        // `Gem::Dependency#<=>` compares names; `sort` keeps nothing else in
        // order, and the order within one name changes nothing (the ranges
        // are intersected).
        dependencies.sort_by(|a, b| dependency_sort_key(&a.0).cmp(dependency_sort_key(&b.0)));
        SpecGroup {
            specs,
            dependencies,
        }
    }

    /// `sorted_spec_names`, what `==` compares.
    fn sorted_names(&self, all: &[Spec]) -> Vec<String> {
        let mut names: Vec<String> = self.specs.iter().map(|&i| all[i].full_name()).collect();
        names.sort();
        names
    }

    /// `merge`: take `other`'s builds when the two are `equivalent?` (one
    /// gem, one version, one source, the same dependencies).
    fn merge(&mut self, other: &SpecGroup, all: &[Spec]) {
        if self.specs.is_empty() || other.specs.is_empty() {
            return;
        }
        let (a, b) = (&all[self.specs[0]], &all[other.specs[0]]);
        if a.name != b.name || a.version != b.version || self.dependencies != other.dependencies {
            return;
        }
        for &i in &other.specs {
            if !self.specs.contains(&i) {
                self.specs.push(i);
            }
        }
    }
}

/// How Ruby sorts the dependency names: `Ruby\0` and `RubyGems\0` are
/// strings like any other, and capitals sort first.
fn dependency_sort_key(name: &PackageName) -> &str {
    match name {
        PackageName::Gem(name) => name,
        PackageName::Ruby => "Ruby\0",
        PackageName::RubyGems => "RubyGems\0",
    }
}

/// `MatchPlatform#installable_on_platform?`: `ruby` installs anywhere, a
/// platform where `Gem::Platform#===` says, and nothing but `ruby` on `ruby`.
fn installable_on(spec: &GemPlatform, target: &GemPlatform) -> bool {
    match (spec, target) {
        (GemPlatform::Ruby, _) => true,
        (s, t) if s == t => true,
        (GemPlatform::Platform(s), GemPlatform::Platform(t)) => s.matches(t),
        _ => false,
    }
}

/// `MatchPlatform.select_best_platform_match`: the builds that install on
/// `platform`, narrowed by `Gem::Platform.sort_and_filter_best_platform_match`
/// to the exact match, or else to the best-ranked ones with the same
/// requirements as the best.
pub(crate) fn best_platform_match(
    specs: &[usize],
    all: &[Spec],
    platform: &GemPlatform,
) -> Vec<usize> {
    let mut matching: Vec<usize> = specs
        .iter()
        .copied()
        .filter(|&i| installable_on(&all[i].platform, platform))
        .collect();
    if matching.len() <= 1 {
        return matching;
    }
    let exact: Vec<usize> = matching
        .iter()
        .copied()
        .filter(|&i| &all[i].platform == platform)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    // `sort_best_platform_match`: a stable sort by specificity.
    let score = |i: usize| gem_platform::platform_specificity_match(&all[i].platform, platform);
    matching.sort_by_key(|&i| score(i));
    let best = &all[matching[0]];
    let best_score = score(matching[0]);
    let sorted_deps = |s: &Spec| {
        let mut deps = s.dependencies.clone();
        deps.sort_by(|a, b| a.0.cmp(&b.0));
        deps
    };
    let best_deps = sorted_deps(best);
    matching
        .into_iter()
        .take_while(|&i| {
            let s = &all[i];
            score(i) == best_score
                && s.required_ruby_version == best.required_ruby_version
                && s.required_rubygems_version == best.required_rubygems_version
                && sorted_deps(s) == best_deps
        })
        .collect()
}

/// `Bundler::Dependency#gem_platforms`'s `expanded_platforms`: the generic
/// platforms a Gemfile platform name stands for (`CurrentRuby::PLATFORM_MAP`),
/// `_33` and the like meaning the same as the name without them.
fn expanded_platforms(names: &[String]) -> Vec<GemPlatform> {
    let mut out = Vec::new();
    for name in names {
        let base = match name.rsplit_once('_') {
            Some((base, digits))
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                base
            }
            _ => name.as_str(),
        };
        let generics: &[&str] = match base {
            "ruby" | "mri" | "rbx" | "truffleruby" => &["ruby"],
            "jruby" => &["java"],
            "windows" => &["mswin32", "mswin64", "universal-mingw"],
            "mswin" => &["mswin32"],
            "mswin64" => &["mswin64"],
            "mingw" | "x64" => &["universal-mingw"],
            _ => &[],
        };
        // `x64_mingw` splits as `x64` and `mingw`: the name, not a version.
        let generics: &[&str] = if name == "x64_mingw" || base == "x64_mingw" {
            &["universal-mingw"]
        } else {
            generics
        };
        for g in generics {
            let platform = if *g == "ruby" {
                GemPlatform::Ruby
            } else {
                GemPlatform::Platform(Platform::parse(g))
            };
            if !out.contains(&platform) {
                out.push(platform);
            }
        }
    }
    out
}

impl GemfileDependency {
    /// `Bundler::Dependency#gem_platforms`: the resolution platforms this
    /// dependency applies on; `ruby` alone when it forces the ruby build.
    pub(crate) fn gem_platforms(&self, valid: &[GemPlatform]) -> Vec<GemPlatform> {
        if self.force_ruby_platform {
            return vec![GemPlatform::Ruby];
        }
        if self.platforms.is_empty() {
            return valid.to_vec();
        }
        let expanded = expanded_platforms(&self.platforms);
        valid
            .iter()
            .filter(|p| expanded.contains(&gem_platform::generic(p)))
            .cloned()
            .collect()
    }

    /// `Bundler::Dependency#current_platform?`: whether the Ruby running is
    /// one the dependency names. Spinel counts as `ruby`, as
    /// `Bundler::CurrentRuby#ruby?` is true whenever the generic local
    /// platform is `ruby` (`generic_local_platform_is_ruby?`, Bundler 2.1+,
    /// docs/cli.md), and as no other engine, so `mri`, `jruby` and the rest
    /// are false. `ruby_40` is `ruby?` on a Ruby whose version starts `4.0.`
    /// (`on_40?`), the language version Spinel implements.
    // ponytail: only `ruby` (and its versions) counts as the current Ruby;
    // upgrade with `Bundler::CurrentRuby`'s engine and Windows checks when
    // Spinel runs on Windows.
    pub(crate) fn current_platform(&self) -> bool {
        self.platforms.is_empty()
            || self
                .platforms
                .iter()
                .any(|p| ruby_platform_here(p, spinel_vm::LANGUAGE_VERSION))
    }
}

/// Whether `CurrentRuby` answers the Gemfile platform `name` with true on
/// a Ruby counted as `ruby` whose `RUBY_VERSION` is `version`: `ruby`, or
/// `ruby_<major><minor>` when `version` starts `<major>.<minor>.`
/// (`KNOWN_MINOR_VERSIONS`, two digits each).
fn ruby_platform_here(name: &str, version: &str) -> bool {
    if name == "ruby" {
        return true;
    }
    match name.strip_prefix("ruby_").map(str::as_bytes) {
        Some(&[major, minor]) if major.is_ascii_digit() && minor.is_ascii_digit() => {
            version.starts_with(&format!("{}.{}.", major as char, minor as char))
        }
        _ => false,
    }
}

/// `Resolver::Package`, plus what `Resolver` caches per package.
struct PackageState {
    /// The resolution platforms, or a top-level dependency's own.
    platforms: Vec<GemPlatform>,
    top_level: bool,
    force_ruby_platform: bool,
    prereleases: Prereleases,
    /// `@sorted_versions[package]`, with each candidate's group, ascending,
    /// and the builds the groups index: the locked ones first.
    candidates: Option<Versions>,
}

/// A package's builds, the locked ones first, and its candidates, each with
/// its group of those builds, ascending.
type Versions = (std::rc::Rc<Vec<Spec>>, Vec<(Candidate, SpecGroup)>);

/// `Bundler::Resolver`'s model over one index: each package's candidates,
/// which `resolver/source.rs` hands the solver.
struct Provider<'a> {
    request: &'a Request,
    index: &'a Index,
    locked: &'a Locked,
    /// Each gem's builds, without the ones that depend on themselves
    /// impossibly, sorted by version, then platform.
    specs: RefCell<HashMap<String, std::rc::Rc<Vec<Spec>>>>,
    packages: RefCell<HashMap<PackageName, PackageState>>,
    /// The root's dependencies (`prepare_dependencies`), each package once.
    root_constraints: Vec<(Package, VersionSet)>,
}

impl<'a> Provider<'a> {
    fn new(
        request: &'a Request,
        index: &'a Index,
        locked: &'a Locked,
        relaxed: &HashSet<PackageName>,
    ) -> Provider<'a> {
        let provider = Provider {
            request,
            index,
            locked,
            specs: RefCell::new(HashMap::new()),
            packages: RefCell::new(HashMap::new()),
            root_constraints: Vec::new(),
        };
        for name in relaxed {
            provider.package_state(name);
            if let Some(state) = provider.packages.borrow_mut().get_mut(name) {
                state.prereleases = Prereleases::ConsiderLast;
            }
        }
        provider
    }

    /// `@all_specs[name]`: the builds `filter_invalid_self_dependencies`
    /// keeps, sorted by `[version, platform.to_s]`. `bundler`'s is the one
    /// build of `Source::Metadata`, which `source_requirements` makes the
    /// only source of `bundler`: the Bundler the lockfile is written for.
    fn all_specs(&self, name: &str) -> std::rc::Rc<Vec<Spec>> {
        if let Some(specs) = self.specs.borrow().get(name) {
            return specs.clone();
        }
        if name == "bundler" {
            let specs = std::rc::Rc::new(vec![bundler_spec()]);
            self.specs
                .borrow_mut()
                .insert(name.to_owned(), specs.clone());
            return specs;
        }
        let mut specs: Vec<Spec> = self
            .index
            .get(name)
            .map(|specs| {
                specs
                    .iter()
                    .filter(|s| {
                        !s.dependencies
                            .iter()
                            .any(|(dep, req)| dep == name && !req.satisfied_by(&s.version))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        specs.sort_by(|a, b| {
            a.version
                .cmp(&b.version)
                .then_with(|| a.platform.to_string().cmp(&b.platform.to_string()))
        });
        let specs = std::rc::Rc::new(specs);
        self.specs
            .borrow_mut()
            .insert(name.to_owned(), specs.clone());
        specs
    }

    /// The state of a gem not in the Gemfile, made on first sight with every
    /// resolution platform and prereleases left out, unless its locked
    /// version is one.
    fn package_state(&self, name: &PackageName) {
        let prereleases = if self.locked_prerelease(name) {
            Prereleases::ConsiderFirst
        } else {
            Prereleases::Ignore
        };
        self.packages
            .borrow_mut()
            .entry(name.clone())
            .or_insert_with(|| PackageState {
                platforms: self.request.platforms.clone(),
                top_level: false,
                force_ruby_platform: false,
                prereleases,
                candidates: None,
            });
    }

    /// `Package#locked_version`.
    fn locked_version(&self, name: &PackageName) -> Option<&Version> {
        match name {
            PackageName::Gem(gem) => self.locked.versions.get(gem),
            _ => None,
        }
    }

    /// Whether the gem's locked version is a prerelease, which lets its
    /// prereleases in as `Package` does.
    fn locked_prerelease(&self, name: &PackageName) -> bool {
        self.locked_version(name).is_some_and(Version::prerelease)
    }

    /// `Package#unlock?`.
    fn unlocked(&self, name: &PackageName) -> bool {
        match name {
            PackageName::Gem(gem) => self.locked.unlock_all || self.locked.unlock.contains(gem),
            _ => false,
        }
    }

    /// `@sorted_versions[package]`, worked out on first use.
    fn with_candidates<T>(
        &self,
        name: &PackageName,
        f: impl FnOnce(&[(Candidate, SpecGroup)], &[Spec]) -> T,
    ) -> T {
        self.package_state(name);
        let all = match name {
            PackageName::Gem(gem) => self.all_specs(gem),
            _ => std::rc::Rc::new(Vec::new()),
        };
        let mut packages = self.packages.borrow_mut();
        let state = packages.get_mut(name).expect("made above");
        if state.candidates.is_none() {
            state.candidates = Some(self.all_versions_for(name, state, &all));
        }
        let (builds, candidates) = state.candidates.as_ref().expect("set above");
        f(candidates, builds)
    }

    /// `Resolver#all_versions_for`: the locked builds and the index's, the
    /// locked ones first and an index build of the same version and
    /// platform left out; those the gem's base requirement allows, grouped
    /// by version, up to two candidates of each, as the module notes say.
    fn all_versions_for(
        &self,
        name: &PackageName,
        state: &PackageState,
        index_specs: &[Spec],
    ) -> Versions {
        let meta = match name {
            PackageName::Ruby => Some(&self.request.ruby_version),
            PackageName::RubyGems => Some(&self.request.rubygems_version),
            PackageName::Gem(_) => None,
        };
        if let Some(version) = meta {
            let group = SpecGroup {
                specs: Vec::new(),
                dependencies: Vec::new(),
            };
            let candidates = vec![(Candidate::new(version.clone(), -1), group)];
            return (std::rc::Rc::new(Vec::new()), candidates);
        }
        let PackageName::Gem(gem) = name else {
            unreachable!("the meta packages returned above");
        };
        // `@base[name]`: `SpecSet#[]` lists a name's builds last first.
        let mut builds: Vec<Spec> = self
            .locked
            .base
            .iter()
            .rev()
            .filter(|s| &s.name == gem)
            .cloned()
            .collect();
        let mut locked_builds = builds.len();
        // `uniq {|spec| [spec.version.hash, spec.platform] }`.
        let mut seen: HashSet<(&Version, &GemPlatform)> =
            builds.iter().map(|b| (&b.version, &b.platform)).collect();
        let mut fresh: Vec<&Spec> = Vec::new();
        // `filter_prereleases`, of the index's builds only.
        for spec in index_specs {
            if state.prereleases == Prereleases::Ignore
                && index_specs.len() > 1
                && spec.version.prerelease()
            {
                continue;
            }
            if seen.insert((&spec.version, &spec.platform)) {
                fresh.push(spec);
            }
        }
        builds.extend(fresh.into_iter().cloned());
        if let Some(requirement) = self.locked.requirements.get(gem) {
            locked_builds = builds[..locked_builds]
                .iter()
                .filter(|b| requirement.satisfied_by(&b.version))
                .count();
            builds.retain(|b| requirement.satisfied_by(&b.version));
        }
        let all = &builds[..];
        // `group_by(&:version)`, which groups by `eql?`, the version
        // string, in the order first seen.
        let mut groups: Vec<(&Version, Vec<usize>)> = Vec::new();
        let mut group_of: HashMap<&str, usize> = HashMap::new();
        for (i, spec) in all.iter().enumerate() {
            match group_of.get(spec.version.as_str()) {
                Some(&g) => groups[g].1.push(i),
                None => {
                    group_of.insert(spec.version.as_str(), groups.len());
                    groups.push((&spec.version, vec![i]));
                }
            }
        }
        let unlocked = self.unlocked(name);
        let mut candidates = Vec::new();
        for (version, specs) in groups {
            // `Package#platform_specs`: on a platform the lockfile has, the
            // locked builds that install there, when there are any, as
            // `select_all_platform_match(prefer_locked: true)` keeps them.
            let platform_specs: Vec<Vec<usize>> = state
                .platforms
                .iter()
                .map(|p| {
                    let prefer_locked = !unlocked && !self.locked.new_platforms.contains(p);
                    let locked: Vec<usize> = specs
                        .iter()
                        .copied()
                        .filter(|&i| i < locked_builds && installable_on(&all[i].platform, p))
                        .collect();
                    if prefer_locked && !locked.is_empty() {
                        best_platform_match(&locked, all, p)
                    } else {
                        best_platform_match(&specs, all, p)
                    }
                })
                .collect();
            let skip = if state.top_level {
                platform_specs.iter().any(Vec::is_empty)
            } else {
                platform_specs.iter().all(Vec::is_empty)
            };
            if skip {
                continue;
            }
            let ruby_specs = best_platform_match(&specs, all, &GemPlatform::Ruby);
            let mut ruby_group = SpecGroup::new(ruby_specs, all);
            if !ruby_group.specs.is_empty() {
                for s in &platform_specs {
                    ruby_group.merge(&SpecGroup::new(s.clone(), all), all);
                }
                candidates.push((Candidate::new(version.clone(), -1), ruby_group.clone()));
                if state.force_ruby_platform {
                    continue;
                }
            }
            let mut flat: Vec<usize> = Vec::new();
            for i in platform_specs.into_iter().flatten() {
                if !flat.contains(&i) {
                    flat.push(i);
                }
            }
            let platform_group = SpecGroup::new(flat, all);
            if platform_group.sorted_names(all) == ruby_group.sorted_names(all) {
                continue;
            }
            candidates.push((Candidate::new(version.clone(), 1), platform_group));
        }
        // `@sorted_versions`: `sort`, which the locked versions coming first
        // can leave to do.
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        (std::rc::Rc::new(builds), candidates)
    }

    /// `select_sorted_versions(package, range)`: the candidates in `range`.
    fn versions_for(&self, name: &PackageName, range: &VersionSet) -> Vec<Candidate> {
        self.with_candidates(name, |candidates, _| {
            candidates
                .iter()
                .map(|(c, _)| c)
                .filter(|c| range.includes(c))
                .cloned()
                .collect()
        })
    }

    fn prereleases(&self, name: &PackageName) -> Prereleases {
        self.package_state(name);
        self.packages.borrow()[name].prereleases
    }

    /// `Package#ignores_prereleases?`.
    fn ignores_prereleases(&self, name: &PackageName) -> bool {
        self.prereleases(name) == Prereleases::Ignore
    }

    /// `@all_specs[name].any? {|s| s.version.prerelease? }`: the meta
    /// packages' one version is the Ruby or RubyGems resolved for.
    fn has_prereleases(&self, name: &PackageName) -> bool {
        match name {
            PackageName::Gem(gem) => self.all_specs(gem).iter().any(|s| s.version.prerelease()),
            PackageName::Ruby => self.request.ruby_version.prerelease(),
            PackageName::RubyGems => self.request.rubygems_version.prerelease(),
        }
    }

    /// The gem's name when it has a base requirement (`base_requirements[name]`).
    fn base_requirement_name(&self, name: &PackageName) -> Option<String> {
        match name {
            PackageName::Gem(gem) if self.locked.requirements.contains_key(gem) => {
                Some(gem.clone())
            }
            _ => None,
        }
    }

    /// `GemVersionPromoter#sort_versions` at its default level, `major`:
    /// newest first, releases before prereleases unless prereleases were
    /// asked for, and then (`post_sort`) the locked version moved to the
    /// front unless the gem is unlocked.
    fn sort_preferred(&self, name: &PackageName, versions: &mut Vec<Candidate>) {
        let specified = self.prereleases(name) == Prereleases::ConsiderFirst;
        versions.sort_by(|a, b| {
            if !specified {
                match (a.version.prerelease(), b.version.prerelease()) {
                    (true, false) => return Ordering::Greater,
                    (false, true) => return Ordering::Less,
                    _ => {}
                }
            }
            b.cmp(a)
        });
        if self.unlocked(name) {
            return;
        }
        if let Some(locked) = self.locked_version(name) {
            // `move_version_to_beginning`, by `to_s`.
            let (mut front, rest): (Vec<Candidate>, Vec<Candidate>) = versions
                .drain(..)
                .partition(|c| c.version.as_str() == locked.as_str());
            front.extend(rest);
            *versions = front;
        }
    }

    /// `Resolver::Base#initialize` and `Resolver#prepare_dependencies`: the
    /// Gemfile's dependencies that apply on some resolution platform, each
    /// making its gem top-level, and the two meta dependencies. A Gemfile gem
    /// no candidate of which satisfies its requirement is tried with
    /// prereleases, then reported as not found.
    fn prepare_root(&mut self) -> Result<(), Failure> {
        let request = self.request;
        let mut deps: Vec<(PackageName, Requirement)> = Vec::new();
        let mut gemfile_deps: HashMap<PackageName, &GemfileDependency> = HashMap::new();
        for dep in &request.dependencies {
            let platforms = dep.gem_platforms(&request.platforms);
            if platforms.is_empty() {
                continue;
            }
            let name = PackageName::Gem(dep.name.clone());
            let relaxed = self
                .packages
                .borrow()
                .get(&name)
                .is_some_and(|s| s.prereleases == Prereleases::ConsiderLast);
            self.packages.borrow_mut().insert(
                name.clone(),
                PackageState {
                    platforms,
                    top_level: true,
                    force_ruby_platform: dep.force_ruby_platform,
                    prereleases: if dep.requirement.prerelease() || self.locked_prerelease(&name) {
                        Prereleases::ConsiderFirst
                    } else if relaxed {
                        Prereleases::ConsiderLast
                    } else {
                        Prereleases::Ignore
                    },
                    candidates: None,
                },
            );
            gemfile_deps.insert(name.clone(), dep);
            deps.push((name, dep.requirement.clone()));
        }
        for (meta, version) in [
            (PackageName::Ruby, &request.ruby_version),
            (PackageName::RubyGems, &request.rubygems_version),
        ] {
            let exact = Requirement::new([format!("= {version}").as_str()])
                .expect("a version makes an exact requirement");
            let relaxed = self
                .packages
                .borrow()
                .get(&meta)
                .is_some_and(|s| s.prereleases == Prereleases::ConsiderLast);
            self.packages.borrow_mut().insert(
                meta.clone(),
                PackageState {
                    platforms: request.platforms.clone(),
                    top_level: true,
                    force_ruby_platform: false,
                    prereleases: if relaxed {
                        Prereleases::ConsiderLast
                    } else {
                        Prereleases::Ignore
                    },
                    candidates: None,
                },
            );
            deps.push((meta, exact));
        }
        let ranges = dependency_hash(
            deps.iter()
                .map(|(name, req)| (Package::Name(name.clone()), solver::candidate_range(req))),
            VersionSet::intersect,
        );
        let mut root = Vec::new();
        for (package, range) in ranges {
            let Package::Name(name) = &package else {
                unreachable!("dependencies are never the root");
            };
            if self.versions_for(name, &range).is_empty() {
                let mut packages = self.packages.borrow_mut();
                let state = packages.get_mut(name).expect("made above");
                if state.prereleases == Prereleases::Ignore {
                    state.prereleases = Prereleases::ConsiderLast;
                    state.candidates = None;
                }
            }
            if !self.versions_for(name, &range).is_empty() {
                root.push((package, range));
                continue;
            }
            let Some(dep) = gemfile_deps.get(name) else {
                root.push((package, range));
                continue;
            };
            if !dep.current_platform() {
                continue;
            }
            return Err(self.not_found(dep));
        }
        self.root_constraints = root;
        Ok(())
    }

    /// `Resolver#raise_not_found!`.
    fn not_found(&self, dep: &GemfileDependency) -> Failure {
        let specs = self.all_specs(&dep.name);
        let label = if dep.requirement.none() {
            dep.name.clone()
        } else {
            format!("{} ({})", dep.name, dep.requirement)
        };
        let matching: Vec<&Spec> = specs
            .iter()
            .filter(|s| dep.requirement.satisfied_by(&s.version))
            .collect();
        let state_platforms = {
            let packages = self.packages.borrow();
            packages[&PackageName::Gem(dep.name.clone())]
                .platforms
                .clone()
        };
        let (listed, part, message): (Vec<&Spec>, &str, String) = if matching.is_empty() {
            (
                specs.iter().collect(),
                &dep.name,
                format!("Could not find gem '{label}'"),
            )
        } else if let [platform] = state_platforms.as_slice() {
            (
                matching,
                &label,
                format!("Could not find gem '{label}' with platform '{platform}'"),
            )
        } else {
            let platforms: Vec<String> = state_platforms.iter().map(ToString::to_string).collect();
            (
                matching,
                &label,
                format!(
                    "Could not find gems matching '{label}' valid for all resolution platforms ({})",
                    platforms.join(", ")
                ),
            )
        };
        let mut message = format!("{message} in {}.\n", self.request.source);
        if !listed.is_empty() {
            message.push('\n');
            message.push_str(&other_specs_matching_message(&listed, part));
        }
        Failure::NotFound(message)
    }

    /// The requirements of `candidate` of `name`, without any on itself
    /// (`version.dependencies.reject {|d| d.name == package.name }`), sorted
    /// by name.
    fn candidate_requirements(
        &self,
        name: &PackageName,
        candidate: &Candidate,
    ) -> Vec<(PackageName, Requirement)> {
        let deps = self.with_candidates(name, |candidates, _| {
            candidates
                .iter()
                .find(|(c, _)| c == candidate)
                .map(|(_, group)| group.dependencies.clone())
                .unwrap_or_default()
        });
        deps.into_iter().filter(|(dep, _)| dep != name).collect()
    }

    /// `Package#platforms`: the resolution platforms, or a top-level
    /// dependency's own.
    fn package_platforms(&self, package: &Package) -> Vec<GemPlatform> {
        match package {
            Package::Root(_) => self.request.platforms.clone(),
            Package::Name(name) => {
                self.package_state(name);
                self.packages.borrow()[name].platforms.clone()
            }
        }
    }

    /// The builds of a chosen candidate that install on each resolution
    /// platform, best match first, as `Candidate#to_specs` and
    /// `SpecSet#for` materialize them for the lockfile.
    fn chosen_specs(&self, name: &PackageName, candidate: &Candidate) -> Vec<Spec> {
        let platforms = self.request.platforms.clone();
        self.with_candidates(name, |candidates, all| {
            let Some((_, group)) = candidates.iter().find(|(c, _)| c == candidate) else {
                return Vec::new();
            };
            let mut chosen: Vec<usize> = Vec::new();
            for platform in &platforms {
                for i in best_platform_match(&group.specs, all, platform) {
                    if !chosen.contains(&i) {
                        chosen.push(i);
                    }
                }
            }
            chosen.into_iter().map(|i| all[i].clone()).collect()
        })
    }

    /// Every build of a chosen candidate: `SpecGroup#to_specs`.
    fn group_specs(&self, name: &PackageName, candidate: &Candidate) -> Vec<Spec> {
        self.with_candidates(name, |candidates, all| {
            candidates
                .iter()
                .find(|(c, _)| c == candidate)
                .map(|(_, group)| group.specs.iter().map(|&i| all[i].clone()).collect())
                .unwrap_or_default()
        })
    }

    /// `@sorted_versions[package]`: the root's one version, or the
    /// package's candidates, ascending.
    fn sorted_candidates(&self, package: &Package) -> Vec<Candidate> {
        match package {
            Package::Root(_) => vec![root_candidate()],
            Package::Name(name) => {
                self.with_candidates(name, |c, _| c.iter().map(|(c, _)| c.clone()).collect())
            }
        }
    }
}

/// `other_specs_matching_message`.
fn other_specs_matching_message(specs: &[&Spec], requirement: &str) -> String {
    let mut message = format!("The source contains the following gems matching '{requirement}':\n");
    let lines: Vec<String> = specs
        .iter()
        .map(|s| format!("  * {}", s.full_name()))
        .collect();
    message.push_str(&lines.join("\n"));
    message
}

/// `Source::Metadata`'s `bundler`: the version the lockfile writer emulates,
/// with no dependencies or requirements.
fn bundler_spec() -> Spec {
    Spec {
        name: "bundler".to_owned(),
        version: Version::new(crate::lockfile::BUNDLER_VERSION).expect("a version"),
        platform: GemPlatform::Ruby,
        dependencies: Vec::new(),
        required_ruby_version: Requirement::default(),
        required_rubygems_version: Requirement::default(),
        checksum: None,
    }
}

/// The root's one version.
fn root_candidate() -> Candidate {
    Candidate::new(Version::new("0").expect("0 is a version"), -1)
}

/// Resolve `request` against `index`, as `Bundler::Resolver#start` does with
/// no lockfile: the builds to install, in the lockfile's order (`full_name`),
/// or why there are none.
pub fn resolve(request: &Request, index: &Index) -> Result<Vec<Resolved>, Failure> {
    solve(
        request,
        index,
        Locked::default(),
        debug_resolver(),
        print_relaxed,
        resolved_builds,
    )
}

/// The builds of a resolution with no lockfile, in the lockfile's order.
fn resolved_builds(provider: &Provider<'_>, solution: Vec<(Package, Candidate)>) -> Vec<Resolved> {
    let mut resolved = Vec::new();
    for (package, candidate) in solution {
        let Package::Name(name @ PackageName::Gem(gem)) = &package else {
            continue;
        };
        // `SpecSet#for` never materializes `bundler`, so neither the
        // lockfile nor the install plan has it.
        if gem == "bundler" {
            continue;
        }
        for spec in provider.chosen_specs(name, &candidate) {
            resolved.push(Resolved {
                name: spec.name,
                version: spec.version,
                platform: spec.platform,
            });
        }
    }
    // `LockfileGenerator`'s order: by `full_name`.
    resolved.sort_by_cached_key(Resolved::full_name);
    resolved
}

/// Resolve `request` against `index` with a lockfile's hold on it (#79), as
/// `Resolver#start` does for `Definition#start_resolution`: every build of
/// each candidate chosen (`Candidate#to_specs`), the lockfile's own where
/// they were preferred, then each locked build of a version chosen whose
/// dependencies the result satisfies
/// (`SpecSet#specs_with_additional_variants_from`). `SpecSet#for` of the
/// Gemfile on the lockfile's platforms, which `crate::locked` runs, then
/// keeps the builds the lockfile lists.
pub fn resolve_locked(
    request: &Request,
    index: &Index,
    locked: &Locked,
) -> Result<Vec<Spec>, Failure> {
    let finish = |provider: &Provider<'_>, solution: Vec<(Package, Candidate)>| {
        let mut specs: Vec<Spec> = Vec::new();
        for (package, candidate) in solution {
            let Package::Name(name @ PackageName::Gem(gem)) = &package else {
                continue;
            };
            if gem == "bundler" {
                continue;
            }
            specs.extend(provider.group_specs(name, &candidate));
        }
        let mut variants: Vec<Spec> = Vec::new();
        for other in &locked.locked_specs {
            // `exemplary_spec`: the last build of the gem the result has.
            let Some(spec) = specs.iter().rev().find(|s| s.name == other.name) else {
                continue;
            };
            let known = |s: &Spec| s.full_name() == other.full_name();
            if spec.version == other.version
                && valid_dependencies(other, &specs)
                && !specs.iter().any(known)
                && !variants.iter().any(known)
            {
                variants.push(other.clone());
            }
        }
        specs.extend(variants);
        specs
    };
    solve(
        request,
        index,
        locked.clone(),
        debug_resolver(),
        print_relaxed,
        finish,
    )
}

/// `SpecSet#validate_deps(s) == :valid`: each of the build's dependencies
/// but `bundler` names a gem of `set` with a build that satisfies it.
pub(crate) fn valid_dependencies(spec: &Spec, set: &[Spec]) -> bool {
    spec.dependencies.iter().all(|(name, req)| {
        name == "bundler"
            || set
                .iter()
                .any(|s| &s.name == name && req.satisfied_by(&s.version))
    })
}

/// `Resolver#debug?`: with Bundler's `DEBUG_RESOLVER` or one of its other
/// names set, a logger that prints every line pubgrub-ruby logs to standard
/// error, as Bundler prints them.
fn debug_resolver() -> Option<solver::Logger> {
    let debug = [
        "BUNDLER_DEBUG_RESOLVER",
        "BUNDLER_DEBUG_RESOLVER_TREE",
        "DEBUG_RESOLVER",
        "DEBUG_RESOLVER_TREE",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some());
    debug.then(|| Box::new(|_, line: &str| eprintln!("{line}")) as solver::Logger)
}

/// With `SPINEL_DEBUG_RELAX` set, a failed try's `find_names_to_relax`,
/// printed to standard error as one JSON line, as
/// `tests/fixtures/resolver/relax_hook.rb` logs Bundler's, for
/// `relax_compare.py` to compare.
fn print_relaxed(names: &source::NamesToRelax) {
    if std::env::var_os("SPINEL_DEBUG_RELAX").is_some() {
        eprintln!("find_names_to_relax: {}", names.to_json());
    }
}

/// `Resolver#start` and `#solve_versions`: resolve with `locked`'s hold on
/// the resolution (none with no lockfile), and on a failure let
/// `find_names_to_relax` name what to relax: the resolution is set up again
/// (`setup_solver`) and retried with those gems unlocked
/// (`Base#unlock_names`) and their prereleases let in
/// (`Base#include_prereleases`), until it resolves or there is nothing to
/// relax. `logger` gets every line pubgrub-ruby logs, on every try,
/// `relaxed_by` what each failed try's `find_names_to_relax` found, and
/// `finish` the solution.
fn solve<T>(
    request: &Request,
    index: &Index,
    mut locked: Locked,
    mut logger: Option<solver::Logger>,
    mut relaxed_by: impl FnMut(&source::NamesToRelax),
    finish: impl FnOnce(&Provider<'_>, Vec<(Package, Candidate)>) -> T,
) -> Result<T, Failure> {
    // `Base#requirements`: the Gemfile's dependencies that apply on some
    // resolution platform, which `indirect_pins` leaves to the last.
    let gemfile: Vec<&str> = request
        .dependencies
        .iter()
        .filter(|d| !d.gem_platforms(&request.platforms).is_empty())
        .map(|d| d.name.as_str())
        .collect();
    let mut relaxed: HashSet<PackageName> = HashSet::new();
    let root = Package::Root(request.gemfile.clone());
    loop {
        let names = {
            let mut provider = Provider::new(request, index, &locked, &relaxed);
            provider.prepare_root()?;
            let mut version_solver = solver::VersionSolver::new(
                source::BundlerSource::new(&provider),
                root.clone(),
                source::BundlerStrategy::default(),
                logger,
            );
            let failure = match version_solver.solve() {
                Ok(decisions) => return Ok(finish(&provider, decisions)),
                Err(failure) => failure,
            };
            logger = version_solver.into_logger();
            let names = source::find_names_to_relax(&provider, &failure.incompatibility);
            relaxed_by(&names);
            if names.unlock.is_empty() && names.prereleases.is_empty() {
                // `SolveFailure#to_s`, and after it the extended explanation.
                let mut explanation = report::explanation(&failure.incompatibility);
                if let Some(extended) = names.extended_explanation {
                    explanation.push_str("\n\n");
                    explanation.push_str(&extended);
                }
                return Err(Failure::Conflict(format!(
                    "Could not find compatible versions\n\n{explanation}"
                )));
            }
            names
        };
        if !names.unlock.is_empty() {
            locked.unlock_names(&names.unlock, &gemfile);
        }
        relaxed.extend(names.prereleases);
    }
}

#[cfg(test)]
mod tests;
