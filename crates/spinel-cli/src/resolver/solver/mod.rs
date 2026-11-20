//! The solver Bundler resolves with: a port of pubgrub-ruby 0.5.0, as
//! vendored in Bundler 4.0.20 (`bundler/vendor/pub_grub`), file for file
//! (#816). It is a port, not a use of the `pubgrub` crate, so that the
//! incompatibilities a resolution derives, and so what Bundler relaxes and
//! how it explains a conflict, are pubgrub-ruby's by construction.
//!
//! - `version_range.rs`, `version_union.rs`: sets of versions, with the
//!   open and closed bounds and the `~>` names pubgrub-ruby prints.
//! - `version_constraint.rs`, `term.rs`: a package's set, asserted or denied.
//! - `incompatibility.rs`: terms that cannot all hold, and their cause.
//! - `assignment.rs`, `partial_solution.rs`: the decisions and derivations
//!   so far.
//! - `version_solver.rs`: unit propagation, conflict resolution, decision
//!   making, over a [`Source`] and a [`Strategy`].
//! - `strategy.rs`: pubgrub-ruby's own `Strategy`.
//! - `rubygems.rs`: `Gem::Requirement` as a range, over plain versions as
//!   pubgrub-ruby's `RubyGems` makes it and over [`super::Candidate`] as
//!   `Bundler::Resolver#requirement_to_range` does.
//!
//! Equality and hashing are by value throughout, where Ruby hashes versions
//! by `eql?` (`1.0` and `1` differ) and compares them by `==`; a resolution
//! never meets two spellings of one version of a package, so no derivation
//! tells the two apart.
//!
//! Not yet used by resolution: the transition plan in #816 wires it in
//! behind a hidden switch, then makes it the default.

mod assignment;
mod incompatibility;
mod partial_solution;
mod rubygems;
mod strategy;
mod term;
mod version_constraint;
mod version_range;
mod version_solver;
mod version_union;

#[cfg(test)]
mod static_package_source;
#[cfg(test)]
mod tests;

use std::fmt;
use std::hash::Hash;

pub use assignment::{Assignment, AssignmentCause};
pub use incompatibility::{Cause, Incompatibility};
pub use partial_solution::PartialSolution;
pub use rubygems::candidate_range;
pub use strategy::{DefaultStrategy, VersionSource};
pub use term::Term;
pub use version_constraint::VersionConstraint;
pub use version_range::{RangeSet, VersionRange};
pub use version_solver::{Level, Logger, SolveFailure, Source, Strategy, VersionSolver};
pub use version_union::VersionUnion;

/// A package as pubgrub-ruby takes one: compared, hashed, printed, and the
/// root or not (`Package.root?`).
pub trait Package: Clone + Eq + Hash + fmt::Display + fmt::Debug {
    fn is_root(&self) -> bool;
}

/// A version as pubgrub-ruby takes one: ordered, hashed, printed.
pub trait Version: Clone + Ord + Hash + fmt::Display + fmt::Debug {}

impl<T: Clone + Ord + Hash + fmt::Display + fmt::Debug> Version for T {}

/// `relation`: how one term's versions relate to another's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    /// `:subset`: all of them are the other's.
    Subset,
    /// `:disjoint`: none of them are.
    Disjoint,
    /// `:overlap`: some are.
    Overlap,
}
