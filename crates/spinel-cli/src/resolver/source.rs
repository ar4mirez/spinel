//! Bundler's source side of pubgrub-ruby (#816): `Bundler::Resolver` as the
//! solver's package source (`incompatibilities_for`,
//! `no_versions_incompatibility_for`, `parse_dependency`, `versions_for`)
//! and `Bundler::Resolver::Strategy` with `GemVersionPromoter`'s order, as
//! Bundler 4.0.20 has them, over the candidates [`Provider`] makes.
//!
//! The candidate model is [`Provider`]'s, shared with the `pubgrub` crate's
//! path: what a package's versions are, which builds each stands for, and
//! their dependencies. What differs is how they reach the solver. Here a
//! dependency is stated over the whole run of versions that share it, open
//! at the first and last version, as Bundler states it, so the solver
//! derives what pubgrub-ruby derives.

use std::collections::HashMap;
use std::rc::Rc;

use crate::gem_requirement::Requirement;
use crate::lockfile::BUNDLER_VERSION;

use super::solver::{
    Cause, Incompatibility, RangeSet, Source, Strategy, Term, VersionConstraint, VersionRange,
    candidate_range,
};
use super::{
    Candidate, Package, PackageName, Provider, Spec, dependency_hash, other_specs_matching_message,
};

type Constraint = VersionConstraint<Package, Candidate>;

/// A version's dependencies: `to_dependency_hash`, each package once, in the
/// order first depended on.
type Dependencies = Rc<Vec<(Package, Constraint)>>;

/// `Bundler::Resolver`, as the source pubgrub-ruby's `VersionSolver` reads.
pub(super) struct BundlerSource<'p, 'a> {
    provider: &'p Provider<'a>,
    /// `@sorted_versions`: each package's candidates, ascending.
    sorted_versions: HashMap<Package, Rc<[Candidate]>>,
    /// `@cached_dependencies`: each package's versions' dependencies.
    cached_dependencies: HashMap<Package, HashMap<Candidate, Dependencies>>,
}

impl<'p, 'a> BundlerSource<'p, 'a> {
    /// The source over `provider`, whose root dependencies are prepared
    /// (`setup_solver`).
    pub(super) fn new(provider: &'p Provider<'a>) -> Self {
        BundlerSource {
            provider,
            sorted_versions: HashMap::new(),
            cached_dependencies: HashMap::new(),
        }
    }

    /// `@sorted_versions[package]`: the root's one version, or the
    /// package's candidates.
    fn sorted_versions(&mut self, package: &Package) -> Rc<[Candidate]> {
        if let Some(versions) = self.sorted_versions.get(package) {
            return versions.clone();
        }
        let versions: Rc<[Candidate]> = self.provider.sorted_candidates(package).into();
        self.sorted_versions
            .insert(package.clone(), versions.clone());
        versions
    }

    /// `versions_for(package, range)`: the package's candidates in `range`.
    pub(super) fn versions_for(
        &mut self,
        package: &Package,
        range: &RangeSet<Candidate>,
    ) -> Vec<Candidate> {
        range.select_versions(&self.sorted_versions(package))
    }

    /// `sort_versions_by_preferred`: `GemVersionPromoter#sort_versions`.
    fn sort_versions_by_preferred(&self, package: &Package, versions: &mut Vec<Candidate>) {
        if let Package::Name(name) = package {
            self.provider.sort_preferred(name, versions);
        }
    }

    /// `@cached_dependencies[package][version]`: the root's prepared
    /// dependencies (`prepare_dependencies`), or the version's own, less any
    /// on the package itself.
    fn dependencies(&mut self, package: &Package, version: &Candidate) -> Dependencies {
        if let Some(deps) = self
            .cached_dependencies
            .get(package)
            .and_then(|versions| versions.get(version))
        {
            return deps.clone();
        }
        let deps = match package {
            Package::Root(_) => self
                .provider
                .root_constraints
                .iter()
                .map(|(dep, range)| (dep.clone(), Constraint::new(dep.clone(), range.clone())))
                .collect(),
            Package::Name(name) => dependency_hash(
                self.provider
                    .candidate_requirements(name, version)
                    .into_iter()
                    .map(|(dep, requirement)| parse_dependency(Package::Name(dep), &requirement)),
                Constraint::intersect,
            ),
        };
        let deps = Rc::new(deps);
        self.cached_dependencies
            .entry(package.clone())
            .or_default()
            .insert(version.clone(), deps.clone());
        deps
    }

    /// `package_deps[version][dep_package]`.
    fn dependency_on(
        &mut self,
        package: &Package,
        version: &Candidate,
        dep_package: &Package,
    ) -> Option<Constraint> {
        self.dependencies(package, version)
            .iter()
            .find(|(p, _)| p == dep_package)
            .map(|(_, constraint)| constraint.clone())
    }

    /// `repository_for(package)`, as `to_s` names it: `Source::Metadata`
    /// for the meta packages, the Gemfile's source for a gem.
    fn repository_for(&self, package: &Package) -> String {
        if package.is_meta() {
            "the local ruby installation".to_owned()
        } else {
            self.provider.request.source.clone()
        }
    }
}

/// `parse_dependency`: the dependency's requirement as a range of
/// candidates (`requirement_to_range`). A `Source::Gemspec` dependency is
/// any version; none reaches this resolver, whose callers reject `gemspec`.
fn parse_dependency(package: Package, requirement: &Requirement) -> (Package, Constraint) {
    let range = candidate_range(requirement);
    (package.clone(), Constraint::new(package, range))
}

impl Source<Package, Candidate> for BundlerSource<'_, '_> {
    /// `incompatibilities_for`: each dependency of `version`, stated for the
    /// run of the package's versions around it that have the same one, open
    /// below when the run reaches the first version and above when it
    /// reaches the last. The root's dependencies on `Ruby\0` and
    /// `RubyGems\0` say which Ruby and RubyGems are current.
    fn incompatibilities_for(
        &mut self,
        package: &Package,
        version: &Candidate,
    ) -> Vec<Rc<Incompatibility<Package, Candidate>>> {
        let sorted_versions = self.sorted_versions(package);
        let at = sorted_versions
            .iter()
            .position(|v| v == version)
            .expect("the solver decides a version the source listed");
        let deps = self.dependencies(package, version);
        let mut incompatibilities = Vec::with_capacity(deps.len());
        for (dep_package, dep_constraint) in deps.iter() {
            let (mut low, mut high) = (at, at);

            // Find the version low such that all >= low share the dependency.
            while low > 0
                && self
                    .dependency_on(package, &sorted_versions[low - 1], dep_package)
                    .as_ref()
                    == Some(dep_constraint)
            {
                low -= 1;
            }
            let low = (low != 0).then(|| sorted_versions[low].clone());

            // Find the version high such that all < high share it.
            while high < sorted_versions.len()
                && self
                    .dependency_on(package, &sorted_versions[high], dep_package)
                    .as_ref()
                    == Some(dep_constraint)
            {
                high += 1;
            }
            let high = sorted_versions.get(high).cloned();

            let include_min = low.is_some();
            let range = RangeSet::Range(VersionRange::new(low, high, include_min, false));
            let self_constraint = Constraint::new(package.clone(), range);

            let custom_explanation = (dep_package.is_meta() && matches!(package, Package::Root(_)))
                .then(|| {
                    format!(
                        "current {dep_package} version is {}",
                        dep_constraint.constraint_string()
                    )
                });

            incompatibilities.push(Rc::new(Incompatibility::new(
                vec![
                    Term::new(self_constraint, true),
                    Term::new(dep_constraint.clone(), false),
                ],
                Cause::Dependency,
                custom_explanation,
            )));
        }
        incompatibilities
    }

    /// `no_versions_incompatibility_for`: no version of the package
    /// satisfies `unsatisfied_term`, explained as Bundler explains it, with
    /// the builds for other platforms that satisfy it listed as its extended
    /// explanation.
    // ponytail: Bundler's cooldown hint has no counterpart: Spinel has no
    // cooldown yet.
    fn no_versions_incompatibility_for(
        &mut self,
        package: &Package,
        unsatisfied_term: &Term<Package, Candidate>,
    ) -> Rc<Incompatibility<Package, Candidate>> {
        let cause = Cause::NoVersions(unsatisfied_term.clone());
        let constraint = unsatisfied_term.constraint();
        let (custom_explanation, extended_explanation) = match package {
            // `bundler_pinned_to_current_version?`: `Source::Metadata`'s
            // `bundler` is the only one, as with no `bundle update --bundler`.
            // `bundler_not_found_message`: the default source's `bundler` is
            // that same one, which the constraint rules out.
            Package::Name(PackageName::Gem(name)) if name == "bundler" => (
                format!(
                    "the current Bundler version ({BUNDLER_VERSION}) does not satisfy {constraint}"
                ),
                Some(
                    "Your bundle requires a different version of Bundler than the one you're \
                     running, and that version could not be found.\n"
                        .to_owned(),
                ),
            ),
            _ => {
                let specs = self.specs_matching_other_platforms(package, constraint);
                let platforms = if specs.is_empty() {
                    String::new()
                } else {
                    let platforms: Vec<String> = self
                        .provider
                        .package_platforms(package)
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    format!(" for any resolution platforms ({})", platforms.join(", "))
                };
                let custom = format!(
                    "{constraint} could not be found in {}{platforms}",
                    self.repository_for(package)
                );
                let label = format!("{package} ({})", constraint.constraint_string());
                let specs: Vec<&Spec> = specs.iter().collect();
                let extended =
                    (!specs.is_empty()).then(|| other_specs_matching_message(&specs, &label));
                (custom, extended)
            }
        };
        Rc::new(
            Incompatibility::new(
                vec![unsatisfied_term.clone()],
                cause,
                Some(custom_explanation),
            )
            .with_extended_explanation(extended_explanation),
        )
    }
}

impl BundlerSource<'_, '_> {
    /// `filter_matching_specs(@all_specs[name], requirements)`: the gem's
    /// builds, for any platform, that satisfy the constraint as Bundler
    /// reads it back from its string, each `OR` alternative a
    /// `Gem::Requirement` of its `,`-separated parts, those of each in turn.
    fn specs_matching_other_platforms(
        &self,
        package: &Package,
        constraint: &Constraint,
    ) -> Vec<Spec> {
        let Package::Name(PackageName::Gem(name)) = package else {
            return Vec::new();
        };
        let all = self.provider.all_specs(name);
        constraint
            .constraint_string()
            .split(" OR ")
            .filter_map(|req| Requirement::new(req.split(',').map(str::trim)).ok())
            .flat_map(|requirement| {
                all.iter()
                    .filter(|spec| requirement.satisfied_by(&spec.version))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// What `find_names_to_relax` found: the gems to unlock and those to let
/// prereleases in for, each once in the order first met, and the last
/// extended explanation met.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct NamesToRelax {
    pub(super) unlock: Vec<String>,
    pub(super) prereleases: Vec<PackageName>,
    pub(super) extended_explanation: Option<String>,
}

impl NamesToRelax {
    /// As `relax_hook.rb` logs Bundler's: `{"unlock":[...],"pre":[...]}`,
    /// names as Bundler spells them.
    pub(super) fn to_json(&self) -> String {
        let pre: Vec<String> = self
            .prereleases
            .iter()
            .map(PackageName::bundler_name)
            .collect();
        format!(
            "{{\"unlock\":{},\"pre\":{}}}",
            serde_json::Value::from(self.unlock.clone()),
            serde_json::Value::from(pre)
        )
    }
}

/// `Resolver#find_names_to_relax`: from the failure, while it is a
/// conflict's, step to the incompatibility being resolved
/// (`cause.incompatibility`, never the satisfier's) and look at each of its
/// terms: a gem with a base requirement is to be unlocked, and a package
/// whose prereleases are left out but that has some is to let them in. A
/// `NoVersions` incompatibility among the conflict's two causes gives its
/// extended explanation. Bundler's third list, gems to look for remotely,
/// has no counterpart: Spinel has no local-only resolution.
pub(super) fn find_names_to_relax(
    provider: &Provider<'_>,
    incompatibility: &Rc<Incompatibility<Package, Candidate>>,
) -> NamesToRelax {
    let mut names = NamesToRelax::default();
    let mut incompatibility = incompatibility;
    while let Cause::Conflict {
        incompatibility: cause_incompatibility,
        satisfier,
    } = incompatibility.cause()
    {
        incompatibility = cause_incompatibility;
        for term in incompatibility.terms() {
            if let Package::Name(name) = term.package() {
                if let Some(gem) = provider.base_requirement_name(name) {
                    if !names.unlock.contains(&gem) {
                        names.unlock.push(gem);
                    }
                } else if provider.ignores_prereleases(name)
                    && provider.has_prereleases(name)
                    && !names.prereleases.contains(name)
                {
                    names.prereleases.push(name.clone());
                }
            }
            let no_versions = [incompatibility, satisfier]
                .into_iter()
                .find(|incompat| matches!(incompat.cause(), Cause::NoVersions(_)));
            if let Some(no_versions) = no_versions {
                names.extended_explanation = no_versions.extended_explanation().map(str::to_owned);
            }
        }
    }
    names
}

/// `[matching_versions.count <= 1 ? 0 : 1, higher_versions.count]`.
type PriorityKey = (u8, usize);

/// `Bundler::Resolver::Strategy`.
#[derive(Default)]
pub(super) struct BundlerStrategy {
    /// `@package_priority_cache`.
    package_priority_cache: HashMap<(Package, RangeSet<Candidate>), PriorityKey>,
}

impl BundlerStrategy {
    /// `next_term_to_try_from`: of the unsatisfied packages, the first with
    /// the least `[matching <= 1 ? 0 : 1, higher.count]`: one with at most
    /// one version left, else the one with the fewest versions above its
    /// range.
    fn next_term_to_try_from<'u>(
        &mut self,
        source: &mut BundlerSource<'_, '_>,
        unsatisfied: &'u [(Package, RangeSet<Candidate>)],
    ) -> &'u (Package, RangeSet<Candidate>) {
        // `min_by`: the first of equals.
        let mut best: Option<(PriorityKey, usize)> = None;
        for (i, (package, range)) in unsatisfied.iter().enumerate() {
            let key = (package.clone(), range.clone());
            let priority = match self.package_priority_cache.get(&key) {
                Some(priority) => *priority,
                None => {
                    let matching_versions = source.versions_for(package, range);
                    let higher_versions = source.versions_for(package, &range.upper_invert());
                    let priority = (u8::from(matching_versions.len() > 1), higher_versions.len());
                    self.package_priority_cache.insert(key, priority);
                    priority
                }
            };
            if best.is_none_or(|(least, _)| priority < least) {
                best = Some((priority, i));
            }
        }
        let (_, i) = best.expect("the solver asks only while packages are unsatisfied");
        &unsatisfied[i]
    }

    /// `most_preferred_version_of`: the versions in range, in
    /// `GemVersionPromoter`'s order when there is more than one.
    fn most_preferred_version_of(
        source: &mut BundlerSource<'_, '_>,
        package: &Package,
        range: &RangeSet<Candidate>,
    ) -> Vec<Candidate> {
        let mut versions = source.versions_for(package, range);
        // The conditional avoids, among other things, sorting the root's.
        if versions.len() > 1 {
            source.sort_versions_by_preferred(package, &mut versions);
        }
        versions
    }
}

impl<'p, 'a> Strategy<Package, Candidate, BundlerSource<'p, 'a>> for BundlerStrategy {
    /// `next_package_and_version`.
    fn next_package_and_version(
        &mut self,
        source: &mut BundlerSource<'p, 'a>,
        unsatisfied: &[(Package, RangeSet<Candidate>)],
    ) -> (Package, Option<Candidate>) {
        let (package, range) = self.next_term_to_try_from(source, unsatisfied);
        let version = BundlerStrategy::most_preferred_version_of(source, package, range)
            .into_iter()
            .next();
        (package.clone(), version)
    }
}
