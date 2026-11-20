//! `version_range.rb`: one interval of versions, and [`RangeSet`], the value
//! pubgrub-ruby duck-types as "a range" (`VersionRange::Empty`, a
//! `VersionRange`, or a `VersionUnion`).
//!
//! A range keeps its open or closed bounds and the name it was made with
//! (`~> 1.0`); a name survives only where pubgrub-ruby hands the same object
//! back, and is lost wherever it builds a new range (an intersection, a span),
//! so `to_s` prints what pubgrub-ruby prints.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use super::Version;
use super::version_union::{self, VersionUnion};

/// `VersionRange`: the versions between `min` and `max`, each bound open or
/// closed, either one missing for no bound. Never empty: the empty range is
/// [`RangeSet::Empty`].
#[derive(Clone, Debug)]
pub struct VersionRange<V> {
    min: Option<V>,
    max: Option<V>,
    include_min: bool,
    include_max: bool,
    name: Option<String>,
}

impl<V: Version> VersionRange<V> {
    /// `VersionRange.new`. A missing bound cannot be closed.
    pub fn new(min: Option<V>, max: Option<V>, include_min: bool, include_max: bool) -> Self {
        assert!(
            min.is_some() || !include_min,
            "Ranges without a lower bound cannot have include_min == true"
        );
        assert!(
            max.is_some() || !include_max,
            "Ranges without an upper bound cannot have include_max == true"
        );
        VersionRange {
            min,
            max,
            include_min,
            include_max,
            name: None,
        }
    }

    /// `VersionRange.new(name:)`: printed as `name` while it is this object.
    pub fn named(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// `VersionRange.any`.
    pub fn any() -> Self {
        VersionRange::new(None, None, false, false)
    }

    /// `VersionRange.new(min: v, max: v, include_min: true, include_max: true)`.
    pub fn exact(version: V) -> Self {
        VersionRange::new(Some(version.clone()), Some(version), true, true)
    }

    pub fn min(&self) -> Option<&V> {
        self.min.as_ref()
    }

    pub fn max(&self) -> Option<&V> {
        self.max.as_ref()
    }

    pub fn include_min(&self) -> bool {
        self.include_min
    }

    pub fn include_max(&self) -> bool {
        self.include_max
    }

    /// `any?`: no bound at all.
    pub fn is_any(&self) -> bool {
        self.min.is_none() && self.max.is_none()
    }

    /// `include?`.
    pub fn includes(&self, version: &V) -> bool {
        self.compare_version(version) == Ordering::Equal
    }

    /// `partition_versions`: sorted `versions` split into those below, within
    /// and above the range.
    pub fn partition_versions<'a>(&self, versions: &'a [V]) -> (&'a [V], &'a [V], &'a [V]) {
        let min_index = match &self.min {
            Some(min) if !versions.is_empty() => {
                if self.include_min {
                    versions.partition_point(|v| v < min)
                } else {
                    versions.partition_point(|v| v <= min)
                }
            }
            _ => 0,
        };
        let (lower, versions) = versions.split_at(min_index);
        let max_index = match &self.max {
            Some(max) if !versions.is_empty() => {
                if self.include_max {
                    versions.partition_point(|v| v <= max)
                } else {
                    versions.partition_point(|v| v < max)
                }
            }
            _ => versions.len(),
        };
        let (within, higher) = versions.split_at(max_index);
        (lower, within, higher)
    }

    /// `select_versions`: the sorted `versions` in the range.
    pub fn select_versions(&self, versions: &[V]) -> Vec<V> {
        if self.is_any() {
            return versions.to_vec();
        }
        self.partition_versions(versions).1.to_vec()
    }

    /// `compare_version`: `Less` below the range, `Greater` above it.
    pub fn compare_version(&self, version: &V) -> Ordering {
        if let Some(min) = &self.min {
            match version.cmp(min) {
                Ordering::Less => return Ordering::Less,
                Ordering::Equal if !self.include_min => return Ordering::Less,
                _ => {}
            }
        }
        if let Some(max) = &self.max {
            match version.cmp(max) {
                Ordering::Equal if !self.include_max => return Ordering::Greater,
                Ordering::Greater => return Ordering::Greater,
                _ => {}
            }
        }
        Ordering::Equal
    }

    /// `strictly_lower?`: every version here is below every version of `other`.
    pub fn strictly_lower(&self, other: &VersionRange<V>) -> bool {
        let (Some(max), Some(other_min)) = (&self.max, &other.min) else {
            return false;
        };
        match max.cmp(other_min) {
            Ordering::Equal => !self.include_max || !other.include_min,
            Ordering::Less => true,
            Ordering::Greater => false,
        }
    }

    /// `strictly_higher?`.
    pub fn strictly_higher(&self, other: &VersionRange<V>) -> bool {
        other.strictly_lower(self)
    }

    /// `intersects?` with another (non-empty) range.
    pub fn intersects_range(&self, other: &VersionRange<V>) -> bool {
        !self.strictly_lower(other) && !self.strictly_higher(other)
    }

    /// `intersect` with another (non-empty) range: always a new range, or
    /// `EMPTY`.
    pub fn intersect_range(&self, other: &VersionRange<V>) -> RangeSet<V> {
        // Which of the two each bound comes from, for `min_range.equal?(max_range)`.
        let min_from_self = match (&self.min, &other.min) {
            (None, _) => false,
            (_, None) => true,
            (Some(min), Some(other_min)) => match min.cmp(other_min) {
                Ordering::Equal => !self.include_min,
                Ordering::Less => false,
                Ordering::Greater => true,
            },
        };
        let max_from_self = match (&self.max, &other.max) {
            (None, _) => false,
            (_, None) => true,
            (Some(max), Some(other_max)) => match max.cmp(other_max) {
                Ordering::Equal => !self.include_max,
                Ordering::Less => true,
                Ordering::Greater => false,
            },
        };
        let min_range = if min_from_self { self } else { other };
        let max_range = if max_from_self { self } else { other };

        if min_from_self != max_from_self {
            if let (Some(min), Some(max)) = (&min_range.min, &max_range.max) {
                match min.cmp(max) {
                    Ordering::Less => {}
                    Ordering::Equal => {
                        if !min_range.include_min || !max_range.include_max {
                            return RangeSet::Empty;
                        }
                    }
                    Ordering::Greater => return RangeSet::Empty,
                }
            }
        }

        RangeSet::Range(VersionRange::new(
            min_range.min.clone(),
            max_range.max.clone(),
            min_range.include_min,
            max_range.include_max,
        ))
    }

    /// `span`: the range covering both, which is their union when they are
    /// contiguous. Always a new range.
    pub fn span(&self, other: &VersionRange<V>) -> VersionRange<V> {
        let min_range = match (&self.min, &other.min) {
            (None, _) => self,
            (_, None) => other,
            (Some(min), Some(other_min)) => match min.cmp(other_min) {
                Ordering::Equal => {
                    if self.include_min {
                        self
                    } else {
                        other
                    }
                }
                Ordering::Less => self,
                Ordering::Greater => other,
            },
        };
        let max_range = match (&self.max, &other.max) {
            (None, _) => self,
            (_, None) => other,
            (Some(max), Some(other_max)) => match max.cmp(other_max) {
                Ordering::Equal => {
                    if self.include_max {
                        self
                    } else {
                        other
                    }
                }
                Ordering::Less => other,
                Ordering::Greater => self,
            },
        };
        VersionRange::new(
            min_range.min.clone(),
            max_range.max.clone(),
            min_range.include_min,
            max_range.include_max,
        )
    }

    /// `contiguous_to?` another (non-empty) range: overlapping or touching.
    pub fn contiguous_to(&self, other: &VersionRange<V>) -> bool {
        if self.is_any() {
            return true;
        }
        self.intersects_range(other) || self.contiguous_below(other) || self.contiguous_above(other)
    }

    /// `contiguous_below?`: this one ends where `other` starts, one of the
    /// two bounds closed.
    pub fn contiguous_below(&self, other: &VersionRange<V>) -> bool {
        let (Some(max), Some(other_min)) = (&self.max, &other.min) else {
            return false;
        };
        max == other_min && (self.include_max || other.include_min)
    }

    /// `contiguous_above?`.
    pub fn contiguous_above(&self, other: &VersionRange<V>) -> bool {
        other.contiguous_below(self)
    }

    /// `allows_all?` another (non-empty) range.
    pub fn allows_all_range(&self, other: &VersionRange<V>) -> bool {
        if self.max.is_some() && other.max.is_none() {
            return false;
        }
        if self.min.is_some() && other.min.is_none() {
            return false;
        }
        if let (Some(min), Some(other_min)) = (&self.min, &other.min) {
            match min.cmp(other_min) {
                Ordering::Less => {}
                Ordering::Equal => {
                    if !self.include_min && other.include_min {
                        return false;
                    }
                }
                Ordering::Greater => return false,
            }
        }
        if let (Some(max), Some(other_max)) = (&self.max, &other.max) {
            match max.cmp(other_max) {
                Ordering::Less => return false,
                Ordering::Equal => {
                    if !self.include_max && other.include_max {
                        return false;
                    }
                }
                Ordering::Greater => {}
            }
        }
        true
    }

    /// `upper_invert`: the versions above the range.
    pub fn upper_invert(&self) -> RangeSet<V> {
        match &self.max {
            None => RangeSet::Empty,
            Some(max) => RangeSet::Range(VersionRange::new(
                Some(max.clone()),
                None,
                !self.include_max,
                false,
            )),
        }
    }

    /// `invert`: the versions outside the range.
    pub fn invert(&self) -> RangeSet<V> {
        if self.is_any() {
            return RangeSet::Empty;
        }
        let low = || VersionRange::new(None, self.min.clone(), false, !self.include_min);
        let high = || VersionRange::new(self.max.clone(), None, !self.include_max, false);
        if self.min.is_none() {
            RangeSet::Range(high())
        } else if self.max.is_none() {
            RangeSet::Range(low())
        } else {
            RangeSet::Range(low()).union(&RangeSet::Range(high()))
        }
    }

    /// `constraints`, joined: `any`, `= v` when both bounds print alike,
    /// else each bound.
    fn constraints(&self) -> String {
        if self.is_any() {
            return "any".to_owned();
        }
        let printed = |v: &Option<V>| v.as_ref().map(ToString::to_string).unwrap_or_default();
        if printed(&self.min) == printed(&self.max) {
            return format!("= {}", printed(&self.min));
        }
        let mut c = Vec::new();
        if let Some(min) = &self.min {
            c.push(format!(
                "{} {min}",
                if self.include_min { ">=" } else { ">" }
            ));
        }
        if let Some(max) = &self.max {
            c.push(format!(
                "{} {max}",
                if self.include_max { "<=" } else { "<" }
            ));
        }
        c.join(", ")
    }
}

/// `==` and `eql?`: the bounds; the name is not compared.
impl<V: PartialEq> PartialEq for VersionRange<V> {
    fn eq(&self, other: &Self) -> bool {
        self.min == other.min
            && self.max == other.max
            && self.include_min == other.include_min
            && self.include_max == other.include_max
    }
}

impl<V: Eq> Eq for VersionRange<V> {}

impl<V: Hash> Hash for VersionRange<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.min.hash(state);
        self.max.hash(state);
        self.include_min.hash(state);
        self.include_max.hash(state);
    }
}

/// `to_s`: the name, else the constraints.
impl<V: Version> fmt::Display for VersionRange<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => f.write_str(name),
            None => f.write_str(&self.constraints()),
        }
    }
}

/// What pubgrub-ruby takes as "a range": `VersionRange::EMPTY`, a
/// `VersionRange`, or a `VersionUnion` of two or more disjoint ranges in
/// order. Each method dispatches as Ruby's would on the three classes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RangeSet<V> {
    Empty,
    Range(VersionRange<V>),
    Union(VersionUnion<V>),
}

impl<V: Version> RangeSet<V> {
    /// `VersionRange.any`.
    pub fn any() -> Self {
        RangeSet::Range(VersionRange::any())
    }

    /// `VersionRange.empty`.
    pub fn empty() -> Self {
        RangeSet::Empty
    }

    /// `ranges`: the disjoint ranges, in order; none for `EMPTY` (whose
    /// `[EMPTY]` every caller drops or skips).
    pub fn ranges(&self) -> &[VersionRange<V>] {
        match self {
            RangeSet::Empty => &[],
            RangeSet::Range(range) => std::slice::from_ref(range),
            RangeSet::Union(union) => union.ranges(),
        }
    }

    /// `empty?`.
    pub fn is_empty(&self) -> bool {
        matches!(self, RangeSet::Empty)
    }

    /// `any?`: every version.
    pub fn is_any(&self) -> bool {
        match self {
            RangeSet::Range(range) => range.is_any(),
            _ => false,
        }
    }

    /// `include?`.
    pub fn includes(&self, version: &V) -> bool {
        match self {
            RangeSet::Empty => false,
            RangeSet::Range(range) => range.includes(version),
            RangeSet::Union(union) => union.includes(version),
        }
    }

    /// `select_versions`: the sorted `versions` in the set.
    pub fn select_versions(&self, versions: &[V]) -> Vec<V> {
        match self {
            RangeSet::Empty => Vec::new(),
            RangeSet::Range(range) => range.select_versions(versions),
            RangeSet::Union(union) => union.select_versions(versions),
        }
    }

    /// `intersects?` (`allows_any?`).
    pub fn intersects(&self, other: &RangeSet<V>) -> bool {
        match (self, other) {
            (RangeSet::Empty, _) | (_, RangeSet::Empty) => false,
            (RangeSet::Range(_), RangeSet::Union(_)) => other.intersects(self),
            (RangeSet::Range(range), RangeSet::Range(other_range)) => {
                range.intersects_range(other_range)
            }
            (RangeSet::Union(union), _) => union.intersects(other.ranges()),
        }
    }

    /// `intersect`.
    pub fn intersect(&self, other: &RangeSet<V>) -> RangeSet<V> {
        match (self, other) {
            (RangeSet::Empty, _) => RangeSet::Empty,
            (RangeSet::Range(_), RangeSet::Empty) => RangeSet::Empty,
            (RangeSet::Range(_), RangeSet::Union(_)) => other.intersect(self),
            (RangeSet::Range(range), RangeSet::Range(other_range)) => {
                range.intersect_range(other_range)
            }
            (RangeSet::Union(union), _) => union.intersect(other.ranges()),
        }
    }

    /// `union`.
    pub fn union(&self, other: &RangeSet<V>) -> RangeSet<V> {
        match (self, other) {
            (_, RangeSet::Union(_)) if !matches!(self, RangeSet::Union(_)) => {
                version_union::union(&[other, self])
            }
            (RangeSet::Range(range), RangeSet::Range(other_range))
                if range.contiguous_to(other_range) =>
            {
                RangeSet::Range(range.span(other_range))
            }
            // pubgrub-ruby's `EMPTY.union(range)` raises (`EMPTY` has no
            // `max`); no derivation reaches it. The union is `range`.
            _ => version_union::union(&[self, other]),
        }
    }

    /// `allows_all?`: every version of `other` is here.
    pub fn allows_all(&self, other: &RangeSet<V>) -> bool {
        match (self, other) {
            (RangeSet::Empty, _) => other.is_empty(),
            (_, RangeSet::Empty) => true,
            (RangeSet::Range(range), RangeSet::Range(other_range)) => {
                range.allows_all_range(other_range)
            }
            _ => version_union::allows_all(self.ranges(), other.ranges()),
        }
    }

    /// `invert`.
    pub fn invert(&self) -> RangeSet<V> {
        match self {
            RangeSet::Empty => RangeSet::any(),
            RangeSet::Range(range) => range.invert(),
            RangeSet::Union(union) => union.invert(),
        }
    }

    /// `upper_invert`: the versions above the set. (`EMPTY.upper_invert`
    /// raises in pubgrub-ruby; nothing calls it.)
    pub fn upper_invert(&self) -> RangeSet<V> {
        match self {
            RangeSet::Empty => RangeSet::Empty,
            RangeSet::Range(range) => range.upper_invert(),
            RangeSet::Union(union) => union.upper_invert(),
        }
    }
}

/// `to_s`.
impl<V: Version> fmt::Display for RangeSet<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RangeSet::Empty => f.write_str("(no versions)"),
            RangeSet::Range(range) => range.fmt(f),
            RangeSet::Union(union) => union.fmt(f),
        }
    }
}
