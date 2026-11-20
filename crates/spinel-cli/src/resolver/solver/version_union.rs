//! `version_union.rb`: two or more disjoint ranges, in order.

use std::cmp::Ordering;
use std::fmt;

use super::Version;
use super::version_range::{RangeSet, VersionRange};

/// `VersionUnion`: disjoint, non-contiguous ranges, lowest first.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VersionUnion<V> {
    ranges: Vec<VersionRange<V>>,
}

/// `VersionUnion.normalize_ranges`: every range of `sets`, the empty ones
/// dropped, those with no lower bound first, then by lower bound (a closed
/// one first), contiguous neighbours merged by `span`.
pub fn normalize_ranges<V: Version>(sets: &[&RangeSet<V>]) -> Vec<VersionRange<V>> {
    let ranges: Vec<&VersionRange<V>> = sets.iter().flat_map(|set| set.ranges()).collect();
    if ranges.is_empty() {
        return Vec::new();
    }
    let (mut original, mut bounded): (Vec<&VersionRange<V>>, Vec<&VersionRange<V>>) =
        ranges.into_iter().partition(|r| r.min().is_none());
    // `sort_by { [r.min, r.include_min ? 0 : 1] }`. Ruby's sort is not
    // stable, but ranges that tie start at the same bound, so they overlap
    // and merge below whichever comes first.
    bounded.sort_by(|a, b| {
        a.min()
            .cmp(&b.min())
            .then_with(|| b.include_min().cmp(&a.include_min()))
    });
    original.extend(bounded);

    let mut out: Vec<VersionRange<V>> = vec![original[0].clone()];
    for range in &original[1..] {
        let last = out.last_mut().expect("starts with one range");
        if last.contiguous_to(range) {
            *last = last.span(range);
        } else {
            out.push((*range).clone());
        }
    }
    out
}

/// `VersionUnion.union(ranges)`: normalized, then `EMPTY`, the one range, or
/// a union.
pub fn union<V: Version>(sets: &[&RangeSet<V>]) -> RangeSet<V> {
    from_ranges(normalize_ranges(sets))
}

/// `VersionUnion.union(ranges, normalize: false)`.
fn from_ranges<V: Version>(mut ranges: Vec<VersionRange<V>>) -> RangeSet<V> {
    match ranges.len() {
        0 => RangeSet::Empty,
        1 => RangeSet::Range(ranges.remove(0)),
        _ => RangeSet::Union(VersionUnion { ranges }),
    }
}

/// `VersionUnion#allows_all?`, over `mine` (a union's ranges, or one range's
/// own as `VersionUnion.new([self])` makes them): each of `others` inside one
/// of `mine`, walking both in order.
pub fn allows_all<V: Version>(mine: &[VersionRange<V>], others: &[VersionRange<V>]) -> bool {
    let mut i = 0;
    others.iter().all(|other| {
        while i < mine.len() {
            if mine[i].allows_all_range(other) {
                break;
            }
            i += 1;
        }
        i < mine.len()
    })
}

/// Which of the two walks advances in `intersects?` and `intersect`: the one
/// whose range ends first (`other_range.max < my_range.max`), the other when
/// mine is unbounded above.
fn advance_other<V: Version>(mine: &VersionRange<V>, other: &VersionRange<V>) -> bool {
    match (mine.max(), other.max()) {
        (None, _) => true,
        (Some(my_max), Some(other_max)) => other_max.cmp(my_max) == Ordering::Less,
        (Some(_), None) => false,
    }
}

impl<V: Version> VersionUnion<V> {
    pub fn ranges(&self) -> &[VersionRange<V>] {
        &self.ranges
    }

    /// `include?`.
    pub fn includes(&self, version: &V) -> bool {
        self.ranges.iter().any(|r| r.includes(version))
    }

    /// `select_versions`: each range's from what lies above the last.
    pub fn select_versions(&self, all_versions: &[V]) -> Vec<V> {
        let mut versions = Vec::new();
        let mut rest = all_versions;
        for range in &self.ranges {
            let (_, matching, higher) = range.partition_versions(rest);
            versions.extend_from_slice(matching);
            rest = higher;
        }
        versions
    }

    /// `intersects?` with `other`'s ranges.
    pub fn intersects(&self, other: &[VersionRange<V>]) -> bool {
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.len() {
            let (mine, theirs) = (&self.ranges[i], &other[j]);
            if mine.intersects_range(theirs) {
                return true;
            }
            if advance_other(mine, theirs) {
                j += 1;
            } else {
                i += 1;
            }
        }
        false
    }

    /// `intersect` with `other`'s ranges: each pair's intersection along the
    /// same walk, not normalized again.
    pub fn intersect(&self, other: &[VersionRange<V>]) -> RangeSet<V> {
        let mut new_ranges = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.len() {
            let (mine, theirs) = (&self.ranges[i], &other[j]);
            if let RangeSet::Range(range) = mine.intersect_range(theirs) {
                new_ranges.push(range);
            }
            if advance_other(mine, theirs) {
                j += 1;
            } else {
                i += 1;
            }
        }
        from_ranges(new_ranges)
    }

    /// `upper_invert`: the last range's.
    pub fn upper_invert(&self) -> RangeSet<V> {
        self.ranges
            .last()
            .expect("a union has ranges")
            .upper_invert()
    }

    /// `invert`: each range's inverse, intersected.
    pub fn invert(&self) -> RangeSet<V> {
        let mut inverses = self.ranges.iter().map(VersionRange::invert);
        let first = inverses.next().expect("a union has ranges");
        inverses.fold(first, |acc, inverse| acc.intersect(&inverse))
    }
}

/// `to_s`: neighbours that meet at a version (`< 1.0` and `> 1.0`) print as
/// one range with `!= 1.0`; the rest are joined by ` OR `.
impl<V: Version> fmt::Display for VersionUnion<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let printed = |v: Option<&V>| v.map(ToString::to_string).unwrap_or_default();
        let mut output = Vec::new();
        let mut i = 0;
        while i < self.ranges.len() {
            let mut ne = Vec::new();
            let mut range = self.ranges[i].clone();
            i += 1;
            while i < self.ranges.len() && printed(self.ranges[i].min()) == printed(range.max()) {
                ne.push(format!("!= {}", printed(range.max())));
                range = range.span(&self.ranges[i]);
                i += 1;
            }
            if ne.is_empty() {
                output.push(range.to_string());
            } else if range.is_any() {
                output.push(ne.join(", "));
            } else {
                output.push(format!("{range}, {}", ne.join(", ")));
            }
        }
        f.write_str(&output.join(" OR "))
    }
}
