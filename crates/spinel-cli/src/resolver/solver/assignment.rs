//! `assignment.rb`: a term in the partial solution, and why it is there.

use std::rc::Rc;

use super::incompatibility::Incompatibility;
use super::term::Term;
use super::version_constraint::VersionConstraint;
use super::{Package, Version};

/// `Assignment#cause`: `:decision`, or the incompatibility the term was
/// derived from.
#[derive(Clone, Debug)]
pub enum AssignmentCause<P, V> {
    Decision,
    Derived(Rc<Incompatibility<P, V>>),
}

/// `Assignment`: the term, its cause, the decision level it was made at,
/// and its position in the partial solution.
#[derive(Clone, Debug)]
pub struct Assignment<P, V> {
    pub term: Term<P, V>,
    pub cause: AssignmentCause<P, V>,
    pub decision_level: usize,
    pub index: usize,
}

impl<P: Package, V: Version> Assignment<P, V> {
    /// `Assignment.decision`: the package at exactly `version`.
    pub fn decision(package: P, version: V, decision_level: usize, index: usize) -> Self {
        Assignment {
            term: Term::new(VersionConstraint::exact(package, version), true),
            cause: AssignmentCause::Decision,
            decision_level,
            index,
        }
    }

    /// `decision?`.
    pub fn is_decision(&self) -> bool {
        matches!(self.cause, AssignmentCause::Decision)
    }
}
