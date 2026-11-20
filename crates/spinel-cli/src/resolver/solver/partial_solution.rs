//! `partial_solution.rb`: the assignments made so far, decisions and
//! derivations, by decision level.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::assignment::{Assignment, AssignmentCause};
use super::incompatibility::Incompatibility;
use super::term::Term;
use super::{Package, Relation, Version};

/// `PartialSolution`.
#[derive(Debug)]
pub struct PartialSolution<P, V> {
    assignments: Vec<Assignment<P, V>>,
    /// `@assignments_by`: each package's assignments, as indexes into
    /// `assignments`, oldest first.
    assignments_by: HashMap<P, Vec<usize>>,
    /// `@cumulative_assignments`: the package's term once each assignment
    /// was added, by the assignment's position in `assignments`.
    cumulative: Vec<Term<P, V>>,
    /// `@decisions`, a Hash: decided versions in the order decided.
    decisions: Vec<(P, V)>,
    decided: HashSet<P>,
    /// `@terms`: each package's assignments intersected.
    terms: HashMap<P, Term<P, V>>,
    relation_cache: HashMap<P, HashMap<Term<P, V>, Relation>>,
    /// `@required`, a Hash: packages with a positive assignment, in the
    /// order they first got one.
    required: Vec<P>,
    required_set: HashSet<P>,
    attempted_solutions: usize,
    backtracking: bool,
}

impl<P: Package, V: Version> Default for PartialSolution<P, V> {
    fn default() -> Self {
        PartialSolution::new()
    }
}

impl<P: Package, V: Version> PartialSolution<P, V> {
    pub fn new() -> Self {
        PartialSolution {
            assignments: Vec::new(),
            assignments_by: HashMap::new(),
            cumulative: Vec::new(),
            decisions: Vec::new(),
            decided: HashSet::new(),
            terms: HashMap::new(),
            relation_cache: HashMap::new(),
            required: Vec::new(),
            required_set: HashSet::new(),
            attempted_solutions: 1,
            backtracking: false,
        }
    }

    pub fn assignments(&self) -> &[Assignment<P, V>] {
        &self.assignments
    }

    /// The decisions, in the order made.
    pub fn decisions(&self) -> &[(P, V)] {
        &self.decisions
    }

    pub fn attempted_solutions(&self) -> usize {
        self.attempted_solutions
    }

    /// `decision_level`: how many decisions there are.
    pub fn decision_level(&self) -> usize {
        self.decisions.len()
    }

    /// `relation`: how the package's assignments so far relate to `term`;
    /// `Overlap` when there are none.
    pub fn relation(&mut self, term: &Term<P, V>) -> Relation {
        let package = term.package();
        let Some(current) = self.terms.get(package) else {
            return Relation::Overlap;
        };
        let cache = self.relation_cache.entry(package.clone()).or_default();
        *cache
            .entry(term.clone())
            .or_insert_with(|| current.relation(term))
    }

    /// `satisfies?`.
    pub fn satisfies(&mut self, term: &Term<P, V>) -> bool {
        self.relation(term) == Relation::Subset
    }

    /// `derive`: `term`, caused by `cause`, at the current level.
    pub fn derive(&mut self, term: Term<P, V>, cause: Rc<Incompatibility<P, V>>) {
        let assignment = Assignment {
            term,
            cause: AssignmentCause::Derived(cause),
            decision_level: self.decision_level(),
            index: self.assignments.len(),
        };
        self.add_assignment(assignment);
    }

    /// `satisfier`: the earliest assignment after which the package's
    /// assignments satisfy `term`.
    pub fn satisfier(&self, term: &Term<P, V>) -> &Assignment<P, V> {
        let by = self
            .assignments_by
            .get(term.package())
            .map(Vec::as_slice)
            .unwrap_or_default();
        // `bsearch` in find-minimum mode.
        let at = by.partition_point(|&i| !self.cumulative[i].satisfies(term));
        match by.get(at) {
            Some(&i) => &self.assignments[i],
            None => panic!("{term} unsatisfied"),
        }
    }

    /// `unsatisfied`: the term of each required package not yet decided.
    pub fn unsatisfied(&self) -> Vec<Term<P, V>> {
        self.required
            .iter()
            .filter(|package| !self.decided.contains(*package))
            .map(|package| self.terms[package].clone())
            .collect()
    }

    /// `decide`: the package at `version`, at a new decision level.
    pub fn decide(&mut self, package: P, version: V) {
        if self.backtracking {
            self.attempted_solutions += 1;
        }
        self.backtracking = false;

        self.decisions.push((package.clone(), version.clone()));
        self.decided.insert(package.clone());
        let assignment = Assignment::decision(
            package,
            version,
            self.decision_level(),
            self.assignments.len(),
        );
        self.add_assignment(assignment);
    }

    /// `backtrack`: everything after decision level `previous_level` undone.
    pub fn backtrack(&mut self, previous_level: usize) {
        self.backtracking = true;

        let new_assignments: Vec<Assignment<P, V>> = std::mem::take(&mut self.assignments)
            .into_iter()
            .filter(|assignment| assignment.decision_level <= previous_level)
            .collect();
        let mut new_decisions = std::mem::take(&mut self.decisions);
        new_decisions.truncate(previous_level);

        self.reset();

        self.decided = new_decisions.iter().map(|(p, _)| p.clone()).collect();
        self.decisions = new_decisions;

        for assignment in new_assignments {
            self.add_assignment(assignment);
        }
    }

    /// `reset!`.
    fn reset(&mut self) {
        self.assignments.clear();
        self.assignments_by.clear();
        self.cumulative.clear();
        self.decisions.clear();
        self.decided.clear();
        self.terms.clear();
        self.relation_cache.clear();
        self.required.clear();
        self.required_set.clear();
    }

    /// `add_assignment`.
    fn add_assignment(&mut self, assignment: Assignment<P, V>) {
        let package = assignment.term.package().clone();

        if assignment.term.is_positive() && self.required_set.insert(package.clone()) {
            self.required.push(package.clone());
        }

        let term = match self.terms.get(&package) {
            Some(old_term) => old_term.intersect(&assignment.term),
            None => assignment.term.clone(),
        };
        self.terms.insert(package.clone(), term.clone());
        if let Some(cache) = self.relation_cache.get_mut(&package) {
            cache.clear();
        }

        self.assignments_by
            .entry(package)
            .or_default()
            .push(self.assignments.len());
        self.assignments.push(assignment);
        self.cumulative.push(term);
    }
}
