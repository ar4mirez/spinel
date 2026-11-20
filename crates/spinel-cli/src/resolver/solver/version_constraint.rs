//! `version_constraint.rb`: a package and a set of its versions.

use std::fmt;

use super::version_range::{RangeSet, VersionRange};
use super::{Package, Relation, Version};

/// `VersionConstraint`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VersionConstraint<P, V> {
    package: P,
    range: RangeSet<V>,
}

impl<P: Package, V: Version> VersionConstraint<P, V> {
    pub fn new(package: P, range: RangeSet<V>) -> Self {
        VersionConstraint { package, range }
    }

    /// `VersionConstraint.exact`.
    pub fn exact(package: P, version: V) -> Self {
        VersionConstraint::new(package, RangeSet::Range(VersionRange::exact(version)))
    }

    /// `VersionConstraint.any`.
    pub fn any(package: P) -> Self {
        VersionConstraint::new(package, RangeSet::any())
    }

    pub fn package(&self) -> &P {
        &self.package
    }

    pub fn range(&self) -> &RangeSet<V> {
        &self.range
    }

    fn same_package(&self, other: &Self) {
        assert!(
            self.package == other.package,
            "Can only intersect between VersionConstraint of the same package"
        );
    }

    /// `intersect`.
    pub fn intersect(&self, other: &Self) -> Self {
        self.same_package(other);
        VersionConstraint::new(self.package.clone(), self.range.intersect(&other.range))
    }

    /// `union`.
    pub fn union(&self, other: &Self) -> Self {
        self.same_package(other);
        VersionConstraint::new(self.package.clone(), self.range.union(&other.range))
    }

    /// `invert`.
    pub fn invert(&self) -> Self {
        VersionConstraint::new(self.package.clone(), self.range.invert())
    }

    /// `allows_all?`.
    pub fn allows_all(&self, other: &Self) -> bool {
        self.range.allows_all(&other.range)
    }

    /// `allows_any?`.
    pub fn allows_any(&self, other: &Self) -> bool {
        self.range.intersects(&other.range)
    }

    /// `subset?`.
    pub fn is_subset(&self, other: &Self) -> bool {
        other.allows_all(self)
    }

    /// `overlap?`.
    pub fn overlaps(&self, other: &Self) -> bool {
        other.allows_any(self)
    }

    /// `relation`.
    pub fn relation(&self, other: &Self) -> Relation {
        if self.is_subset(other) {
            Relation::Subset
        } else if self.overlaps(other) {
            Relation::Overlap
        } else {
            Relation::Disjoint
        }
    }

    /// `to_s(allow_every:)`: the root by its name, `every version of foo`
    /// when allowed, else `foo` and the constraint.
    pub fn to_s(&self, allow_every: bool) -> String {
        if self.package.is_root() {
            self.package.to_string()
        } else if allow_every && self.is_any() {
            format!("every version of {}", self.package)
        } else {
            format!("{} {}", self.package, self.constraint_string())
        }
    }

    /// `constraint_string`: `>= 0` for every version, else the range.
    pub fn constraint_string(&self) -> String {
        if self.is_any() {
            ">= 0".to_owned()
        } else {
            self.range.to_string()
        }
    }

    /// `empty?`.
    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    /// `any?`: every version of the package.
    pub fn is_any(&self) -> bool {
        self.range.is_any()
    }
}

/// `to_s`.
impl<P: Package, V: Version> fmt::Display for VersionConstraint<P, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_s(false))
    }
}
