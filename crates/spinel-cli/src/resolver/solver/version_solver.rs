//! `version_solver.rb`: unit propagation, conflict resolution and decision
//! making over a [`Source`] of incompatibilities, choosing by a
//! [`Strategy`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::incompatibility::{Cause, Incompatibility};
use super::partial_solution::PartialSolution;
use super::term::Term;
use super::version_constraint::VersionConstraint;
use super::version_range::RangeSet;
use super::{Package, Relation, Version};

/// What the solver asks of a package source (`Bundler::Resolver`, or
/// pubgrub-ruby's `BasicPackageSource`).
pub trait Source<P, V> {
    /// `incompatibilities_for`: the facts about `package` at `version`,
    /// its dependencies.
    fn incompatibilities_for(&mut self, package: &P, version: &V)
    -> Vec<Rc<Incompatibility<P, V>>>;

    /// `no_versions_incompatibility_for`: the fact that no version satisfies
    /// `unsatisfied_term`.
    fn no_versions_incompatibility_for(
        &mut self,
        package: &P,
        unsatisfied_term: &Term<P, V>,
    ) -> Rc<Incompatibility<P, V>>;
}

/// What the solver asks of a strategy (`Bundler::Resolver::Strategy`, or
/// pubgrub-ruby's `Strategy`).
pub trait Strategy<P, V, S> {
    /// `next_package_and_version`: of the undecided packages and their
    /// ranges, in the partial solution's order, the package to decide next
    /// and its preferred version in its range, none if none is.
    fn next_package_and_version(
        &mut self,
        source: &mut S,
        unsatisfied: &[(P, RangeSet<V>)],
    ) -> (P, Option<V>);
}

/// A logger's level, as pubgrub-ruby logs each line at `debug` or `info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
}

/// `logger`: each line pubgrub-ruby logs, with its level.
pub type Logger = Box<dyn FnMut(Level, &str)>;

/// `SolveFailure`: the incompatibility version solving failed with, whose
/// derivation explains the failure.
#[derive(Debug, Clone)]
pub struct SolveFailure<P, V> {
    pub incompatibility: Rc<Incompatibility<P, V>>,
}

/// `propagate_incompatibility`'s result: nothing derived, a conflict, or the
/// package a term was derived for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Propagated<P> {
    Nothing,
    Conflict,
    Package(P),
}

/// `VersionSolver`.
pub struct VersionSolver<P, V, S, St> {
    source: S,
    strategy: St,
    logger: Option<Logger>,
    /// `@incompatibilities`: every incompatibility, under each of its terms'
    /// packages, in the order added.
    incompatibilities: HashMap<P, Vec<Rc<Incompatibility<P, V>>>>,
    /// `@seen_incompatibilities`: the source's, by `eql?`.
    seen_incompatibilities: HashSet<Rc<Incompatibility<P, V>>>,
    solution: PartialSolution<P, V>,
}

impl<P, V, S, St> VersionSolver<P, V, S, St>
where
    P: Package,
    V: Version,
    S: Source<P, V>,
    St: Strategy<P, V, S>,
{
    /// `VersionSolver.new(source:, root:, strategy:, logger:)`: the root
    /// incompatibility added and propagated.
    pub fn new(source: S, root: P, strategy: St, logger: Option<Logger>) -> Self {
        let mut solver = VersionSolver {
            source,
            strategy,
            logger,
            incompatibilities: HashMap::new(),
            seen_incompatibilities: HashSet::new(),
            solution: PartialSolution::new(),
        };
        solver.add_incompatibility(Rc::new(Incompatibility::new(
            vec![Term::new(VersionConstraint::any(root.clone()), false)],
            Cause::Root,
            None,
        )));
        // The root incompatibility's propagation derives `root`, with nothing
        // to conflict with.
        solver
            .propagate(root)
            .expect("the root incompatibility cannot conflict");
        solver
    }

    /// The logger back, for the next try's solver: Bundler logs every try
    /// of a resolution to one logger.
    pub fn into_logger(self) -> Option<Logger> {
        self.logger
    }

    fn log(&mut self, level: Level, message: impl FnOnce() -> String) {
        if let Some(logger) = &mut self.logger {
            logger(level, &message());
        }
    }

    /// `work`: one decision and its propagation; false once solved.
    pub fn work(&mut self) -> Result<bool, SolveFailure<P, V>> {
        let unsatisfied_terms = self.solution.unsatisfied();
        if unsatisfied_terms.is_empty() {
            let attempts = self.solution.attempted_solutions();
            self.log(Level::Info, || {
                format!("Solution found after {attempts} attempts:")
            });
            let decisions = self.solution.decisions().to_vec();
            for (package, version) in decisions {
                if package.is_root() {
                    continue;
                }
                self.log(Level::Info, || format!("* {package} {version}"));
            }
            return Ok(false);
        }

        let next_package = self.choose_package_version_from(&unsatisfied_terms);
        self.propagate(next_package)?;
        Ok(true)
    }

    /// `solve`: the decisions, the root's first, in the order made.
    pub fn solve(&mut self) -> Result<Vec<(P, V)>, SolveFailure<P, V>> {
        while self.work()? {}
        Ok(self.solution.decisions().to_vec())
    }

    /// `propagate`: unit propagation from `initial_package`, resolving any
    /// conflict it finds.
    fn propagate(&mut self, initial_package: P) -> Result<(), SolveFailure<P, V>> {
        let mut changed = vec![Propagated::Package(initial_package)];
        // `while package = changed.shift`: a `nil` pushed after a conflict
        // ends the loop as an empty list does.
        while !changed.is_empty() {
            let package = match changed.remove(0) {
                Propagated::Nothing => break,
                Propagated::Conflict => None,
                Propagated::Package(package) => Some(package),
            };
            // `reverse_each` over the list as it was: what conflict
            // resolution appends is not visited.
            let incompatibilities = package
                .and_then(|package| self.incompatibilities.get(&package).cloned())
                .unwrap_or_default();
            for incompatibility in incompatibilities.iter().rev() {
                match self.propagate_incompatibility(incompatibility) {
                    Propagated::Conflict => {
                        let root_cause = self.resolve_conflict(incompatibility.clone())?;
                        changed.clear();
                        changed.push(self.propagate_incompatibility(&root_cause));
                    }
                    Propagated::Nothing => {}
                    package @ Propagated::Package(_) => changed.push(package),
                }
            }
            // `uniq!`.
            let mut unique = Vec::with_capacity(changed.len());
            for entry in changed {
                if !unique.contains(&entry) {
                    unique.push(entry);
                }
            }
            changed = unique;
        }
        Ok(())
    }

    /// `propagate_incompatibility`: when every term but one is satisfied,
    /// the last one's inverse derived; a conflict when all are.
    fn propagate_incompatibility(
        &mut self,
        incompatibility: &Rc<Incompatibility<P, V>>,
    ) -> Propagated<P> {
        let mut unsatisfied: Option<&Term<P, V>> = None;
        for term in incompatibility.terms() {
            match self.solution.relation(term) {
                Relation::Disjoint => return Propagated::Nothing,
                Relation::Overlap => {
                    // More than one term inconclusive: nothing to deduce.
                    if unsatisfied.is_some() {
                        return Propagated::Nothing;
                    }
                    unsatisfied = Some(term);
                }
                Relation::Subset => {}
            }
        }
        let Some(unsatisfied) = unsatisfied else {
            return Propagated::Conflict;
        };

        let derived = unsatisfied.invert();
        self.log(Level::Debug, || format!("derived: {derived}"));
        let package = unsatisfied.package().clone();
        self.solution.derive(derived, incompatibility.clone());
        Propagated::Package(package)
    }

    /// `choose_package_version_from`: the strategy's next package and
    /// version, decided unless its dependencies conflict with the partial
    /// solution; a package with no version gets a `NoVersions` fact.
    fn choose_package_version_from(&mut self, unsatisfied_terms: &[Term<P, V>]) -> P {
        let remaining: Vec<(P, RangeSet<V>)> = unsatisfied_terms
            .iter()
            .map(|t| (t.package().clone(), t.constraint().range().clone()))
            .collect();

        let (package, version) = self
            .strategy
            .next_package_and_version(&mut self.source, &remaining);

        self.log(Level::Debug, || {
            let version = version
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("attempting {package} {version}")
        });

        let Some(version) = version else {
            let unsatisfied_term = unsatisfied_terms
                .iter()
                .find(|t| *t.package() == package)
                .expect("the strategy picks an unsatisfied package");
            let incompatibility = self
                .source
                .no_versions_incompatibility_for(&package, unsatisfied_term);
            self.add_incompatibility(incompatibility);
            return package;
        };

        let mut conflict = false;
        for incompatibility in self.source.incompatibilities_for(&package, &version) {
            if self.seen_incompatibilities.contains(&incompatibility) {
                self.log(Level::Debug, || format!("knew: {incompatibility}"));
                continue;
            }
            self.seen_incompatibilities.insert(incompatibility.clone());

            self.add_incompatibility(incompatibility.clone());

            conflict = conflict
                || incompatibility
                    .terms()
                    .iter()
                    .all(|term| *term.package() == package || self.solution.satisfies(term));
        }

        if !conflict {
            self.log(Level::Info, || format!("selected {package} {version}"));
            self.solution.decide(package.clone(), version);
        } else {
            self.log(Level::Info, || format!("conflict: {conflict}"));
        }

        package
    }

    /// `resolve_conflict`: from an incompatibility the partial solution
    /// satisfies, derive new ones by resolution until one can be backtracked
    /// to and propagated; a failure when the root itself is ruled out.
    fn resolve_conflict(
        &mut self,
        mut incompatibility: Rc<Incompatibility<P, V>>,
    ) -> Result<Rc<Incompatibility<P, V>>, SolveFailure<P, V>> {
        self.log(Level::Info, || format!("conflict: {incompatibility}"));

        let mut new_incompatibility: Option<Rc<Incompatibility<P, V>>> = None;

        while !incompatibility.is_failure() {
            let mut most_recent: Option<(usize, super::assignment::Assignment<P, V>)> = None;
            let mut difference: Option<Term<P, V>> = None;
            let mut previous_level = 1;

            for (i, term) in incompatibility.terms().iter().enumerate() {
                let satisfier = self.solution.satisfier(term).clone();

                // `most_recent_term == term` compares by identity: true only
                // when this term just became the most recent.
                let mut became_most_recent = false;
                match &most_recent {
                    None => {
                        most_recent = Some((i, satisfier));
                        became_most_recent = true;
                    }
                    Some((_, most_recent_satisfier))
                        if most_recent_satisfier.index < satisfier.index =>
                    {
                        previous_level = previous_level.max(most_recent_satisfier.decision_level);
                        most_recent = Some((i, satisfier));
                        difference = None;
                        became_most_recent = true;
                    }
                    Some(_) => {
                        previous_level = previous_level.max(satisfier.decision_level);
                    }
                }

                if became_most_recent {
                    let (_, most_recent_satisfier) = most_recent.as_ref().expect("just set");
                    let diff = most_recent_satisfier.term.difference(term);
                    if diff.is_empty() {
                        difference = None;
                    } else {
                        let difference_satisfier = self.solution.satisfier(&diff.invert());
                        previous_level = previous_level.max(difference_satisfier.decision_level);
                        difference = Some(diff);
                    }
                }
            }

            let (most_recent_index, most_recent_satisfier) =
                most_recent.expect("a conflict that is not a failure has terms");
            let most_recent_term = &incompatibility.terms()[most_recent_index];

            if previous_level < most_recent_satisfier.decision_level
                || most_recent_satisfier.is_decision()
            {
                self.log(Level::Info, || format!("backtracking to {previous_level}"));
                self.solution.backtrack(previous_level);

                if let Some(new_incompatibility) = new_incompatibility {
                    self.add_incompatibility(new_incompatibility);
                }

                return Ok(incompatibility);
            }

            let super::assignment::AssignmentCause::Derived(satisfier_cause) =
                &most_recent_satisfier.cause
            else {
                unreachable!("a decision backtracks above");
            };

            // `incompatibility.terms - [most_recent_term]`: `Array#-` drops
            // every `eql?` term, which, one term per package, is that one.
            let mut new_terms: Vec<Term<P, V>> = incompatibility
                .terms()
                .iter()
                .filter(|t| *t != most_recent_term)
                .cloned()
                .collect();
            new_terms.extend(
                satisfier_cause
                    .terms()
                    .iter()
                    .filter(|t| t.package() != most_recent_satisfier.term.package())
                    .cloned(),
            );
            if let Some(difference) = &difference {
                new_terms.push(difference.invert());
            }

            let derived = Rc::new(Incompatibility::new(
                new_terms,
                Cause::Conflict {
                    incompatibility: incompatibility.clone(),
                    satisfier: satisfier_cause.clone(),
                },
                None,
            ));

            if incompatibility.to_string() == derived.to_string() {
                self.log(Level::Info, || {
                    "!! failed to resolve conflicts, this shouldn't have happened".to_owned()
                });
                break;
            }

            let most_recent_term = most_recent_term.clone();
            new_incompatibility = Some(derived.clone());
            incompatibility = derived;

            let partially = if difference.is_some() {
                " partially"
            } else {
                ""
            };
            self.log(Level::Info, || {
                format!(
                    "! {most_recent_term} is{partially} satisfied by {}",
                    most_recent_satisfier.term
                )
            });
            self.log(Level::Info, || {
                format!("! which is caused by {satisfier_cause}")
            });
            self.log(Level::Info, || format!("! thus {incompatibility}"));
        }

        Err(SolveFailure { incompatibility })
    }

    /// `add_incompatibility`: listed under each of its terms' packages.
    fn add_incompatibility(&mut self, incompatibility: Rc<Incompatibility<P, V>>) {
        self.log(Level::Debug, || format!("fact: {incompatibility}"));
        for term in incompatibility.terms() {
            self.incompatibilities
                .entry(term.package().clone())
                .or_default()
                .push(incompatibility.clone());
        }
    }
}
