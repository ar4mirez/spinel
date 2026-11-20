//! `static_package_source.rb`, with the `BasicPackageSource` it inherits:
//! packages, versions and dependencies written out by hand, for the
//! solver's tests.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::gem_version::Version as GemVersion;

use super::rubygems::parse_range;
use super::{
    Cause, Incompatibility, Package, RangeSet, Source, Term, VersionConstraint, VersionRange,
    VersionSource,
};

/// A package of a static source: `Package::ROOT`, or a gem by its name
/// (pubgrub-ruby uses the name string itself).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StaticPackage {
    Root,
    Gem(String),
}

impl fmt::Display for StaticPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StaticPackage::Root => f.write_str("root"),
            StaticPackage::Gem(name) => f.write_str(name),
        }
    }
}

impl Package for StaticPackage {
    fn is_root(&self) -> bool {
        matches!(self, StaticPackage::Root)
    }
}

/// A dependency list: names and requirement strings, in order.
type Deps = Vec<(String, String)>;

/// `StaticPackageSource`.
#[derive(Default)]
pub struct StaticPackageSource {
    root_deps: Deps,
    /// `@packages`: each package's versions in the order added.
    packages: Vec<(String, Vec<(GemVersion, Deps)>)>,
    /// `@sorted_versions`.
    sorted_versions: HashMap<StaticPackage, Vec<GemVersion>>,
}

/// `Package.root_version`.
pub fn root_version() -> GemVersion {
    GemVersion::new("0").expect("0 is a version")
}

fn v(version: &str) -> GemVersion {
    GemVersion::new(version).expect("a version")
}

impl StaticPackageSource {
    pub fn new() -> Self {
        StaticPackageSource::default()
    }

    /// `DSL#root(deps:)`.
    pub fn root(mut self, deps: &[(&str, &str)]) -> Self {
        for (name, requirement) in deps {
            match self.root_deps.iter_mut().find(|(n, _)| n == name) {
                Some(dep) => dep.1 = (*requirement).to_owned(),
                None => self
                    .root_deps
                    .push(((*name).to_owned(), (*requirement).to_owned())),
            }
        }
        self
    }

    /// `DSL#add(name, version, deps:)`, less a dependency on itself that
    /// this version satisfies.
    pub fn add(mut self, name: &str, version: &str, deps: &[(&str, &str)]) -> Self {
        let version = v(version);
        let deps: Deps = deps
            .iter()
            .filter(|(dep_name, requirement)| {
                !(*dep_name == name && parse_range(&[requirement]).includes(&version))
            })
            .map(|(n, r)| ((*n).to_owned(), (*r).to_owned()))
            .collect();
        let at = match self.packages.iter().position(|(n, _)| n == name) {
            Some(at) => at,
            None => {
                self.packages.push((name.to_owned(), Vec::new()));
                self.packages.len() - 1
            }
        };
        let versions = &mut self.packages[at].1;
        assert!(
            !versions.iter().any(|(existing, _)| *existing == version),
            "{name} {version} declared twice"
        );
        versions.push((version, deps));
        self
    }

    fn versions_of(&self, name: &str) -> &[(GemVersion, Deps)] {
        let (_, versions) = self
            .packages
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no package {name}"));
        versions
    }

    /// `@cached_dependencies[package][version]`.
    fn dependencies_for(&self, package: &StaticPackage, version: &GemVersion) -> &Deps {
        match package {
            StaticPackage::Root => &self.root_deps,
            StaticPackage::Gem(name) => {
                let (_, deps) = self
                    .versions_of(name)
                    .iter()
                    .find(|(v, _)| v == version)
                    .expect("a version of the package");
                deps
            }
        }
    }

    /// `@sorted_versions[package]`.
    fn sorted_versions(&mut self, package: &StaticPackage) -> Vec<GemVersion> {
        if !self.sorted_versions.contains_key(package) {
            let versions = match package {
                StaticPackage::Root => vec![root_version()],
                StaticPackage::Gem(_) => {
                    let mut versions = self.all_versions_for(package);
                    versions.sort();
                    versions
                }
            };
            self.sorted_versions.insert(package.clone(), versions);
        }
        self.sorted_versions[package].clone()
    }

    /// `parse_dependency`: none for a package the source does not have.
    fn parse_dependency(&self, package: &str, dependency: &str) -> Option<RangeSet<GemVersion>> {
        if !self.packages.iter().any(|(n, _)| n == package) {
            return None;
        }
        Some(parse_range(&[dependency]))
    }
}

impl VersionSource<StaticPackage, GemVersion> for StaticPackageSource {
    fn all_versions_for(&mut self, package: &StaticPackage) -> Vec<GemVersion> {
        match package {
            StaticPackage::Root => vec![root_version()],
            StaticPackage::Gem(name) => self
                .versions_of(name)
                .iter()
                .map(|(v, _)| v.clone())
                .collect(),
        }
    }

    fn versions_for(
        &mut self,
        package: &StaticPackage,
        range: &RangeSet<GemVersion>,
    ) -> Vec<GemVersion> {
        range.select_versions(&self.sorted_versions(package))
    }
}

impl Source<StaticPackage, GemVersion> for StaticPackageSource {
    fn no_versions_incompatibility_for(
        &mut self,
        _package: &StaticPackage,
        unsatisfied_term: &Term<StaticPackage, GemVersion>,
    ) -> Rc<Incompatibility<StaticPackage, GemVersion>> {
        Rc::new(Incompatibility::new(
            vec![unsatisfied_term.clone()],
            Cause::NoVersions(unsatisfied_term.clone()),
            None,
        ))
    }

    /// `BasicPackageSource#incompatibilities_for`: each dependency over the
    /// run of versions around `version` that share it, open at either end
    /// that reaches the first or last version.
    fn incompatibilities_for(
        &mut self,
        package: &StaticPackage,
        version: &GemVersion,
    ) -> Vec<Rc<Incompatibility<StaticPackage, GemVersion>>> {
        let sorted_versions = self.sorted_versions(package);
        let deps = self.dependencies_for(package, version).clone();
        let mut incompatibilities = Vec::new();
        for (dep_package, dep_constraint_name) in deps {
            let shares = |v: &GemVersion| {
                self.dependencies_for(package, v)
                    .iter()
                    .any(|(n, r)| *n == dep_package && *r == dep_constraint_name)
            };
            let at = sorted_versions
                .iter()
                .position(|v| v == version)
                .expect("a version of the package");
            let (mut low, mut high) = (at, at);
            while low > 0 && shares(&sorted_versions[low - 1]) {
                low -= 1;
            }
            let low = (low != 0).then(|| sorted_versions[low].clone());
            while high < sorted_versions.len() && shares(&sorted_versions[high]) {
                high += 1;
            }
            let high = sorted_versions.get(high).cloned();

            let include_min = low.is_some();
            let range = RangeSet::Range(VersionRange::new(low, high, include_min, false));
            let self_constraint = VersionConstraint::new(package.clone(), range);

            let Some(dep_range) = self.parse_dependency(&dep_package, &dep_constraint_name) else {
                // An unknown package: this version is invalid.
                return vec![Rc::new(Incompatibility::new(
                    vec![Term::new(self_constraint, true)],
                    Cause::InvalidDependency {
                        package: StaticPackage::Gem(dep_package),
                        constraint: dep_constraint_name,
                    },
                    None,
                ))];
            };
            let dep_constraint = VersionConstraint::new(StaticPackage::Gem(dep_package), dep_range);

            incompatibilities.push(Rc::new(Incompatibility::new(
                vec![
                    Term::new(self_constraint, true),
                    Term::new(dep_constraint, false),
                ],
                Cause::Dependency,
                None,
            )));
        }
        incompatibilities
    }
}
