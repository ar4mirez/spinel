//! `term.rb`: a constraint, asserted (positive) or denied (negative).

use std::fmt;

use super::version_constraint::VersionConstraint;
use super::{Package, Relation, Version};

/// `Term`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Term<P, V> {
    constraint: VersionConstraint<P, V>,
    positive: bool,
}

impl<P: Package, V: Version> Term<P, V> {
    pub fn new(constraint: VersionConstraint<P, V>, positive: bool) -> Self {
        Term {
            constraint,
            positive,
        }
    }

    pub fn package(&self) -> &P {
        self.constraint.package()
    }

    pub fn constraint(&self) -> &VersionConstraint<P, V> {
        &self.constraint
    }

    /// `positive?`.
    pub fn is_positive(&self) -> bool {
        self.positive
    }

    /// `negative?`.
    pub fn is_negative(&self) -> bool {
        !self.positive
    }

    /// `to_s(allow_every:)`.
    pub fn to_s(&self, allow_every: bool) -> String {
        if self.positive {
            self.constraint.to_s(allow_every)
        } else {
            format!("not {}", self.constraint)
        }
    }

    /// `invert` (`inverse`).
    pub fn invert(&self) -> Self {
        Term::new(self.constraint.clone(), !self.positive)
    }

    /// `intersect`: both terms' versions of the package at once.
    pub fn intersect(&self, other: &Self) -> Self {
        assert!(self.package() == other.package(), "packages must match");
        match (self.positive, other.positive) {
            (true, true) => Term::new(self.constraint.intersect(&other.constraint), true),
            (false, false) => Term::new(self.constraint.union(&other.constraint), false),
            _ => {
                let (positive, negative) = if self.positive {
                    (self, other)
                } else {
                    (other, self)
                };
                Term::new(
                    positive.constraint.intersect(&negative.constraint.invert()),
                    true,
                )
            }
        }
    }

    /// `difference`.
    pub fn difference(&self, other: &Self) -> Self {
        self.intersect(&other.invert())
    }

    /// `relation`: how this term's versions relate to `other`'s.
    pub fn relation(&self, other: &Self) -> Relation {
        match (self.positive, other.positive) {
            (true, true) => self.constraint.relation(&other.constraint),
            (false, true) => {
                if self.constraint.allows_all(&other.constraint) {
                    Relation::Disjoint
                } else {
                    Relation::Overlap
                }
            }
            (true, false) => {
                if !other.constraint.allows_any(&self.constraint) {
                    Relation::Subset
                } else if other.constraint.allows_all(&self.constraint) {
                    Relation::Disjoint
                } else {
                    Relation::Overlap
                }
            }
            (false, false) => {
                if self.constraint.allows_all(&other.constraint) {
                    Relation::Subset
                } else {
                    Relation::Overlap
                }
            }
        }
    }

    /// `normalized_constraint`: the versions the term allows.
    pub fn normalized_constraint(&self) -> VersionConstraint<P, V> {
        if self.positive {
            self.constraint.clone()
        } else {
            self.constraint.invert()
        }
    }

    /// `satisfies?`: this term implies `other`.
    pub fn satisfies(&self, other: &Self) -> bool {
        assert!(self.package() == other.package(), "packages must match");
        self.relation(other) == Relation::Subset
    }

    /// `empty?`: the term allows no version.
    pub fn is_empty(&self) -> bool {
        self.normalized_constraint().is_empty()
    }
}

/// `to_s`.
impl<P: Package, V: Version> fmt::Display for Term<P, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_s(false))
    }
}
