//! `incompatibility.rb`: terms that cannot all hold, and why.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use super::term::Term;
use super::{Package, Version};

/// What an incompatibility came from: `incompatibility.cause`.
///
/// Derived incompatibilities share their causes by reference (`Rc`), as
/// pubgrub-ruby's share objects, so a cause reached twice in a derivation
/// is the same one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Cause<P, V> {
    /// `:root`: the root must be selected.
    Root,
    /// `:dependency`: the first term's versions depend on the second's
    /// inverse.
    Dependency,
    /// `InvalidDependency`: a dependency on a package the source does not
    /// know, with the dependency as the source wrote it. pubgrub-ruby's
    /// `BasicPackageSource` states it, as the tests' static source does;
    /// Bundler's source never does (`Resolver#incompatibilities_for`
    /// replaces that method), but `FailureWriter` still writes it.
    #[cfg_attr(not(test), allow(dead_code))]
    InvalidDependency { package: P, constraint: String },
    /// `NoVersions`: no version satisfies the term.
    NoVersions(Term<P, V>),
    /// `ConflictCause`: derived during conflict resolution from
    /// `incompatibility` and the cause of the assignment that satisfied it
    /// (`satisfier`).
    Conflict {
        incompatibility: Rc<Incompatibility<P, V>>,
        satisfier: Rc<Incompatibility<P, V>>,
    },
}

/// `Incompatibility`, with the `extended_explanation` Bundler's subclass
/// (`Bundler::Resolver::Incompatibility`) adds.
#[derive(Clone, Debug)]
pub struct Incompatibility<P, V> {
    terms: Vec<Term<P, V>>,
    cause: Cause<P, V>,
    custom_explanation: Option<String>,
    extended_explanation: Option<String>,
}

impl<P: Package, V: Version> Incompatibility<P, V> {
    /// `Incompatibility.new(terms, cause:, custom_explanation:)`, its terms
    /// cleaned up. A dependency has exactly two terms.
    pub fn new(
        terms: Vec<Term<P, V>>,
        cause: Cause<P, V>,
        custom_explanation: Option<String>,
    ) -> Self {
        let terms = cleanup_terms(terms, &cause);
        assert!(
            !matches!(cause, Cause::Dependency) || terms.len() == 2,
            "a dependency Incompatibility must have exactly two terms. Got {terms:?}"
        );
        Incompatibility {
            terms,
            cause,
            custom_explanation,
            extended_explanation: None,
        }
    }

    /// `Bundler::Resolver::Incompatibility.new(..., extended_explanation:)`:
    /// what Bundler prints after the explanation when the failure's walk
    /// (`find_names_to_relax`) meets this incompatibility.
    pub fn with_extended_explanation(mut self, extended_explanation: Option<String>) -> Self {
        self.extended_explanation = extended_explanation;
        self
    }

    pub fn terms(&self) -> &[Term<P, V>] {
        &self.terms
    }

    pub fn cause(&self) -> &Cause<P, V> {
        &self.cause
    }

    pub fn extended_explanation(&self) -> Option<&str> {
        self.extended_explanation.as_deref()
    }

    /// `failure?`: no terms, or only the root.
    pub fn is_failure(&self) -> bool {
        self.terms.is_empty()
            || (self.terms.len() == 1
                && self.terms[0].package().is_root()
                && self.terms[0].is_positive())
    }

    /// `conflict?`: derived in conflict resolution.
    pub fn is_conflict(&self) -> bool {
        matches!(self.cause, Cause::Conflict { .. })
    }

    // `external_incompatibilities` is not ported: pubgrub-ruby's raises
    // (`[this]`) on any incompatibility that is not a conflict, and nothing
    // in Bundler calls it.
}

/// `cleanup_terms`: a derived incompatibility drops a positive root term
/// (unless it is the only term); terms on one package are intersected into
/// one, in the order each package first appears.
fn cleanup_terms<P: Package, V: Version>(
    mut terms: Vec<Term<P, V>>,
    cause: &Cause<P, V>,
) -> Vec<Term<P, V>> {
    if terms.len() != 1 && matches!(cause, Cause::Conflict { .. }) {
        terms.retain(|term| !(term.is_positive() && term.package().is_root()));
    }
    if terms.len() <= 1 {
        return terms;
    }
    if terms.len() == 2 && terms[0].package() != terms[1].package() {
        return terms;
    }
    // `terms.group_by(&:package).map { inject(:intersect) }`.
    let mut grouped: Vec<Term<P, V>> = Vec::new();
    for term in terms {
        match grouped.iter_mut().find(|t| t.package() == term.package()) {
            Some(acc) => *acc = acc.intersect(&term),
            None => grouped.push(term),
        }
    }
    grouped
}

/// `hash` and `eql?`: the cause and the terms.
impl<P: PartialEq, V: PartialEq> PartialEq for Incompatibility<P, V> {
    fn eq(&self, other: &Self) -> bool {
        self.cause == other.cause && self.terms == other.terms
    }
}

impl<P: Eq, V: Eq> Eq for Incompatibility<P, V> {}

impl<P: Hash, V: Hash> Hash for Incompatibility<P, V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cause.hash(state);
        self.terms.hash(state);
    }
}

/// `to_s`: the custom explanation, else a sentence by cause and terms.
impl<P: Package, V: Version> fmt::Display for Incompatibility<P, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(explanation) = &self.custom_explanation {
            return f.write_str(explanation);
        }
        let terms = &self.terms;
        let joined = |terms: &[Term<P, V>], sep: &str| {
            terms
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(sep)
        };
        match &self.cause {
            Cause::Root => f.write_str("(root dependency)"),
            Cause::Dependency => write!(
                f,
                "{} depends on {}",
                terms[0].to_s(true),
                terms[1].invert()
            ),
            Cause::InvalidDependency { package, .. } => write!(
                f,
                "{} depends on unknown package {package}",
                terms[0].to_s(true)
            ),
            Cause::NoVersions(constraint) => write!(f, "no versions satisfy {constraint}"),
            Cause::Conflict { .. } => {
                if self.is_failure() {
                    f.write_str("version solving has failed")
                } else if terms.len() == 1 {
                    let term = &terms[0];
                    if term.is_positive() {
                        if term.constraint().is_any() {
                            write!(f, "{} cannot be used", term.package())
                        } else {
                            write!(f, "{} cannot be used", term.to_s(true))
                        }
                    } else {
                        write!(f, "{} is required", term.invert())
                    }
                } else if terms.iter().all(Term::is_positive) {
                    if terms.len() == 2 {
                        write!(
                            f,
                            "{} is incompatible with {}",
                            terms[0].to_s(true),
                            terms[1]
                        )
                    } else {
                        write!(f, "one of {} must be false", joined(terms, " or "))
                    }
                } else if terms.iter().all(Term::is_negative) {
                    if terms.len() == 2 {
                        write!(f, "either {} or {}", terms[0].invert(), terms[1].invert())
                    } else {
                        let inverted: Vec<Term<P, V>> = terms.iter().map(Term::invert).collect();
                        write!(f, "one of {} must be true", joined(&inverted, " or "))
                    }
                } else {
                    let positive: Vec<Term<P, V>> =
                        terms.iter().filter(|t| t.is_positive()).cloned().collect();
                    let negative: Vec<Term<P, V>> = terms
                        .iter()
                        .filter(|t| t.is_negative())
                        .map(Term::invert)
                        .collect();
                    if positive.len() == 1 {
                        write!(
                            f,
                            "{} requires {}",
                            positive[0].to_s(true),
                            joined(&negative, " or ")
                        )
                    } else {
                        write!(
                            f,
                            "if {} then {}",
                            joined(&positive, " and "),
                            joined(&negative, " or ")
                        )
                    }
                }
            }
        }
    }
}
