//! The default solver's explanation of a failed resolution, written as
//! Bundler writes it: pubgrub-ruby's `FailureWriter` (a port of Dart pub's),
//! ported over the `pubgrub` crate's derivation tree, and the `to_s` of
//! pubgrub-ruby's incompatibilities, terms and ranges, with Bundler's own
//! wording where it overrides them (`current Ruby version is ...`, `... could
//! not be found in ...`).
//!
//! pubgrub-rs's tree holds the same facts in other shapes, which are put
//! back into Bundler's before writing:
//!
//! - pubgrub-rs states a dependency for the versions it tried, where Bundler
//!   states it for every version that shares it. Each dependency is widened
//!   back to Bundler's range (`Resolver#incompatibilities_for`: outward while
//!   the dependency is the same, open at the first and last version), and a
//!   term that rules versions out is written as the runs of candidates it
//!   covers, the same way. What pubgrub-rs never tried it cannot state, so a
//!   range can still start at the first release where Bundler's starts at a
//!   prerelease with the same dependency.
//! - pubgrub-rs, having ruled out each version of a gem it tried, rules out
//!   the rest of the range with a "no versions" fact about the gaps between
//!   them, which Bundler's ranged dependencies never need. Such a fact (on a
//!   range with no candidates, beside a cause that rules the gem out) is
//!   folded into that cause, as pubgrub-rs's own `collapse_no_versions`
//!   folds them; a version genuinely not found, a dependency's range with
//!   nothing in it, is kept.
//! - Ranges carry no names in pubgrub-rs. pubgrub-ruby writes a `~>`
//!   requirement's range as `~> x` until it is intersected or joined with
//!   another; so is a dependency's range here, and a derived term that is
//!   one dependency's range passed through.
//! - A derived incompatibility's terms have no order in pubgrub-rs; they are
//!   written root first, then by name.
//!
//! `Solver::PubgrubRuby` derives as pubgrub-ruby derives, so its failures
//! need none of this: `resolver/report.rs` writes them as they are.
// ponytail: the reshaping (`collapse_gaps`, `passed_through`, the widening
// of `dependency_span` and `displayed`) exists only to undo pubgrub-rs's
// shapes, and some of pubgrub-rs's failures still read otherwise than
// Bundler's; slice 5 of #816 makes the port the only solver and deletes this
// file with the `pubgrub` crate.

use std::collections::HashMap;
use std::ops::Bound;
use std::sync::Arc;

use pubgrub::{DerivationTree, Derived, External, Ranges, Term};

use super::{Candidate, Package, PackageName, Provider, VersionSet, pessimistic_end};

type Tree = DerivationTree<Package, VersionSet, String>;

/// The identity of an incompatibility in the tree: its shared id when it
/// appears more than once, else where it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Shared(usize),
    At(usize),
}

fn key(tree: &Tree) -> Key {
    match tree {
        DerivationTree::Derived(d) => d
            .shared_id
            .map_or(Key::At(std::ptr::from_ref(d) as usize), Key::Shared),
        DerivationTree::External(e) => Key::At(std::ptr::from_ref(e) as usize),
    }
}

fn is_conflict(tree: &Tree) -> bool {
    matches!(tree, DerivationTree::Derived(_))
}

fn causes(tree: &Tree) -> Option<(&Tree, &Tree)> {
    match tree {
        DerivationTree::Derived(d) => Some((&d.cause1, &d.cause2)),
        DerivationTree::External(_) => None,
    }
}

struct Writer<'p, 'a> {
    provider: &'p Provider<'a>,
    root: Key,
    derivations: HashMap<Key, usize>,
    lines: Vec<(String, Option<usize>)>,
    line_numbers: HashMap<Key, usize>,
}

/// `SolveFailure#explanation`, and after it Bundler's extended explanation
/// of a version not found, when there is one.
pub(super) fn explain(tree: &Tree, provider: &Provider<'_>) -> String {
    let tree = &collapse_gaps(tree, provider);
    let mut writer = Writer {
        provider,
        root: key(tree),
        derivations: HashMap::new(),
        lines: Vec::new(),
        line_numbers: HashMap::new(),
    };
    writer.count_derivations(tree);
    let mut explanation = writer.write(tree);
    if let Some(extended) = writer.extended_explanation(tree) {
        explanation.push_str("\n\n");
        explanation.push_str(&extended);
    }
    explanation
}

impl Writer<'_, '_> {
    /// `FailureWriter#write`.
    fn write(&mut self, root: &Tree) -> String {
        if !is_conflict(root) {
            return self.incompatibility(root);
        }
        self.visit(root, false);
        let padding = self
            .line_numbers
            .values()
            .max()
            .map_or(0, |last| format!("({last}) ").len());
        self.lines
            .iter()
            .map(|(message, number)| {
                if message.is_empty() {
                    return String::new();
                }
                let lead = number.map_or(String::new(), |n| format!("({n}) "));
                let lead = format!("{lead:<padding$}");
                let message = message.replace('\n', &format!("\n{}", " ".repeat(padding + 2)));
                format!("{lead}{message}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn write_line(&mut self, incompatibility: &Tree, message: String, numbered: bool) {
        let number = numbered.then(|| {
            let number = self.line_numbers.len() + 1;
            self.line_numbers.insert(key(incompatibility), number);
            number
        });
        self.lines.push((message, number));
    }

    fn line(&self, tree: &Tree) -> Option<usize> {
        self.line_numbers.get(&key(tree)).copied()
    }

    /// `FailureWriter#visit`.
    fn visit(&mut self, incompatibility: &Tree, conclusion: bool) {
        let Some((conflict, other)) = causes(incompatibility) else {
            unreachable!("only conflicts are visited");
        };
        let numbered = conclusion || self.derivations[&key(incompatibility)] > 1;
        let conjunction = if conclusion || key(incompatibility) == self.root {
            "So,"
        } else {
            "And"
        };
        let this = self.incompatibility(incompatibility);
        if is_conflict(conflict) && is_conflict(other) {
            match (self.line(conflict), self.line(other)) {
                (Some(conflict_line), Some(other_line)) => {
                    let message = format!(
                        "Because {} ({conflict_line})\nand {} ({other_line}),\n{this}.",
                        self.incompatibility(conflict),
                        self.incompatibility(other)
                    );
                    self.write_line(incompatibility, message, numbered);
                }
                (Some(_), None) | (None, Some(_)) => {
                    let (with_line, without_line) = if self.line(conflict).is_some() {
                        (conflict, other)
                    } else {
                        (other, conflict)
                    };
                    let line = self.line(with_line).expect("one has a line");
                    self.visit(without_line, false);
                    let message = format!(
                        "{conjunction} because {} ({line}),\n{this}.",
                        self.incompatibility(with_line)
                    );
                    self.write_line(incompatibility, message, numbered);
                }
                (None, None) => {
                    let single_line_conflict = single_line(conflict);
                    let single_line_other = single_line(other);
                    if single_line_conflict || single_line_other {
                        let (first, second) = if single_line_other {
                            (conflict, other)
                        } else {
                            (other, conflict)
                        };
                        self.visit(first, false);
                        self.visit(second, false);
                        self.write_line(incompatibility, format!("Thus, {this}."), numbered);
                    } else {
                        self.visit(conflict, true);
                        self.lines.push((String::new(), None));
                        self.visit(other, false);
                        let line = self.line(conflict).expect("a conclusion is numbered");
                        let message = format!(
                            "{conjunction} because {} ({line}),\n{this}.",
                            self.incompatibility(conflict)
                        );
                        self.write_line(incompatibility, message, numbered);
                    }
                }
            }
        } else if is_conflict(conflict) || is_conflict(other) {
            let (derived, ext) = if is_conflict(conflict) {
                (conflict, other)
            } else {
                (other, conflict)
            };
            if let Some(derived_line) = self.line(derived) {
                let message = format!(
                    "Because {}\nand {} ({derived_line}),\n{this}.",
                    self.incompatibility(ext),
                    self.incompatibility(derived)
                );
                self.write_line(incompatibility, message, numbered);
            } else if self.collapsible(derived) {
                let (derived_conflict, derived_other) = causes(derived).expect("a conflict");
                let (collapsed_derived, collapsed_ext) = if is_conflict(derived_conflict) {
                    (derived_conflict, derived_other)
                } else {
                    (derived_other, derived_conflict)
                };
                self.visit(collapsed_derived, false);
                let message = format!(
                    "{conjunction} because {}\nand {},\n{this}.",
                    self.incompatibility(collapsed_ext),
                    self.incompatibility(ext)
                );
                self.write_line(incompatibility, message, numbered);
            } else {
                self.visit(derived, false);
                let message = format!(
                    "{conjunction} because {},\n{this}.",
                    self.incompatibility(ext)
                );
                self.write_line(incompatibility, message, numbered);
            }
        } else {
            let message = format!(
                "Because {}\nand {},\n{this}.",
                self.incompatibility(conflict),
                self.incompatibility(other)
            );
            self.write_line(incompatibility, message, numbered);
        }
    }

    /// `collapsible?`.
    fn collapsible(&self, incompatibility: &Tree) -> bool {
        if self.derivations[&key(incompatibility)] > 1 {
            return false;
        }
        let Some((conflict, other)) = causes(incompatibility) else {
            return false;
        };
        if is_conflict(conflict) == is_conflict(other) {
            return false;
        }
        let complex = if is_conflict(conflict) {
            conflict
        } else {
            other
        };
        !self.line_numbers.contains_key(&key(complex))
    }

    fn count_derivations(&mut self, incompatibility: &Tree) {
        let entry = self.derivations.entry(key(incompatibility)).or_insert(0);
        *entry += 1;
        if *entry == 1
            && let Some((conflict, other)) = causes(incompatibility)
        {
            self.count_derivations(conflict);
            self.count_derivations(other);
        }
    }

    /// `Incompatibility#to_s`, with Bundler's custom explanations.
    fn incompatibility(&self, tree: &Tree) -> String {
        match tree {
            DerivationTree::External(External::NotRoot(..)) => "(root dependency)".to_owned(),
            DerivationTree::External(External::Custom(_, _, message)) => message.clone(),
            DerivationTree::External(External::NoVersions(package, range)) => {
                let platforms = if self.other_platform_specs(package, range).is_empty() {
                    String::new()
                } else {
                    let platforms: Vec<String> = self
                        .provider
                        .request
                        .platforms
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    format!(" for any resolution platforms ({})", platforms.join(", "))
                };
                format!(
                    "{} could not be found in {}{platforms}",
                    self.constraint(package, range, false),
                    self.provider.request.source
                )
            }
            DerivationTree::External(External::FromDependencyOf(package, held, dep, range)) => {
                if matches!(package, Package::Root(_)) && dep.is_meta() {
                    return format!(
                        "current {dep} version is {}",
                        constraint_string(range, true)
                    );
                }
                let span = self
                    .provider
                    .dependency_span(package, held, dep, range)
                    .unwrap_or_else(|| held.clone());
                format!(
                    "{} depends on {}",
                    self.constraint(package, &span, true),
                    self.named_constraint(dep, range)
                )
            }
            DerivationTree::Derived(derived) => {
                let mut terms: Vec<(&Package, &Term<VersionSet>)> = derived.terms.iter().collect();
                // `cleanup_terms`: of more than one, the root's positive term
                // goes without saying.
                if terms.len() != 1 {
                    terms.retain(|(p, t)| {
                        !(matches!(p, Package::Root(_)) && matches!(t, Term::Positive(_)))
                    });
                }
                terms.sort_by_key(|(p, _)| (!matches!(p, Package::Root(_)), p.to_string()));
                self.conflict_terms(&terms, &|p| passed_through(derived, p))
            }
        }
    }

    /// `Incompatibility#to_s` for a `ConflictCause`. A required range that
    /// `named` says came through from one dependency keeps its `~>` name.
    fn conflict_terms(
        &self,
        terms: &[(&Package, &Term<VersionSet>)],
        named: &dyn Fn(&Package) -> bool,
    ) -> String {
        let failure = matches!(terms, [] | [(Package::Root(_), Term::Positive(_))]);
        if failure {
            return "version solving has failed".to_owned();
        }
        let positive = |t: &Term<VersionSet>| matches!(t, Term::Positive(_));
        let range = |t: &Term<VersionSet>| -> VersionSet {
            match t {
                Term::Positive(r) | Term::Negative(r) => r.clone(),
            }
        };
        // A positive term rules versions of a gem out, which Bundler states
        // as runs between its candidates; a negative one is a dependency's
        // range, which it states as it is.
        let shown_range = |p: &Package, t: &Term<VersionSet>| {
            if positive(t) {
                self.displayed(p, &range(t))
            } else {
                range(t)
            }
        };
        // `Term#to_s`: a negative term is `not` its constraint.
        let term = |(p, t): &(&Package, &Term<VersionSet>)| {
            let shown = self.constraint(p, &shown_range(p, t), false);
            if positive(t) {
                shown
            } else {
                format!("not {shown}")
            }
        };
        let every =
            |(p, t): &(&Package, &Term<VersionSet>)| self.constraint(p, &shown_range(p, t), true);
        let invert = |(p, t): &(&Package, &Term<VersionSet>)| {
            if positive(t) {
                format!("not {}", self.constraint(p, &shown_range(p, t), false))
            } else if named(p) && !matches!(p, Package::Root(_)) {
                self.named_constraint(p, &range(t))
            } else {
                self.constraint(p, &range(t), false)
            }
        };
        if let [single] = terms {
            let (package, t) = single;
            return if positive(t) {
                if range(t) == Ranges::full() {
                    format!("{package} cannot be used")
                } else {
                    format!("{} cannot be used", every(single))
                }
            } else {
                format!("{} is required", invert(single))
            };
        }
        let positives: Vec<_> = terms.iter().filter(|(_, t)| positive(t)).collect();
        let negatives: Vec<_> = terms.iter().filter(|(_, t)| !positive(t)).collect();
        if negatives.is_empty() {
            if let [a, b] = terms {
                format!("{} is incompatible with {}", every(a), term(b))
            } else {
                let all: Vec<String> = terms.iter().map(term).collect();
                format!("one of {} must be false", all.join(" or "))
            }
        } else if positives.is_empty() {
            if let [a, b] = terms {
                format!("either {} or {}", invert(a), invert(b))
            } else {
                let all: Vec<String> = terms.iter().map(invert).collect();
                format!("one of {} must be true", all.join(" or "))
            }
        } else {
            let required: Vec<String> = negatives.iter().map(|t| invert(t)).collect();
            if let [only] = positives.as_slice() {
                format!("{} requires {}", every(only), required.join(" or "))
            } else {
                let all: Vec<String> = positives.iter().map(|t| term(t)).collect();
                format!("if {} then {}", all.join(" and "), required.join(" or "))
            }
        }
    }

    /// `VersionConstraint#to_s`: the root by its name, a whole range as
    /// `every version of` where allowed.
    fn constraint(&self, package: &Package, range: &VersionSet, allow_every: bool) -> String {
        if matches!(package, Package::Root(_)) {
            package.to_string()
        } else if allow_every && *range == Ranges::full() {
            format!("every version of {package}")
        } else {
            format!("{package} {}", constraint_string(range, false))
        }
    }

    /// A dependency's own range, which keeps the `~> x` name
    /// `requirement_to_range` gives it; ranges made from others do not.
    fn named_constraint(&self, package: &Package, range: &VersionSet) -> String {
        format!("{package} {}", constraint_string(range, true))
    }

    /// A range of versions ruled out, as Bundler's would be: the runs of the
    /// package's candidates it covers, each from its first candidate to the
    /// candidate after its last, open at either end of the list.
    fn displayed(&self, package: &Package, range: &VersionSet) -> VersionSet {
        if matches!(package, Package::Root(_)) {
            return range.clone();
        }
        let sorted = self.provider.sorted_candidates(package);
        let held: Vec<bool> = sorted.iter().map(|c| range.contains(c)).collect();
        if !held.contains(&true) {
            return range.clone();
        }
        let mut runs = Ranges::empty();
        let mut i = 0;
        while i < sorted.len() {
            if !held[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < sorted.len() && held[i] {
                i += 1;
            }
            let run = match (start, i == sorted.len()) {
                (0, true) => Ranges::full(),
                (0, false) => Ranges::strictly_lower_than(sorted[i].clone()),
                (start, true) => Ranges::higher_than(sorted[start].clone()),
                (start, false) => Ranges::between(sorted[start].clone(), sorted[i].clone()),
            };
            runs = runs.union(&run);
        }
        runs
    }

    /// The builds of `package` in `range` that no resolution platform could
    /// use: `specs_matching_other_platforms`.
    fn other_platform_specs(&self, package: &Package, range: &VersionSet) -> Vec<String> {
        let Package::Name(PackageName::Gem(name)) = package else {
            return Vec::new();
        };
        self.provider
            .all_specs(name)
            .iter()
            .filter(|s| {
                range.contains(&Candidate::new(s.version.clone(), -1))
                    || range.contains(&Candidate::new(s.version.clone(), 1))
            })
            .map(super::Spec::full_name)
            .collect()
    }

    /// `Resolver#find_names_to_relax`'s `extended_explanation`: along each
    /// derivation's first cause, the last version not found that builds for
    /// other platforms would have satisfied, listed.
    fn extended_explanation(&self, tree: &Tree) -> Option<String> {
        let mut found = None;
        let mut node = tree;
        while let Some((conflict, other)) = causes(node) {
            for candidate in [conflict, other] {
                if let DerivationTree::External(External::NoVersions(package, range)) = candidate {
                    let specs = self.other_platform_specs(package, range);
                    if !specs.is_empty() {
                        let label = format!("{package} ({})", constraint_string(range, false));
                        let lines: Vec<String> = specs.iter().map(|s| format!("  * {s}")).collect();
                        found = Some(format!(
                            "The source contains the following gems matching '{label}':\n{}",
                            lines.join("\n")
                        ));
                    }
                }
            }
            node = conflict;
        }
        found
    }
}

/// The tree without its facts about gaps between candidates (see the module
/// notes): a derived incompatibility one of whose causes says a range with
/// no candidates of a gem has no versions, and whose other cause rules that
/// gem out (a positive term on it), becomes that other cause, its range on
/// the gem widened by the gap.
fn collapse_gaps(tree: &Tree, provider: &Provider<'_>) -> Tree {
    let DerivationTree::Derived(derived) = tree else {
        return tree.clone();
    };
    let cause1 = collapse_gaps(&derived.cause1, provider);
    let cause2 = collapse_gaps(&derived.cause2, provider);
    for (gap, other) in [(&cause1, &cause2), (&cause2, &cause1)] {
        let DerivationTree::External(External::NoVersions(package, set)) = gap else {
            continue;
        };
        let Package::Name(name) = package else {
            continue;
        };
        if !provider.versions_for(name, set).is_empty() {
            continue;
        }
        match other {
            DerivationTree::External(External::FromDependencyOf(p, held, dep, range))
                if p == package =>
            {
                return DerivationTree::External(External::FromDependencyOf(
                    p.clone(),
                    held.union(set),
                    dep.clone(),
                    range.clone(),
                ));
            }
            DerivationTree::Derived(inner) => {
                if let Some(Term::Positive(held)) = inner.terms.get(package) {
                    let mut terms = inner.terms.clone();
                    terms.insert(package.clone(), Term::Positive(held.union(set)));
                    return DerivationTree::Derived(Derived {
                        terms,
                        shared_id: inner.shared_id,
                        cause1: inner.cause1.clone(),
                        cause2: inner.cause2.clone(),
                    });
                }
            }
            DerivationTree::External(_) => {}
        }
    }
    DerivationTree::Derived(Derived {
        terms: derived.terms.clone(),
        shared_id: derived.shared_id,
        cause1: Arc::new(cause1),
        cause2: Arc::new(cause2),
    })
}

/// Whether a derived incompatibility's term on `package` is a dependency's
/// range passed through unchanged, as pubgrub-ruby passes a term with its
/// name: one cause depends on `package` with that very range and the other
/// does not name `package`, so nothing was intersected or joined with it.
fn passed_through(derived: &Derived<Package, VersionSet, String>, package: &Package) -> bool {
    let Some(Term::Negative(range)) = derived.terms.get(package) else {
        return false;
    };
    let gives = |cause: &Tree| {
        matches!(cause, DerivationTree::External(External::FromDependencyOf(_, _, dep, r))
            if dep == package && r == range)
    };
    let names = |cause: &Tree| super::tree_packages(cause).contains(&package);
    (gives(&derived.cause1) && !names(&derived.cause2))
        || (gives(&derived.cause2) && !names(&derived.cause1))
}

/// `single_line?`: an incompatibility both of whose causes are external.
fn single_line(tree: &Tree) -> bool {
    causes(tree).is_some_and(|(a, b)| !is_conflict(a) && !is_conflict(b))
}

fn bound_version(bound: Bound<&Candidate>) -> Option<String> {
    match bound {
        Bound::Included(c) | Bound::Excluded(c) => Some(c.to_string()),
        Bound::Unbounded => None,
    }
}

/// `VersionConstraint#constraint_string`: `>= 0` for every version, else
/// the range as `VersionUnion#to_s` writes it, pieces that meet at a version
/// written as `!=` that version. With `named`, a `~>` range is written as
/// the requirement it came from.
pub(super) fn constraint_string(range: &VersionSet, named: bool) -> String {
    if *range == Ranges::full() {
        return ">= 0".to_owned();
    }
    let segments: Vec<(Bound<&Candidate>, Bound<&Candidate>)> = range
        .iter()
        .map(|(start, end)| (start.as_ref(), end.as_ref()))
        .collect();
    if segments.is_empty() {
        return "(no versions)".to_owned();
    }
    let mut output = Vec::new();
    let mut i = 0;
    while i < segments.len() {
        let (min, mut max) = segments[i];
        let mut ne = Vec::new();
        while i + 1 < segments.len()
            && bound_version(segments[i + 1].0).is_some()
            && bound_version(segments[i + 1].0) == bound_version(max)
        {
            ne.push(format!("!= {}", bound_version(max).expect("bounded")));
            max = segments[i + 1].1;
            i += 1;
        }
        i += 1;
        let any = matches!((min, max), (Bound::Unbounded, Bound::Unbounded));
        if ne.is_empty() {
            output.push(range_string(min, max, named));
        } else if any {
            output.push(ne.join(", "));
        } else {
            output.push(format!(
                "{}, {}",
                range_string(min, max, named),
                ne.join(", ")
            ));
        }
    }
    output.join(" OR ")
}

/// `VersionRange#to_s`: its name when it is a `~>` range that keeps one,
/// else `= v` for one version, else its bounds.
fn range_string(min: Bound<&Candidate>, max: Bound<&Candidate>, named: bool) -> String {
    if named
        && let (Bound::Included(low), Bound::Excluded(high)) = (min, max)
        && *high == pessimistic_end(low.version())
    {
        return format!("~> {low}");
    }
    if let (Some(low), Some(high)) = (bound_version(min), bound_version(max))
        && low == high
    {
        return format!("= {low}");
    }
    let mut parts = Vec::new();
    match min {
        Bound::Included(v) => parts.push(format!(">= {v}")),
        Bound::Excluded(v) => parts.push(format!("> {v}")),
        Bound::Unbounded => {}
    }
    match max {
        Bound::Included(v) => parts.push(format!("<= {v}")),
        Bound::Excluded(v) => parts.push(format!("< {v}")),
        Bound::Unbounded => {}
    }
    if parts.is_empty() {
        "any".to_owned()
    } else {
        parts.join(", ")
    }
}
