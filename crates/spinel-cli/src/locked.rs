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
//! still hold; [`Definition::write_lock`] leaves the lockfile as it was
//! when that differs from it only where `write_lock` looks past.
//!
//! An update (#87, `bundle update`, `bundle lock --update`) is an
//! [`Unlock`] passed to [`Definition::unlocking`]: every gem, with the
//! lockfile's specs no longer the base, or the gems named and every locked
//! gem they reach (`@gems_to_unlock` from `SpecSet#for`), each named one
//! then pinned to the version a resolution with everything unlocked gives
//! it (`additional_base_requirements_to_force_updates`). The floors that
//! keep a locked gem from going down stay either way.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::gem_platform::{self, GemPlatform};
use crate::gem_requirement::Requirement;
use crate::gem_version::Version;
use crate::local_source::{Declared, DeclaredSource, Materialized};
use crate::lockfile::LockSource;
use crate::lockfile::{
    self, BUNDLER_VERSION, Checksum, LockedSpec, Lockfile, LockfileError, Source, SourceKind,
};
use crate::resolver::{self, Failure, GemfileDependency, Index, Locked, Request, Spec};

/// The Gemfile's git and path sources (#88), as `Definition` compares them
/// with the lockfile's.
#[derive(Debug, Clone, Copy)]
pub struct Locals<'a> {
    pub sources: &'a [DeclaredSource],
    /// Each path source's gems, read now; `None` for a git source, which is
    /// read once its revision is known.
    pub read: &'a [Option<Materialized>],
    /// For each Gemfile dependency, the source it is pinned to.
    pub pins: &'a [Option<usize>],
    /// `Bundler.root`, which a path source's `remote:` is relative to.
    pub project: &'a Path,
}

impl Locals<'static> {
    /// A Gemfile with no git or path source.
    pub fn none() -> Locals<'static> {
        Locals {
            sources: &[],
            read: &[],
            pins: &[],
            project: Path::new(""),
        }
    }
}

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

/// What an update unlocks: `Definition#initialize`'s `unlock`, as `bundle
/// update` and `bundle lock --update` pass it.
// ponytail: no `--conservative`, `--patch`, `--minor`, `--major`, `--strict`,
// `--pre`, `--group`, `--source`, `--ruby` or `--bundler`. Upgrade: the
// conservative unlock of `Definition#initialize` and the levels of
// `GemVersionPromoter`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Unlock {
    /// `bundle install`: nothing asked for.
    #[default]
    Nothing,
    /// `unlock == true`: `bundle update` with no gems (`@unlocking_all`).
    All,
    /// `{gems: [...]}`: `bundle update GEM...` (`@explicit_unlocks`).
    Gems(Vec<String>),
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
    /// `@unlocking_all`: every gem unlocked, the lockfile's specs no base.
    unlocking_all: bool,
    /// `@explicit_unlocks`: the gems an update names.
    explicit_unlocks: Vec<String>,
    /// `unlocking?`: an update, or the `ruby` directive unlocking Ruby.
    unlocking: bool,
    changed_dependencies: Vec<String>,
    resolve_needed: bool,
    /// Why a resolution is needed, as `lockfile_changed_reason` names it.
    reasons: Reasons,
    /// A `DEPENDENCIES` line no Gemfile dependency equals (`deleted_deps`).
    deleted_deps: bool,
    /// `may_include_redundant_platform_specific_gems?`.
    redundant_platform_gems: bool,
    /// `RUBY VERSION` as the lockfile has it.
    locked_ruby: Option<String>,
    unlocking_ruby: bool,
    /// Whether the Gemfile has a `ruby` directive.
    ruby_directive: bool,
    /// `@locked_bundler_version`: the lockfile's `BUNDLED WITH`.
    locked_bundler: Option<Version>,
    /// For each git source of the Gemfile, the revision it stays at: the
    /// locked one, unless the source is new or unlocked (`unlock!`).
    locked_revisions: Vec<Option<String>>,
    /// `@path_changes`: a path source's gemspecs no longer what the
    /// lockfile has (`converge_paths`).
    path_changes: bool,
    /// What `lockfile_changes_summary` lists under `You have changed in the
    /// Gemfile`: a gem whose source is not the lockfile's.
    source_moves: Vec<String>,
}

/// What `Definition#initialize` notes as it converges the lockfile with the
/// Gemfile, each named as `lockfile_changed_reason` names it.
#[derive(Debug, Clone, Default)]
struct Reasons {
    /// `@source_changes`.
    source_changes: bool,
    /// `@missing_lockfile_dep`: a Gemfile gem the lockfile lists in
    /// `DEPENDENCIES` but has no spec for.
    missing_lockfile_dep: Option<String>,
    /// `@locked_spec_with_missing_checksums`.
    missing_checksums: Option<String>,
    /// `@locked_spec_with_empty_checksums`.
    empty_checksums: Option<String>,
    /// `@locked_spec_with_missing_deps`: a spec depending on a gem the
    /// lockfile does not have.
    missing_deps: Option<String>,
    /// `@locked_spec_with_invalid_deps`: a spec whose dependency no locked
    /// version meets.
    invalid_deps: Option<String>,
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
    /// `unlock` is what an update unlocks, [`Unlock::Nothing`] when none
    /// was asked for.
    pub fn unlocking(
        lock: &Lockfile,
        remote: &str,
        dependencies: &[GemfileDependency],
        ruby: Option<&[String]>,
        local: &GemPlatform,
        add: &[GemPlatform],
        unlock: &Unlock,
    ) -> Result<Definition, LockfileError> {
        Definition::with_locals(
            lock,
            remote,
            dependencies,
            ruby,
            local,
            add,
            unlock,
            Locals::none(),
        )
    }

    /// [`Definition::unlocking`] for a Gemfile with git and path sources.
    #[allow(clippy::too_many_arguments)]
    pub fn with_locals(
        lock: &Lockfile,
        remote: &str,
        dependencies: &[GemfileDependency],
        ruby: Option<&[String]>,
        local: &GemPlatform,
        add: &[GemPlatform],
        unlock: &Unlock,
        locals: Locals<'_>,
    ) -> Result<Definition, LockfileError> {
        let unlocking_all = *unlock == Unlock::All;
        let explicit_unlocks = match unlock {
            Unlock::Gems(gems) => gems.clone(),
            _ => Vec::new(),
        };
        // `converge_sources`: the lockfile's rubygems source is the
        // Gemfile's when it is its one `GEM` section, with the Gemfile's
        // remote; each git and path source is the lockfile's that is `==`
        // to it (`replace_sources!`), and the list changed unless every
        // source is one the lockfile has, and it has no other.
        // ponytail: one rubygems source, compared by its one remote. Upgrade
        // with `Source::Rubygems#==` over several remotes.
        let wanted = lockfile::gem_remotes(remote);
        let same_source = |source: &Source| matches!(&source.kind, SourceKind::Gem { remotes } if *remotes == wanted);
        let mut matched: Vec<Option<usize>> = Vec::new();
        for declared in locals.sources {
            let found = lock.sources.iter().enumerate().position(|(i, s)| {
                !matched.contains(&Some(i)) && declared.kind.same_as_locked(&s.kind, locals.project)
            });
            matched.push(found);
        }
        let gem_sources: Vec<&Source> = lock
            .sources
            .iter()
            .filter(|s| matches!(s.kind, SourceKind::Gem { .. }))
            .collect();
        let others_matched =
            lock.sources.iter().enumerate().all(|(i, s)| {
                matches!(s.kind, SourceKind::Gem { .. }) || matched.contains(&Some(i))
            });
        let source_changes = !(gem_sources.len() == 1
            && same_source(gem_sources[0])
            && others_matched
            && matched.iter().all(Option::is_some));
        let kept_source = |i: usize| same_source(&lock.sources[i]) || matched.contains(&Some(i));
        let mut locked: Vec<LockedBuild> = lock
            .sources
            .iter()
            .enumerate()
            .flat_map(|(i, source)| {
                let rubygems = matches!(source.kind, SourceKind::Gem { .. });
                let kept = kept_source(i);
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

        // `@gems_to_unlock`: the gems named, and every locked gem they
        // reach (`@locked_specs.for(eager_unlock, platforms)`), which with
        // everything unlocked is none, as `@locked_specs` is then empty.
        let mut gems_to_unlock: Vec<String> = Vec::new();
        if !unlocking_all && !explicit_unlocks.is_empty() {
            let specs: Vec<Spec> = locked.iter().map(|b| b.spec.clone()).collect();
            let eager: Vec<GemfileDependency> = explicit_unlocks
                .iter()
                .map(|name| GemfileDependency {
                    name: name.clone(),
                    requirement: Requirement::default(),
                    platforms: Vec::new(),
                    force_ruby_platform: false,
                })
                .collect();
            for spec in lockfile::spec_set_for(&specs, &eager, &platforms) {
                if !gems_to_unlock.contains(&spec.name) {
                    gems_to_unlock.push(spec.name);
                }
            }
        }

        // `converge_dependencies`, for the dependencies relevant here.
        let mut changed_dependencies = Vec::new();
        let mut missing_lockfile_dep = None;
        let generic_local = [gem_platform::generic(local)];
        for dep in dependencies {
            if dep.gem_platforms(&generic_local).is_empty() || !dep.current_platform() {
                continue;
            }
            let mut changed = !lock.dependencies.iter().any(|d| d.name == dep.name);
            if dep.name != "bundler" {
                // `@originally_locked_specs[name].first`: the last of them.
                match locked.iter().rev().find(|b| b.spec.name == dep.name) {
                    None if !changed => missing_lockfile_dep = Some(dep.name.clone()),
                    None => {}
                    Some(build) if !matches(&dep.requirement, &build.spec.version) => {
                        gems_to_unlock.push(dep.name.clone());
                        changed = true;
                    }
                    Some(_) => {}
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
        let missing_checksums = locked
            .iter()
            .find(|b| checksum(b) == Some(None))
            .map(|b| b.spec.name.clone());
        let empty_checksums = locked
            .iter()
            .find(|b| b.rubygems && checksum(b) == Some(Some(true)))
            .map(|b| b.spec.name.clone());
        let specs: Vec<Spec> = locked.iter().map(|b| b.spec.clone()).collect();
        let names: HashSet<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        // `SpecSet#validate_deps`: the first dependency that fails says
        // which, a gem missing or a version no locked spec meets.
        let validate = |s: &Spec| {
            for (name, req) in &s.dependencies {
                if name == "bundler" {
                    continue;
                }
                if !names.contains(name.as_str()) {
                    return Some(true);
                }
                if !specs
                    .iter()
                    .any(|o| &o.name == name && matches(req, &o.version))
                {
                    return Some(false);
                }
            }
            None
        };
        let mut missing_deps = Vec::new();
        let mut invalid_deps = Vec::new();
        for spec in &specs {
            match validate(spec) {
                Some(true) => missing_deps.push(spec),
                Some(false) => invalid_deps.push(spec),
                None => {}
            }
        }
        let bad: Vec<String> = missing_deps
            .iter()
            .chain(&invalid_deps)
            .map(|s| s.full_name())
            .collect();
        let bad_deps = !bad.is_empty();
        let reasons = Reasons {
            source_changes,
            missing_lockfile_dep,
            missing_checksums,
            empty_checksums,
            missing_deps: missing_deps.first().map(|s| s.name.clone()),
            invalid_deps: invalid_deps.first().map(|s| s.name.clone()),
        };
        // With everything unlocked `check_lockfile` reads `@locked_specs`,
        // which is then empty: nothing is dropped, and the floors still see
        // every locked spec.
        if bad_deps && !unlocking_all {
            locked.retain(|b| !bad.contains(&b.spec.full_name()));
        }

        // `converge_paths`: a path source changed when the lockfile has no
        // source `==` to it, the Gemfile gems pinned to it are not the ones
        // the lockfile pins there (`dependencies_for_source_changed?`), or
        // a spec the lockfile has from it is not one it has now, with the
        // same dependencies (`specs_for_source_changed?`). And
        // `converge_specs`: a locked spec of a path source takes the
        // dependencies its gemspec has now, and one the source no longer
        // has is unlocked.
        let mut path_changes = false;
        for (i, declared) in locals.sources.iter().enumerate() {
            if !matches!(declared.kind, Declared::Path { .. }) {
                continue;
            }
            let Some(at) = matched[i] else {
                path_changes = true;
                continue;
            };
            let source = &lock.sources[at];
            let mut gemfile_deps: Vec<String> = dependencies
                .iter()
                .zip(locals.pins)
                .filter(|(_, pin)| **pin == Some(i))
                .map(|(d, _)| format!("{} ({})", d.name, d.requirement))
                .collect();
            let mut locked_deps: Vec<String> = lock
                .dependencies
                .iter()
                .filter(|d| d.pinned && source.specs.iter().any(|s| s.name == d.name))
                .map(|d| format!("{} ({})", d.name, d.requirement))
                .collect();
            gemfile_deps.sort();
            gemfile_deps.dedup();
            locked_deps.sort();
            if gemfile_deps != locked_deps {
                path_changes = true;
            }
            let Some(read) = locals.read.get(i).and_then(Option::as_ref) else {
                continue;
            };
            let dep_set = |deps: &[(String, Requirement)]| {
                let mut set: Vec<String> = deps.iter().map(|(n, r)| format!("{n} ({r})")).collect();
                set.sort();
                set
            };
            for spec in &source.specs {
                let full_name = spec.full_name();
                let now = read.gems.iter().find(|g| g.full_name() == full_name);
                if now.is_none_or(|g| dep_set(&g.spec.dependencies) != dep_set(&spec.dependencies))
                {
                    path_changes = true;
                }
                for build in locked
                    .iter_mut()
                    .filter(|b| b.spec.full_name() == full_name)
                {
                    match now {
                        Some(gem) => build.spec.dependencies = gem.spec.dependencies.clone(),
                        None => {
                            if !gems_to_unlock.contains(&spec.name) {
                                gems_to_unlock.push(spec.name.clone());
                            }
                        }
                    }
                }
            }
        }

        // `converge_locked_specs`: a git source keeps its locked revision
        // unless everything is unlocked, or every gem the lockfile has from
        // it is (`bundle update` of its gems), when the revision its ref is
        // at now is taken (`unlock!`).
        let locked_revisions: Vec<Option<String>> = locals
            .sources
            .iter()
            .zip(&matched)
            .map(|(declared, at)| {
                let source = &lock.sources[(*at)?];
                let SourceKind::Git { revision, .. } = &source.kind else {
                    return None;
                };
                if !matches!(declared.kind, Declared::Git { .. }) || unlocking_all {
                    return None;
                }
                let all_unlocked = !source.specs.is_empty()
                    && source
                        .specs
                        .iter()
                        .all(|s| gems_to_unlock.contains(&s.name));
                (!all_unlocked).then(|| revision.clone())
            })
            .collect();

        // `lockfile_changes_summary`'s changed sources: a gem the Gemfile
        // and the lockfile both list, from sources that are not the same
        // (`include?`), as `to_gemfile` names them.
        let mut source_moves = Vec::new();
        for (dep, pin) in dependencies
            .iter()
            .zip(locals.pins.iter().chain(std::iter::repeat(&None)))
        {
            let Some(lock_dep) = lock.dependencies.iter().find(|d| d.name == dep.name) else {
                continue;
            };
            let lock_source = lock_dep
                .pinned
                .then(|| {
                    lock.sources
                        .iter()
                        .position(|s| s.specs.iter().any(|sp| sp.name == dep.name))
                })
                .flatten();
            let same = match (*pin, lock_source) {
                (None, None) => true,
                (Some(i), Some(at)) => {
                    matched[i] == Some(at)
                        || locals.sources[i]
                            .kind
                            .includes_locked(&lock.sources[at].kind, locals.project)
                }
                _ => false,
            };
            if !same {
                let named =
                    |s: Option<String>| s.unwrap_or_else(|| "no specified source".to_owned());
                source_moves.push(format!(
                    "* {} from `{}` to `{}`",
                    dep.name,
                    named(lock_source.map(|at| lock.sources[at].kind.to_gemfile())),
                    named(pin.map(|i| locals.sources[i].kind.to_gemfile())),
                ));
            }
        }

        let something_changed = source_changes
            || !changed_dependencies.is_empty()
            || current_platform_missing
            || !new_platforms.is_empty()
            || path_changes
            || reasons.missing_lockfile_dep.is_some()
            || reasons.missing_checksums.is_some()
            || reasons.empty_checksums.is_some()
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
            unlocking_all,
            unlocking: unlocking_all || !explicit_unlocks.is_empty() || unlocking_ruby,
            resolve_needed: unlocking_all
                || !explicit_unlocks.is_empty()
                || unlocking_ruby
                || something_changed,
            explicit_unlocks,
            changed_dependencies,
            reasons,
            deleted_deps,
            redundant_platform_gems,
            locked_ruby: lock.ruby_version.clone(),
            unlocking_ruby,
            ruby_directive: ruby.is_some(),
            locked_bundler: lock.bundled_with.clone(),
            locked_revisions,
            path_changes,
            source_moves,
        })
    }

    /// The revision the Gemfile's git source `i` stays at: its locked one,
    /// unless it is new or unlocked.
    pub fn locked_revision(&self, i: usize) -> Option<&str> {
        self.locked_revisions.get(i)?.as_deref()
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

    /// `lockfile_changes_summary("frozen mode is set")`, which
    /// `ensure_equivalent_gemfile_and_lockfile` raises for an explicit
    /// `--frozen`: why the lockfile would change and what the Gemfile added
    /// and deleted, or `None` when it would not. `lock` is the lockfile this
    /// was made from, `dependencies` every Gemfile dependency, and
    /// `lockfile_name` the lockfile as the message names it. The command
    /// that would update it is `spinel install`.
    pub fn frozen_summary(
        &self,
        lock: &Lockfile,
        dependencies: &[GemfileDependency],
        lockfile_name: &str,
    ) -> Option<String> {
        // `SharedHelpers.pretty_dependency`.
        let pretty = |name: &str, requirement: &Requirement, platforms: &[String]| {
            let mut text = format!("* {name}");
            if !requirement.none() {
                text.push_str(&format!(" ({requirement})"));
            }
            let platforms = platforms.join(", ");
            if !platforms.is_empty() && platforms != "ruby" {
                text.push_str(&format!(" {platforms}"));
            }
            text
        };
        // `new_deps` and `deleted_deps`, as `Gem::Dependency#==` compares
        // them: by name and requirement.
        let added: Vec<String> = dependencies
            .iter()
            .filter(|d| {
                !lock
                    .dependencies
                    .iter()
                    .any(|l| l.name == d.name && l.requirement == d.requirement)
            })
            .map(|d| pretty(&d.name, &d.requirement, &d.platforms))
            .collect();
        let deleted: Vec<String> = lock
            .dependencies
            .iter()
            .filter(|l| {
                !dependencies
                    .iter()
                    .any(|d| l.name == d.name && l.requirement == d.requirement)
            })
            .map(|l| pretty(&l.name, &l.requirement, &[]))
            .collect();
        // ponytail: `@new_platforms` and `@removed_platforms` are empty, as
        // nothing adds or removes a platform yet. Upgrade with `spinel lock
        // --add-platform`.
        if added.is_empty()
            && deleted.is_empty()
            && self.source_moves.is_empty()
            && !self.resolve_needed
        {
            return None;
        }
        // `change_reason`.
        let reason = if !self.resolve_needed {
            "some dependencies were deleted from your gemfile".to_owned()
        } else if self.unlocking {
            // `unlocking_reason`.
            if self.gems_to_unlock.is_empty() {
                "bundler is unlocking ruby".to_owned()
            } else {
                format!(
                    "bundler is unlocking gems: ({})",
                    self.gems_to_unlock.join(", ")
                )
            }
        } else {
            // `lockfile_changed_reason`.
            let r = &self.reasons;
            let quoted = |text: &str, name: &Option<String>| {
                name.as_ref()
                    .map(|n| text.replace("{}", &format!("\"{n}\"")))
            };
            [
                r.source_changes
                    .then(|| "the list of sources changed".to_owned()),
                (!self.changed_dependencies.is_empty())
                    .then(|| "the dependencies in your gemfile changed".to_owned()),
                self.current_platform_missing
                    .then(|| "your lockfile is missing the current platform".to_owned()),
                (!self.new_platforms.is_empty())
                    .then(|| "you are adding a new platform to your lockfile".to_owned()),
                self.path_changes
                    .then(|| "the gemspecs for path gems changed".to_owned()),
                quoted("your lockfile is missing {}", &r.missing_lockfile_dep),
                quoted(
                    "your lockfile is missing a CHECKSUMS entry for {}",
                    &r.missing_checksums,
                ),
                quoted(
                    "your lockfile has an empty CHECKSUMS entry for {}",
                    &r.empty_checksums,
                ),
                quoted(
                    "your lockfile includes {} but not some of its dependencies",
                    &r.missing_deps,
                ),
                quoted(
                    "your lockfile does not satisfy dependencies of {}",
                    &r.invalid_deps,
                ),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
        };
        let mut chars = reason.chars();
        let mut message: String = chars
            .next()
            .map(|c| c.to_ascii_uppercase())
            .into_iter()
            .collect();
        message.push_str(chars.as_str().trim());
        message.push_str(", but ");
        if !message.starts_with("Your lockfile") {
            message.push_str("the lockfile ");
        }
        message.push_str("can't be updated because frozen mode is set");
        if !added.is_empty() {
            message.push_str("\n\nYou have added to the Gemfile:\n");
            message.push_str(&added.join("\n"));
        }
        if !deleted.is_empty() {
            message.push_str("\n\nYou have deleted from the Gemfile:\n");
            message.push_str(&deleted.join("\n"));
        }
        if !self.source_moves.is_empty() {
            message.push_str("\n\nYou have changed in the Gemfile:\n");
            message.push_str(&self.source_moves.join("\n"));
        }
        if !self.unlocking {
            message.push_str(&format!(
                "\n\nRun `spinel install` elsewhere and add the updated {lockfile_name} to version control.\n"
            ));
        }
        Some(message)
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
        // With everything unlocked, `@locked_specs` is empty and so is the
        // base.
        let mut converged: Vec<Spec> = Vec::new();
        let mut deps: Vec<GemfileDependency> = Vec::new();
        for build in &self.locked {
            if self.unlocking_all || self.gems_to_unlock.contains(&build.spec.name) {
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
            unlock_all: self.unlocking_all,
            new_platforms,
            locked_specs: self.locked.iter().map(|b| b.spec.clone()).collect(),
        }
    }

    /// `additional_base_requirements_to_force_updates`: each gem an update
    /// names is pinned to the version a resolution of `request` with every
    /// gem unlocked and no base gives it (`new_resolver_for_full_update`),
    /// when that resolution has it.
    fn force_updates(
        &self,
        base: &mut Locked,
        request: &Request,
        index: &Index,
    ) -> Result<(), Failure> {
        if self.explicit_unlocks.is_empty() {
            return Ok(());
        }
        let unlocked = Locked {
            versions: base.versions.clone(),
            unlock_all: true,
            new_platforms: base.new_platforms.clone(),
            locked_specs: base.locked_specs.clone(),
            ..Locked::default()
        };
        let full_update = resolver::resolve_locked(request, index, &unlocked)?;
        for name in &self.explicit_unlocks {
            // `SpecSet#version_for`: the first spec's.
            if let Some(spec) = full_update.iter().find(|s| &s.name == name) {
                let pin = Requirement::new([format!("= {}", spec.version).as_str()])
                    .expect("a version makes one");
                base.requirements.insert(name.clone(), pin);
            }
        }
        Ok(())
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
        let mut base = self.resolution_base(&request.dependencies);
        self.force_updates(&mut base, &request, index)?;
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
        locals: &[LockSource],
    ) -> Lockfile {
        let bundler = Version::new(BUNDLER_VERSION).expect("a version");
        self.lockfile_for(remote, request, index, specs, locals, &bundler)
    }

    /// `Definition#write_lock` over `current`, the lockfile's text, when no
    /// update of Bundler was asked for: the lockfile [`Definition::lockfile`]
    /// writes, unless nothing unlocks (an update does, and the `ruby`
    /// directive unlocks the locked Ruby when it comes, goes or no longer
    /// allows it: `@unlocking ||= @unlocking_ruby`), no major version of Bundler is crossed and what the
    /// lockfile's own Bundler would write differs from `current` only in
    /// `RUBY VERSION`, `BUNDLED WITH`, sections that Bundler did not have
    /// yet or sections Bundler does not know (`lockfiles_equal?` with
    /// `preserve_unknown_sections`); then `current`, as it was.
    pub fn write_lock(
        &self,
        current: &str,
        remote: &str,
        request: &Request,
        index: Option<&Index>,
        specs: &[Spec],
        locals: &[LockSource],
    ) -> String {
        let proposed = self.lockfile(remote, request, index, specs, locals).write();
        if self.unlocking {
            return proposed;
        }
        let major = BUNDLER_VERSION.split('.').next().expect("a major version");
        let current_major = Version::new(major).expect("a version");
        let kept = match &self.locked_bundler {
            Some(locked) if locked.release() < current_major => return proposed,
            Some(locked) => self
                .lockfile_for(remote, request, index, specs, locals, locked)
                .write(),
            None => proposed.clone(),
        };
        if lockfiles_equal(current, &kept, self.locked_bundler.as_ref()) {
            current.to_owned()
        } else {
            proposed
        }
    }

    /// The lockfile `LockfileGenerator` of Bundler `bundler` writes.
    fn lockfile_for(
        &self,
        remote: &str,
        request: &Request,
        index: Option<&Index>,
        specs: &[Spec],
        locals: &[LockSource],
        bundler: &Version,
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
            let bundler = format!("bundler ({bundler})");
            lines.extend(locked.iter().filter(|c| c.lock_name == bundler).cloned());
            lines
        });
        // `Definition#locked_ruby_version`.
        let ruby_version =
            self.ruby_directive
                .then(|| match (&self.locked_ruby, self.unlocking_ruby) {
                    (Some(locked), false) => locked.clone(),
                    _ => format!("ruby {}", request.ruby_version),
                });
        let locked_specs = specs
            .iter()
            .map(|s| LockedSpec {
                name: s.name.clone(),
                version: s.version.clone(),
                platform: s.platform.clone(),
                dependencies: s.dependencies.clone(),
            })
            .collect();
        Lockfile {
            sources: lockfile::lock_sources(remote, locals, locked_specs),
            platforms: self.platforms.clone(),
            dependencies: lockfile::lock_dependencies(&request.dependencies, locals),
            checksums,
            ruby_version,
            bundled_with: Some(bundler.clone()),
        }
    }
}

/// `LockfileParser::SECTIONS_BY_VERSION_INTRODUCED`.
const SECTIONS_BY_VERSION_INTRODUCED: &[(&str, &[&str])] = &[
    ("1.0", &["DEPENDENCIES", "PLATFORMS", "GIT", "GEM", "PATH"]),
    ("1.10", &["BUNDLED WITH"]),
    ("1.12", &["RUBY VERSION"]),
    ("1.13", &["PLUGIN SOURCE"]),
    ("2.5.0", &["CHECKSUMS"]),
];

/// `Definition#lockfiles_equal?` with `preserve_unknown_sections`, when no
/// update of Bundler was asked for: `current` and `proposed` are equal
/// without `RUBY VERSION`, `BUNDLED WITH`, the sections introduced after
/// `locked_bundler` (`sections_to_ignore`) and those `current` has that
/// Bundler does not know (`unknown_sections_in_lockfile`).
fn lockfiles_equal(current: &str, proposed: &str, locked_bundler: Option<&Version>) -> bool {
    let base =
        locked_bundler.map_or_else(|| Version::new("1.0").expect("a version"), Version::release);
    let mut ignored: Vec<&str> = SECTIONS_BY_VERSION_INTRODUCED
        .iter()
        .filter(|(version, _)| Version::new(version).expect("a version") > base)
        .flat_map(|(_, sections)| sections.iter().copied())
        .collect();
    // `sections_in_lockfile`: lines of `/^\w[\w ]*$/`.
    for line in current.lines() {
        let section = line.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            && line
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ');
        let known = SECTIONS_BY_VERSION_INTRODUCED
            .iter()
            .any(|(_, sections)| sections.contains(&line));
        if section && !known && !ignored.contains(&line) {
            ignored.push(line);
        }
    }
    ignored.extend(["RUBY VERSION", "BUNDLED WITH"]);
    without_sections(current, &ignored) == without_sections(proposed, &ignored)
}

/// `text.gsub(/#{Regexp.union(sections)}\n(\s{2,}.*\n)+/, "\n")`, then
/// `gsub(/\n{2,}/, "\n\n").strip`: each of `sections` with its indented
/// lines taken out.
fn without_sections(text: &str, sections: &[&str]) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indented = |line: &str| {
        line.ends_with('\n') && line.chars().take(2).filter(|c| c.is_whitespace()).count() == 2
    };
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line
            .strip_suffix('\n')
            .is_some_and(|name| sections.contains(&name))
        {
            let end = i + 1 + lines[i + 1..].iter().take_while(|l| indented(l)).count();
            if end > i + 1 {
                out.push('\n');
                i = end;
                continue;
            }
        }
        out.push_str(line);
        i += 1;
    }
    let mut collapsed = String::with_capacity(out.len());
    let mut newlines = 0;
    for c in out.chars() {
        if c == '\n' {
            newlines += 1;
            if newlines <= 2 {
                collapsed.push(c);
            }
        } else {
            newlines = 0;
            collapsed.push(c);
        }
    }
    collapsed.trim().to_owned()
}

/// A lockfile spec as the resolver reads it.
pub(crate) fn locked_spec(spec: &LockedSpec) -> Spec {
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
