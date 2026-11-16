//! The resolver (#78, docs/package-manager.md, "Resolver"): a Gemfile's
//! dependencies and a compact index in, the gems to install out, chosen as
//! Bundler 4 chooses them, or an explanation of why there are none.
//!
//! Bundler resolves with PubGrub (its vendored `pub_grub`), and so does this,
//! with the `pubgrub` crate. What makes the choices Bundler's is the model
//! fed to it, which follows `Bundler::Resolver` and its `Package`,
//! `Candidate`, `SpecGroup` and `Strategy` step for step:
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
//!   fails on a gem with prereleases left out, it is retried with them in,
//!   as Bundler retries.
//! - **Order.** The next gem decided is the one with at most one candidate
//!   left, then the one with the fewest candidates above the range asked
//!   for, ties going to the one found first; its candidate is the newest,
//!   releases before prereleases unless prereleases were asked for, a
//!   platform build before the ruby build of the same version.
//!
//! A conflict is explained as Bundler explains it: pubgrub-ruby's
//! `FailureWriter`, ported in `resolver/report.rs`.
//!
//! Not here yet: the lockfile and locked versions (#79, #80), git and path
//! sources (#88), sources scoped to a gem, and `bundler` itself as a
//! dependency, which Bundler pins to the version running.

mod report;

use std::cell::RefCell;
use std::cmp::{Ordering, Reverse};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Bound;

use pubgrub::{
    Dependencies, DependencyProvider, DerivationTree, External, PackageResolutionStatistics,
    PubGrubError, Ranges,
};

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
    pub required_ruby_version: Option<Requirement>,
    pub required_rubygems_version: Option<Requirement>,
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
            if let Some(req) = req.as_ref().filter(|r| !r.none()) {
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
    let mut fields: Vec<&str> = s.split(',').collect();
    while fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields
}

/// `/info/<gem>`, as `Gem::Resolver::APISet::GemParser` reads each line after
/// the `---` header: `version[-platform] dep:req&req,dep:req|key:value,...`.
/// Of the keys, `ruby` and `rubygems` are read; `checksum` and `created_at`
/// are the store's and Bundler's cooldown's.
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
                Some((name, reqs)) => Requirement::new(reqs.split('&'))
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
            required_ruby_version: None,
            required_rubygems_version: None,
        };
        for field in requirements {
            let Some((key, value)) = field.split_once(':') else {
                continue;
            };
            let read = || Requirement::new(value.split('&')).map_err(|e| error(e.to_string()));
            match key {
                "ruby" => spec.required_ruby_version = Some(read()?),
                "rubygems" => spec.required_rubygems_version = Some(read()?),
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
    /// The requirements conflict (`Bundler::SolveFailure`): pubgrub's
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

    pub fn version(&self) -> &Version {
        &self.version
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Candidate) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

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

pub type VersionSet = Ranges<Candidate>;

/// `Resolver#requirement_to_range`: each pair as a range of candidates,
/// intersected. `~>` ends before the bump's `.A`, the lowest prerelease of
/// it, so no prerelease of the next release is in.
pub fn requirement_to_range(requirement: &Requirement) -> VersionSet {
    requirement
        .requirements()
        .iter()
        .map(|(op, version)| {
            let ver = || Candidate::new(version.clone(), -1);
            let platform_ver = || Candidate::new(version.clone(), 1);
            let exact = || Ranges::from_range_bounds(ver()..=platform_ver());
            match op {
                Op::Pessimistic => Ranges::between(ver(), pessimistic_end(version)),
                Op::Gt => Ranges::strictly_higher_than(platform_ver()),
                Op::Ge => Ranges::higher_than(ver()),
                Op::Lt => Ranges::strictly_lower_than(ver()),
                Op::Le => Ranges::lower_than(platform_ver()),
                Op::Eq => exact(),
                Op::Ne => exact().complement(),
            }
        })
        .fold(Ranges::full(), |all, range| all.intersection(&range))
}

/// Where `~> version` ends: `"#{version.bump}.A"`.
fn pessimistic_end(version: &Version) -> Candidate {
    let end = Version::new(&format!("{}.A", version.bump())).expect("a bump is a version");
    Candidate::new(end, -1)
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
fn best_platform_match(specs: &[usize], all: &[Spec], platform: &GemPlatform) -> Vec<usize> {
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
    fn gem_platforms(&self, valid: &[GemPlatform]) -> Vec<GemPlatform> {
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
    /// one the dependency names. Spinel's generic platform is `ruby`, which
    /// makes `ruby?` true and every engine-specific name false.
    // ponytail: only `ruby` (and its versions) counts as the current Ruby;
    // upgrade with `Bundler::CurrentRuby`'s engine and Windows checks when
    // Spinel runs on Windows.
    fn current_platform(&self) -> bool {
        self.platforms.is_empty()
            || self.platforms.iter().any(|p| {
                p == "ruby"
                    || p.strip_prefix("ruby_")
                        .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
            })
    }
}

/// `Resolver::Package`, plus what `Resolver` caches per package.
struct PackageState {
    /// The resolution platforms, or a top-level dependency's own.
    platforms: Vec<GemPlatform>,
    top_level: bool,
    force_ruby_platform: bool,
    prereleases: Prereleases,
    /// `@sorted_versions[package]`, with each candidate's group, ascending.
    candidates: Option<Vec<(Candidate, SpecGroup)>>,
}

/// The `PubGrub` source: `Bundler::Resolver` over one index.
struct Provider<'a> {
    request: &'a Request,
    index: &'a Index,
    /// Each gem's builds, without the ones that depend on themselves
    /// impossibly, sorted by version, then platform.
    specs: RefCell<HashMap<String, std::rc::Rc<Vec<Spec>>>>,
    packages: RefCell<HashMap<PackageName, PackageState>>,
    root_dependencies: Vec<(Package, VersionSet)>,
    /// When each package was first depended on, as pubgrub-ruby's partial
    /// solution orders the packages it has yet to decide: propagation walks
    /// a package's incompatibilities last first, so of one version's
    /// dependencies the last is seen first.
    seen: RefCell<HashMap<Package, usize>>,
}

impl<'a> Provider<'a> {
    fn new(request: &'a Request, index: &'a Index, relaxed: &HashSet<String>) -> Provider<'a> {
        let provider = Provider {
            request,
            index,
            specs: RefCell::new(HashMap::new()),
            packages: RefCell::new(HashMap::new()),
            root_dependencies: Vec::new(),
            seen: RefCell::new(HashMap::new()),
        };
        for name in relaxed {
            provider.package_state(&PackageName::Gem(name.clone()));
            if let Some(state) = provider
                .packages
                .borrow_mut()
                .get_mut(&PackageName::Gem(name.clone()))
            {
                state.prereleases = Prereleases::ConsiderLast;
            }
        }
        provider
    }

    /// `@all_specs[name]`: the builds `filter_invalid_self_dependencies`
    /// keeps, sorted by `[version, platform.to_s]`.
    fn all_specs(&self, name: &str) -> std::rc::Rc<Vec<Spec>> {
        if let Some(specs) = self.specs.borrow().get(name) {
            return specs.clone();
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
    /// resolution platform and prereleases left out.
    fn package_state(&self, name: &PackageName) {
        self.packages
            .borrow_mut()
            .entry(name.clone())
            .or_insert_with(|| PackageState {
                platforms: self.request.platforms.clone(),
                top_level: false,
                force_ruby_platform: false,
                prereleases: Prereleases::Ignore,
                candidates: None,
            });
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
        f(state.candidates.as_deref().expect("set above"), &all)
    }

    /// `Resolver#all_versions_for`: group the builds by version and make up
    /// to two candidates of each, as the module notes say.
    fn all_versions_for(
        &self,
        name: &PackageName,
        state: &PackageState,
        all: &[Spec],
    ) -> Vec<(Candidate, SpecGroup)> {
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
            return vec![(Candidate::new(version.clone(), -1), group)];
        }
        // `filter_prereleases`.
        let indices: Vec<usize> = (0..all.len())
            .filter(|&i| {
                !(state.prereleases == Prereleases::Ignore
                    && all.len() > 1
                    && all[i].version.prerelease())
            })
            .collect();
        // `group_by(&:version)`, which groups by `eql?`: the version string.
        let mut groups: Vec<(&Version, Vec<usize>)> = Vec::new();
        for i in indices {
            match groups.last_mut() {
                Some((v, members)) if v.as_str() == all[i].version.as_str() => members.push(i),
                _ => groups.push((&all[i].version, vec![i])),
            }
        }
        let mut candidates = Vec::new();
        for (version, specs) in groups {
            let platform_specs: Vec<Vec<usize>> = state
                .platforms
                .iter()
                .map(|p| best_platform_match(&specs, all, p))
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
        candidates
    }

    /// `versions_for(package, range)`: the candidates in `range`.
    fn versions_for(&self, name: &PackageName, range: &VersionSet) -> Vec<Candidate> {
        self.with_candidates(name, |candidates, _| {
            candidates
                .iter()
                .map(|(c, _)| c)
                .filter(|c| range.contains(*c))
                .cloned()
                .collect()
        })
    }

    /// The candidates above every one `range` holds: `range.upper_invert`.
    fn higher_versions(&self, name: &PackageName, range: &VersionSet) -> usize {
        let Some((_, end)) = range.bounding_range() else {
            return 0;
        };
        self.with_candidates(name, |candidates, _| {
            candidates
                .iter()
                .filter(|(c, _)| match end {
                    Bound::Unbounded => false,
                    Bound::Included(end) => c > end,
                    Bound::Excluded(end) => c >= end,
                })
                .count()
        })
    }

    fn prereleases(&self, name: &PackageName) -> Prereleases {
        self.package_state(name);
        self.packages.borrow()[name].prereleases
    }

    /// `GemVersionPromoter#sort_versions` with no lockfile: newest first,
    /// releases before prereleases unless prereleases were asked for.
    fn sort_preferred(&self, name: &PackageName, versions: &mut [Candidate]) {
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
    }

    /// `to_dependency_hash`: each dependency as a package and range, the
    /// ranges of one name intersected, in the order first seen.
    fn dependency_hash(
        deps: impl IntoIterator<Item = (PackageName, VersionSet)>,
    ) -> Vec<(Package, VersionSet)> {
        let mut out: Vec<(Package, VersionSet)> = Vec::new();
        for (name, range) in deps {
            let package = Package::Name(name);
            match out.iter_mut().find(|(p, _)| *p == package) {
                Some((_, existing)) => *existing = existing.intersection(&range),
                None => out.push((package, range)),
            }
        }
        out
    }

    /// `Resolver::Base#initialize` and `Resolver#prepare_dependencies`: the
    /// Gemfile's dependencies that apply on some resolution platform, each
    /// making its gem top-level, and the two meta dependencies. A Gemfile gem
    /// no candidate of which satisfies its requirement is tried with
    /// prereleases, then reported as not found.
    fn prepare_root(&mut self) -> Result<(), Failure> {
        let request = self.request;
        let mut deps: Vec<(PackageName, VersionSet)> = Vec::new();
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
                    prereleases: if dep.requirement.prerelease() {
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
            deps.push((name, requirement_to_range(&dep.requirement)));
        }
        for (meta, version) in [
            (PackageName::Ruby, &request.ruby_version),
            (PackageName::RubyGems, &request.rubygems_version),
        ] {
            let exact = Requirement::new([format!("= {version}").as_str()])
                .expect("a version makes an exact requirement");
            self.packages.borrow_mut().insert(
                meta.clone(),
                PackageState {
                    platforms: request.platforms.clone(),
                    top_level: true,
                    force_ruby_platform: false,
                    prereleases: Prereleases::Ignore,
                    candidates: None,
                },
            );
            deps.push((meta, requirement_to_range(&exact)));
        }
        let mut root = Vec::new();
        for (package, range) in Provider::dependency_hash(deps) {
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
        self.root_dependencies = root;
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

    /// The dependencies of `candidate` of `name`, without any on itself.
    fn candidate_dependencies(
        &self,
        name: &PackageName,
        candidate: &Candidate,
    ) -> Vec<(Package, VersionSet)> {
        let deps = self.with_candidates(name, |candidates, _| {
            candidates
                .iter()
                .find(|(c, _)| c == candidate)
                .map(|(_, group)| group.dependencies.clone())
                .unwrap_or_default()
        });
        Provider::dependency_hash(
            deps.into_iter()
                .filter(|(dep, _)| dep != name)
                .map(|(dep, req)| (dep, requirement_to_range(&req))),
        )
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

    /// The candidates of a package, for the report: Bundler's ranges in
    /// explanations run between them.
    fn sorted_candidates(&self, package: &Package) -> Vec<Candidate> {
        match package {
            Package::Root(_) => vec![root_candidate()],
            Package::Name(name) => {
                self.with_candidates(name, |c, _| c.iter().map(|(c, _)| c.clone()).collect())
            }
        }
    }

    /// Of the candidates of `package`, the range `Resolver#incompatibilities_for`
    /// gives the dependency on `dep` with `range` that the versions in `held`
    /// have: outward from them while each candidate's dependency on `dep` is
    /// the same, open where that reaches the first or last.
    fn dependency_span(
        &self,
        package: &Package,
        held: &VersionSet,
        dep: &Package,
        range: &VersionSet,
    ) -> Option<VersionSet> {
        let Package::Name(name) = package else {
            return None;
        };
        let sorted = self.sorted_candidates(package);
        let same = |c: &Candidate| {
            self.candidate_dependencies(name, c)
                .iter()
                .any(|(p, r)| p == dep && r == range)
        };
        let first = sorted.iter().position(|c| held.contains(c))?;
        let last = sorted.iter().rposition(|c| held.contains(c))?;
        let mut low = first;
        while low > 0 && same(&sorted[low - 1]) {
            low -= 1;
        }
        let mut high = last + 1;
        while high < sorted.len() && same(&sorted[high]) {
            high += 1;
        }
        Some(match (low, high == sorted.len()) {
            (0, true) => Ranges::full(),
            (0, false) => Ranges::strictly_lower_than(sorted[high].clone()),
            (low, true) => Ranges::higher_than(sorted[low].clone()),
            (low, false) => Ranges::between(sorted[low].clone(), sorted[high].clone()),
        })
    }

    /// The packages with prereleases left out that the conflict ran
    /// through: `Resolver#find_names_to_relax`. Bundler walks each
    /// derivation's first cause; pubgrub-rs orders a derivation's causes
    /// otherwise, so every package in the derivation is looked at.
    fn names_to_relax(&self, tree: &DerivationTree<Package, VersionSet, String>) -> Vec<String> {
        let mut names = Vec::new();
        let mut stack = vec![tree];
        while let Some(node) = stack.pop() {
            if let DerivationTree::Derived(derived) = node {
                stack.push(&derived.cause1);
                stack.push(&derived.cause2);
            }
            for package in tree_packages(node) {
                let Package::Name(name @ PackageName::Gem(gem)) = package else {
                    continue;
                };
                if self.prereleases(name) == Prereleases::Ignore
                    && self.all_specs(gem).iter().any(|s| s.version.prerelease())
                    && !names.contains(gem)
                {
                    names.push(gem.clone());
                }
            }
        }
        names
    }
}

/// The packages an incompatibility names.
fn tree_packages(tree: &DerivationTree<Package, VersionSet, String>) -> Vec<&Package> {
    match tree {
        DerivationTree::External(External::NotRoot(p, _))
        | DerivationTree::External(External::NoVersions(p, _))
        | DerivationTree::External(External::Custom(p, _, _)) => vec![p],
        DerivationTree::External(External::FromDependencyOf(p, _, d, _)) => vec![p, d],
        DerivationTree::Derived(derived) => derived.terms.keys().collect(),
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

/// The root's one version.
fn root_candidate() -> Candidate {
    Candidate::new(Version::new("0").expect("0 is a version"), -1)
}

/// Why pubgrub stopped other than a conflict; the provider never fails.
#[derive(Debug)]
pub struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unreachable")
    }
}

impl std::error::Error for Never {}

impl DependencyProvider for Provider<'_> {
    type P = Package;
    type V = Candidate;
    type VS = VersionSet;
    type M = String;
    type Err = Never;
    /// `Strategy#next_term_to_try_from`'s `min_by` key, then the order the
    /// package was found in, which `min_by` breaks ties by; reversed, since
    /// pubgrub decides the highest priority first.
    type Priority = Reverse<(u8, usize, usize)>;

    fn prioritize(
        &self,
        package: &Package,
        range: &VersionSet,
        _stats: &PackageResolutionStatistics,
    ) -> Self::Priority {
        let Package::Name(name) = package else {
            return Reverse((0, 0, 0));
        };
        let matching = self.versions_for(name, range).len();
        let higher = self.higher_versions(name, range);
        let seen = self
            .seen
            .borrow()
            .get(package)
            .copied()
            .unwrap_or(usize::MAX);
        Reverse((u8::from(matching > 1), higher, seen))
    }

    fn choose_version(
        &self,
        package: &Package,
        range: &VersionSet,
    ) -> Result<Option<Candidate>, Never> {
        let Package::Name(name) = package else {
            return Ok(Some(root_candidate()));
        };
        let mut versions = self.versions_for(name, range);
        if versions.len() > 1 {
            self.sort_preferred(name, &mut versions);
        }
        Ok(versions.into_iter().next())
    }

    fn get_dependencies(
        &self,
        package: &Package,
        version: &Candidate,
    ) -> Result<Dependencies<Package, VersionSet, String>, Never> {
        let deps = match package {
            Package::Root(_) => self.root_dependencies.clone(),
            Package::Name(name) => self.candidate_dependencies(name, version),
        };
        let mut seen = self.seen.borrow_mut();
        for (dep, _) in deps.iter().rev() {
            let next = seen.len();
            seen.entry(dep.clone()).or_insert(next);
        }
        drop(seen);
        Ok(Dependencies::Available(deps.into_iter().collect()))
    }
}

/// Resolve `request` against `index`, as `Bundler::Resolver#start` does with
/// no lockfile: the builds to install, in the lockfile's order (`full_name`),
/// or why there are none.
pub fn resolve(request: &Request, index: &Index) -> Result<Vec<Resolved>, Failure> {
    // ponytail: no lockfile, so every gem is free to move and `bundler` is
    // resolved from the index like any gem. Upgrade: locked versions as the
    // only candidates of unchanged gems (#79, `@base`), and `bundler` pinned
    // to the version the lockfile writer emulates (#80).
    let mut relaxed: HashSet<String> = HashSet::new();
    loop {
        let mut provider = Provider::new(request, index, &relaxed);
        provider.prepare_root()?;
        let root = Package::Root(request.gemfile.clone());
        match pubgrub::resolve(&provider, root, root_candidate()) {
            Ok(solution) => {
                let mut resolved = Vec::new();
                for (package, candidate) in solution {
                    let Package::Name(name @ PackageName::Gem(_)) = &package else {
                        continue;
                    };
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
                return Ok(resolved);
            }
            Err(PubGrubError::NoSolution(tree)) => {
                let names = provider.names_to_relax(&tree);
                if !names.is_empty() {
                    relaxed.extend(names);
                    continue;
                }
                let explanation = report::explain(&tree, &provider);
                return Err(Failure::Conflict(format!(
                    "Could not find compatible versions\n\n{explanation}"
                )));
            }
            // The provider never fails, so the solver has nothing else to
            // pass on.
            Err(error) => return Err(Failure::Conflict(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests;
