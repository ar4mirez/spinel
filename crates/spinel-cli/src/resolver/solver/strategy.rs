//! `strategy.rb`: pubgrub-ruby's own decision order, over a source that
//! lists versions as `BasicPackageSource` does. Bundler replaces it with
//! `Bundler::Resolver::Strategy`; the solver's tests decide by this one.

use std::collections::HashMap;

use super::version_range::RangeSet;
use super::version_solver;
use super::{Package, Version};

/// The version listing a [`DefaultStrategy`] reads
/// (`BasicPackageSource#all_versions_for` and `#versions_for`).
pub trait VersionSource<P, V> {
    /// `all_versions_for`: every version, in preferred order of selection.
    fn all_versions_for(&mut self, package: &P) -> Vec<V>;

    /// `versions_for`: the versions in `range`, sorted.
    fn versions_for(&mut self, package: &P, range: &RangeSet<V>) -> Vec<V>;
}

/// `[matching_versions.count <= 1 ? 0 : 1, higher_versions.count]`.
type PriorityKey = (u8, usize);

/// `Strategy`: decide the package with at most one version left, else the
/// one with the fewest versions above its range; take its most preferred
/// version.
pub struct DefaultStrategy<P, V> {
    root_package: P,
    root_version: V,
    /// `@version_indexes`: each version's position in `all_versions_for`.
    version_indexes: HashMap<P, HashMap<V, usize>>,
}

impl<P: Package, V: Version> DefaultStrategy<P, V> {
    pub fn new(root_package: P, root_version: V) -> Self {
        DefaultStrategy {
            root_package,
            root_version,
            version_indexes: HashMap::new(),
        }
    }

    fn version_indexes<S: VersionSource<P, V>>(
        &mut self,
        source: &mut S,
        package: &P,
    ) -> &HashMap<V, usize> {
        if !self.version_indexes.contains_key(package) {
            let indexes = if *package == self.root_package {
                HashMap::from([(self.root_version.clone(), 0)])
            } else {
                let mut indexes = HashMap::new();
                for (i, version) in source.all_versions_for(package).into_iter().enumerate() {
                    // `each.with_index.to_h`: a repeated version keeps its
                    // last position.
                    indexes.insert(version, i);
                }
                indexes
            };
            self.version_indexes.insert(package.clone(), indexes);
        }
        &self.version_indexes[package]
    }

    /// `most_preferred_version_of`: the version in range listed first.
    fn most_preferred_version_of<S: VersionSource<P, V>>(
        &mut self,
        source: &mut S,
        package: &P,
        range: &RangeSet<V>,
    ) -> Option<V> {
        let versions = source.versions_for(package, range);
        let indexes = self.version_indexes(source, package);
        // `min_by`: the first of equals.
        let mut best: Option<(usize, &V)> = None;
        for version in &versions {
            let index = indexes[version];
            if best.is_none_or(|(least, _)| index < least) {
                best = Some((index, version));
            }
        }
        best.map(|(_, version)| version.clone())
    }

    /// `next_term_to_try_from`: the first package with the least
    /// `[matching <= 1 ? 0 : 1, higher.count]`.
    fn next_term_to_try_from<'a, S: VersionSource<P, V>>(
        &mut self,
        source: &mut S,
        unsatisfied: &'a [(P, RangeSet<V>)],
    ) -> &'a (P, RangeSet<V>) {
        // `min_by`: the first of equals.
        let mut best: Option<(PriorityKey, usize)> = None;
        for (i, (package, range)) in unsatisfied.iter().enumerate() {
            let matching_versions = source.versions_for(package, range);
            let higher_versions = source.versions_for(package, &range.upper_invert());
            let key = (
                if matching_versions.len() <= 1 { 0 } else { 1 },
                higher_versions.len(),
            );
            if best.is_none_or(|(least, _)| key < least) {
                best = Some((key, i));
            }
        }
        let (_, i) = best.expect("the solver asks only while packages are unsatisfied");
        &unsatisfied[i]
    }
}

impl<P, V, S> version_solver::Strategy<P, V, S> for DefaultStrategy<P, V>
where
    P: Package,
    V: Version,
    S: VersionSource<P, V>,
{
    fn next_package_and_version(
        &mut self,
        source: &mut S,
        unsatisfied: &[(P, RangeSet<V>)],
    ) -> (P, Option<V>) {
        let (package, range) = self.next_term_to_try_from(source, unsatisfied);
        let version = self.most_preferred_version_of(source, package, range);
        (package.clone(), version)
    }
}
