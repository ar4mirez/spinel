//! Locked preferences (#79, docs/package-manager.md, "Resolver"): what
//! Bundler 4's `Definition` makes of a `Gemfile.lock` that exists when no
//! update was asked for, as `bundle install` and `bundle lock` read one.
//!
//! [`Definition::new`] compares the lockfile with the Gemfile as
//! `Definition#initialize` does. A Gemfile gem that is new, or whose locked
//! version no longer meets its requirement, is a changed dependency, and
//! one of the latter is unlocked (`converge_dependencies`); a locked spec
//! whose dependencies the lockfile does not meet is dropped
//! (`check_lockfile`); the local platform is added when the lockfile has
//! nothing that runs here (`add_current_platform`). When nothing changed,
//! the lockfile's own specs are the resolution, and the index is never read
//! (`Definition#resolve`'s `no_resolve_needed?`); a gem taken out of the
//! Gemfile takes the gems only it needed with it. Otherwise
//! [`Definition::start_resolution`] resolves with the lockfile's hold
//! ([`resolver::Locked`], `Definition#resolution_base`) and keeps what
//! `SpecSet#for` keeps of the result on the lockfile's platforms.
//! [`Definition::lockfile`] writes it as `LockfileGenerator` writes it: the
//! lockfile's platforms, its checksums and its `RUBY VERSION` where they
//! still hold.

use std::collections::{HashMap, HashSet};

use crate::gem_platform::{self, GemPlatform};
use crate::gem_requirement::Requirement;
use crate::gem_version::Version;
use crate::lockfile::{
    self, BUNDLER_VERSION, Checksum, Dependency, LockedSpec, Lockfile, LockfileError, Source,
    SourceKind,
};
use crate::resolver::{self, Failure, GemfileDependency, Index, Locked, Request, Spec};

/// A spec the lockfile has, as the resolver reads it.
#[derive(Debug, Clone)]
struct LockedBuild {
    /// The lockfile's line: its dependencies, and no Ruby requirement, as a
    /// `LazySpecification` read from a lockfile has none.
    spec: Spec,
    /// Whether it is locked from a `GEM` section, which has checksums.
    rubygems: bool,
    /// Whether its source is still the Gemfile's (`!source_changed?`).
    kept: bool,
}

/// `Definition`, for a lockfile that exists and a Gemfile with one rubygems
/// source.
#[derive(Debug, Clone)]
pub struct Definition {
    /// `Bundler.local_platform`.
    local: GemPlatform,
    /// `@platforms`: the lockfile's, and those added.
    platforms: Vec<GemPlatform>,
    /// Platforms added that the lockfile does not have.
    new_platforms: Vec<GemPlatform>,
    current_platform_missing: bool,
    /// The lockfile's platform nearest the local one, when the local one is
    /// not locked but something locked runs here.
    most_specific_non_local: Option<GemPlatform>,
    /// `@locked_specs`, sorted by `full_name` as `LockfileParser` sorts them,
    /// less what `check_lockfile` took out.
    locked: Vec<LockedBuild>,
    /// The lockfile's `CHECKSUMS`; `None` when it has none, and then none
    /// is written (`@locked_checksums`).
    checksums: Option<Vec<Checksum>>,
    gems_to_unlock: Vec<String>,
    changed_dependencies: Vec<String>,
    resolve_needed: bool,
    /// A `DEPENDENCIES` line no Gemfile dependency equals (`deleted_deps`).
    deleted_deps: bool,
    /// `may_include_redundant_platform_specific_gems?`.
    redundant_platform_gems: bool,
    /// `RUBY VERSION` as the lockfile has it.
    locked_ruby: Option<String>,
    unlocking_ruby: bool,
}

/// `Gem::Dependency#matches_spec?`: any version for no requirement.
fn matches(requirement: &Requirement, version: &Version) -> bool {
    requirement.none() || requirement.satisfied_by(version)
}

/// `RubyVersion.from_string`'s version: `ruby 3.4.1p0` is 3.4.1.
fn locked_ruby_version(locked: &str) -> Option<Version> {
    let rest = locked.strip_prefix("ruby ")?;
    let word = rest.split(' ').next()?;
    let version = match word.rfind('p') {
        Some(at)
            if at > 0
                && word.as_bytes()[at - 1].is_ascii_digit()
                && word[at + 1..]
                    .trim_start_matches('-')
                    .bytes()
                    .all(|b| b.is_ascii_digit())
                && word.len() > at + 1 =>
        {
            &word[..at]
        }
        _ => word,
    };
    Version::new(version).ok()
}

impl Definition {
    /// `Definition#initialize` for `lock` and a Gemfile of `dependencies`
    /// from the rubygems source at `remote`, with a `ruby` directive of
    /// `ruby` (its version requirements) or none, on `local`; `add` are
    /// platforms to add, as `bundle lock --add-platform` adds them.
    pub fn new(
        lock: &Lockfile,
        remote: &str,
        dependencies: &[GemfileDependency],
        ruby: Option<&[String]>,
        local: &GemPlatform,
        add: &[GemPlatform],
    ) -> Result<Definition, LockfileError> {
        // `converge_sources`: the lockfile's source is the Gemfile's when it
        // is its one `GEM` section, with the Gemfile's remote.
        // ponytail: one rubygems source, compared by its one remote. Upgrade
        // with `Source::Rubygems#==` over several remotes, and git and path
        // sources (#88).
        let remote = normalized_remote(remote);
        let same_source = |source: &Source| matches!(&source.kind, SourceKind::Gem { remotes } if remotes == std::slice::from_ref(&remote));
        let source_changes = !(lock.sources.len() == 1 && same_source(&lock.sources[0]));
        let mut locked: Vec<LockedBuild> = lock
            .sources
            .iter()
            .flat_map(|source| {
                let rubygems = matches!(source.kind, SourceKind::Gem { .. });
                let kept = same_source(source);
                source.specs.iter().map(move |s| LockedBuild {
                    spec: locked_spec(s),
                    rubygems,
                    kept,
                })
            })
            .collect();
        locked.sort_by_cached_key(|b| b.spec.full_name());

        // `@unlocking_ruby`: the directive came or went, or no longer
        // allows the locked Ruby (`RubyVersion#diff`).
        // ponytail: the version alone is compared. Upgrade with the
        // engine, engine version and patchlevel of `RubyVersion#diff`.
        let unlocking_ruby = match (ruby, &lock.ruby_version) {
            (Some(versions), Some(locked_ruby)) => {
                let Some(version) = locked_ruby_version(locked_ruby) else {
                    return Err(LockfileError(format!(
                        "The Ruby version {locked_ruby} from Gemfile.lock could not be parsed. \
                         Try running bundle update --ruby to resolve this."
                    )));
                };
                versions.is_empty()
                    || !versions.iter().all(|v| {
                        Requirement::new([v.as_str()]).is_ok_and(|r| r.satisfied_by(&version))
                    })
            }
            (ruby, locked_ruby) => ruby.is_some() != locked_ruby.is_some(),
        };

        // `add_current_platform`.
        let mut platforms = lock.platforms.clone();
        let mut current_platform_missing = false;
        let mut most_specific_non_local = None;
        if !platforms.contains(local) {
            let generic = gem_platform::generic(local);
            let runs_here = platforms.iter().any(|p| {
                *p == generic
                    || matches!((local, p), (GemPlatform::Platform(l), GemPlatform::Platform(p)) if l.matches(p))
            });
            if runs_here {
                // `LockfileParser#most_specific_locked_platform`.
                most_specific_non_local = lock
                    .platforms
                    .iter()
                    .min_by_key(|p| gem_platform::platform_specificity_match(p, local))
                    .cloned();
            } else {
                platforms.push(local.clone());
                current_platform_missing = true;
            }
        }
        // `add_platform`.
        let mut new_platforms = Vec::new();
        for platform in add {
            if !platforms.contains(platform) {
                new_platforms.push(platform.clone());
                platforms.push(platform.clone());
            }
        }

        // `converge_dependencies`, for the dependencies relevant here.
        let mut gems_to_unlock = Vec::new();
        let mut changed_dependencies = Vec::new();
        let mut missing_lockfile_dep = false;
        let generic_local = [gem_platform::generic(local)];
        for dep in dependencies {
            if dep.gem_platforms(&generic_local).is_empty() || !dep.current_platform() {
                continue;
            }
            let mut changed = !lock.dependencies.iter().any(|d| d.name == dep.name);
            if dep.name != "bundler" {
                // `@originally_locked_specs[name].first`: the last of them.
                match locked.iter().rev().find(|b| b.spec.name == dep.name) {
                    None => missing_lockfile_dep |= !changed,
                    Some(build) => {
                        if !matches(&dep.requirement, &build.spec.version) {
                            gems_to_unlock.push(dep.name.clone());
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                changed_dependencies.push(dep.name.clone());
            }
        }

        // `check_lockfile`.
        let checksum = |b: &LockedBuild| {
            lock.checksums.as_ref().map(|sums| {
                let name = lockfile::lock_name(&b.spec.name, &b.spec.version, &b.spec.platform);
                sums.iter()
                    .find(|c| c.lock_name == name)
                    .map(|c| c.checksums.is_empty())
            })
        };
        let missing_checksums = locked.iter().any(|b| checksum(b) == Some(None));
        let empty_checksums = locked
            .iter()
            .any(|b| b.rubygems && checksum(b) == Some(Some(true)));
        let specs: Vec<Spec> = locked.iter().map(|b| b.spec.clone()).collect();
        let names: HashSet<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        let mut bad_deps = false;
        let bad: Vec<String> = specs
            .iter()
            .filter(|s| {
                !s.dependencies.iter().all(|(name, req)| {
                    name == "bundler"
                        || (names.contains(name.as_str())
                            && specs
                                .iter()
                                .any(|o| &o.name == name && matches(req, &o.version)))
                })
            })
            .map(Spec::full_name)
            .collect();
        if !bad.is_empty() {
            bad_deps = true;
            locked.retain(|b| !bad.contains(&b.spec.full_name()));
        }

        let something_changed = source_changes
            || !changed_dependencies.is_empty()
            || current_platform_missing
            || !new_platforms.is_empty()
            || missing_lockfile_dep
            || missing_checksums
            || empty_checksums
            || bad_deps;
        let deleted_deps = lock.dependencies.iter().any(|locked_dep| {
            !dependencies
                .iter()
                .any(|d| d.name == locked_dep.name && d.requirement == locked_dep.requirement)
        });
        let redundant_platform_gems = lock
            .bundled_with
            .as_ref()
            .is_none_or(|v| *v < Version::new("1.16.2").expect("a version"));
        Ok(Definition {
            local: local.clone(),
            platforms,
            new_platforms,
            current_platform_missing,
            most_specific_non_local,
            locked,
            checksums: lock.checksums.clone(),
            gems_to_unlock,
            changed_dependencies,
            resolve_needed: unlocking_ruby || something_changed,
            deleted_deps,
            redundant_platform_gems,
            locked_ruby: lock.ruby_version.clone(),
            unlocking_ruby,
        })
    }

    /// `resolve_needed?`: whether the Gemfile asks for something the
    /// lockfile does not hold.
    pub fn resolve_needed(&self) -> bool {
        self.resolve_needed
    }

    /// `@platforms`: the resolution platforms.
    pub fn platforms(&self) -> &[GemPlatform] {
        &self.platforms
    }

    /// The resolution when nothing changed: the lockfile's specs, or, when
    /// a dependency was taken out (or the lockfile is older than Bundler
    /// 1.16.2), what `SpecSet#for` of the Gemfile keeps of them.
    pub fn unchanged(&self, dependencies: &[GemfileDependency]) -> Vec<Spec> {
        let specs: Vec<Spec> = self.locked.iter().map(|b| b.spec.clone()).collect();
        if self.deleted_deps || self.redundant_platform_gems {
            lockfile::spec_set_for(&specs, dependencies, &self.platforms)
        } else {
            specs
        }
    }

    /// `Definition#resolution_base`: `converge_locked_specs`, then the base
    /// requirements and `additional_base_requirements_to_prevent_downgrades`.
    fn resolution_base(&self, dependencies: &[GemfileDependency]) -> Locked {
        // `converge_specs`: the locked specs a Gemfile dependency they meet
        // reaches, through no unlocked gem, from a source still the
        // Gemfile's.
        let mut converged: Vec<Spec> = Vec::new();
        let mut deps: Vec<GemfileDependency> = Vec::new();
        for build in &self.locked {
            if self.gems_to_unlock.contains(&build.spec.name) {
                continue;
            }
            // `LazySpecification#satisfies?`.
            if let Some(dep) = dependencies
                .iter()
                .find(|d| d.name == build.spec.name && matches(&d.requirement, &build.spec.version))
            {
                deps.push(dep.clone());
            }
            if build.kept {
                converged.push(build.spec.clone());
            }
        }
        let base = lockfile::spec_set_for(&converged, &deps, &self.platforms);

        let mut requirements: HashMap<String, Requirement> = HashMap::new();
        let requirement = |op: &str, version: &Version| {
            Requirement::new([format!("{op} {version}").as_str()]).expect("a version makes one")
        };
        for spec in &base {
            requirements.insert(spec.name.clone(), requirement("=", &spec.version));
        }
        for build in &self.locked {
            if !build.kept || self.changed_dependencies.contains(&build.spec.name) {
                continue;
            }
            requirements.insert(
                build.spec.name.clone(),
                requirement(">=", &build.spec.version),
            );
        }
        let mut new_platforms = self.new_platforms.clone();
        if self.current_platform_missing {
            new_platforms.push(self.local.clone());
        }
        Locked {
            versions: self
                .locked
                .iter()
                .map(|b| (b.spec.name.clone(), b.spec.version.clone()))
                .collect(),
            base,
            requirements,
            unlock: self.gems_to_unlock.iter().cloned().collect(),
            new_platforms,
            locked_specs: self.locked.iter().map(|b| b.spec.clone()).collect(),
        }
    }

    /// `Definition#start_resolution`: resolve `request` against `index` on
    /// the lockfile's platforms, the local one with them for the resolution
    /// when only a less specific one is locked, then keep what `SpecSet#for`
    /// of the Gemfile keeps on those platforms and `ruby`.
    // ponytail: a platform added that the result has no builds for is
    // written anyway, and a locked platform the lockfile's specs cannot
    // install on is kept. Upgrade with `raise_incomplete!` for new platforms
    // and `remove_invalid_platforms!`.
    pub fn start_resolution(
        &mut self,
        request: &Request,
        index: &Index,
    ) -> Result<Vec<Spec>, Failure> {
        let for_resolvability =
            self.most_specific_non_local.is_some() && !self.platforms.contains(&self.local);
        if for_resolvability {
            self.platforms.push(self.local.clone());
        }
        let mut request = request.clone();
        request.platforms = self.platforms.clone();
        let base = self.resolution_base(&request.dependencies);
        let result = resolver::resolve_locked(&request, index, &base)?;
        if let Some(most_specific) = self.most_specific_non_local.clone() {
            let generic_local = [gem_platform::generic(&self.local)];
            let current: Vec<GemfileDependency> = request
                .dependencies
                .iter()
                .filter(|d| !d.gem_platforms(&generic_local).is_empty() && d.current_platform())
                .cloned()
                .collect();
            if lockfile::incomplete_for_platform(&result, &current, &most_specific) {
                self.platforms.retain(|p| *p != most_specific);
            } else if for_resolvability {
                let local = self.local.clone();
                self.platforms.retain(|p| *p != local);
            }
        }
        let mut platforms = self.platforms.clone();
        if !platforms.contains(&GemPlatform::Ruby) {
            platforms.push(GemPlatform::Ruby);
        }
        Ok(lockfile::spec_set_for(
            &result,
            &request.dependencies,
            &platforms,
        ))
    }

    /// The lockfile `LockfileGenerator` writes for `specs`, a resolution of
    /// `request` from the rubygems source at `remote`: the platforms, the
    /// Gemfile's dependencies, a `CHECKSUMS` line for each spec when the
    /// lockfile had the section (the lockfile's checksums, and the index's
    /// SHA-256 for a spec it had none for), the locked `RUBY VERSION` unless
    /// the directive changed, and `BUNDLED WITH` the Bundler written for.
    /// `index` is `None` when nothing was resolved.
    pub fn lockfile(
        &self,
        remote: &str,
        request: &Request,
        index: Option<&Index>,
        specs: &[Spec],
        ruby_directive: bool,
    ) -> Lockfile {
        let checksums = self.checksums.as_ref().map(|locked| {
            let mut lines: Vec<Checksum> = specs
                .iter()
                .map(|s| {
                    let lock_name = lockfile::lock_name(&s.name, &s.version, &s.platform);
                    let mut sums: Vec<(String, String)> = locked
                        .iter()
                        .find(|c| c.lock_name == lock_name)
                        .map(|c| c.checksums.clone())
                        .unwrap_or_default();
                    let indexed = s.checksum.clone().or_else(|| {
                        index?
                            .get(&s.name)?
                            .iter()
                            .find(|i| {
                                i.version.as_str() == s.version.as_str() && i.platform == s.platform
                            })?
                            .checksum
                            .clone()
                    });
                    if let Some(sha) = indexed
                        && !sums.iter().any(|(algo, _)| algo == "sha256")
                    {
                        sums.push(("sha256".to_owned(), sha));
                    }
                    Checksum {
                        lock_name,
                        checksums: sums,
                    }
                })
                .collect();
            // `bundler_checksum`: the lockfile's, for the Bundler written for.
            let bundler = format!("bundler ({BUNDLER_VERSION})");
            lines.extend(locked.iter().filter(|c| c.lock_name == bundler).cloned());
            lines
        });
        // `Definition#locked_ruby_version`.
        let ruby_version = ruby_directive.then(|| match (&self.locked_ruby, self.unlocking_ruby) {
            (Some(locked), false) => locked.clone(),
            _ => format!("ruby {}", request.ruby_version),
        });
        Lockfile {
            sources: vec![Source {
                kind: SourceKind::Gem {
                    remotes: vec![normalized_remote(remote)],
                },
                specs: specs
                    .iter()
                    .map(|s| LockedSpec {
                        name: s.name.clone(),
                        version: s.version.clone(),
                        platform: s.platform.clone(),
                        dependencies: s.dependencies.clone(),
                    })
                    .collect(),
            }],
            platforms: self.platforms.clone(),
            dependencies: request
                .dependencies
                .iter()
                .map(|d| Dependency {
                    name: d.name.clone(),
                    requirement: d.requirement.clone(),
                    pinned: false,
                })
                .collect(),
            checksums,
            ruby_version,
            bundled_with: Some(Version::new(BUNDLER_VERSION).expect("a version")),
        }
    }
}

/// A remote as a lockfile's `GEM` section has it: no credentials, a
/// trailing `/`.
fn normalized_remote(remote: &str) -> String {
    let mut remote = lockfile::credless(remote);
    if !remote.ends_with('/') {
        remote.push('/');
    }
    remote
}

/// A lockfile spec as the resolver reads it.
fn locked_spec(spec: &LockedSpec) -> Spec {
    Spec {
        name: spec.name.clone(),
        version: spec.version.clone(),
        platform: spec.platform.clone(),
        dependencies: spec.dependencies.clone(),
        required_ruby_version: Requirement::default(),
        required_rubygems_version: Requirement::default(),
        checksum: None,
    }
}

#[cfg(test)]
mod tests;
