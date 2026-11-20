//! A failed resolution's explanation, as Bundler writes it: pubgrub-ruby's
//! `FailureWriter` (`failure_writer.rb`, a port of Dart pub's), as vendored
//! in Bundler 4.0.20, ported line for line over the solver's own
//! incompatibilities (`resolver/solver/`). Each incompatibility is written
//! by its `to_s`, which carries Bundler's wording where Bundler gives one
//! (`custom_explanation`: `current Ruby version is ...`, `... could not be
//! found in ...`); the extended explanation `find_names_to_relax` picks up
//! follows the whole (`Resolver#solve_versions`).
//!
//! `FailureWriter` keys its counts and line numbers by `Hash`, so by
//! `Incompatibility#eql?`: two incompatibilities with equal causes and
//! terms are one, wherever in the derivation each was reached. Only "is
//! this the failure itself" asks for the very same object (`==`, which
//! `Incompatibility` leaves as identity).

use std::collections::HashMap;
use std::rc::Rc;

use super::solver::{Cause, Incompatibility, Package, Term, Version};

/// `SolveFailure#explanation`: `FailureWriter.new(incompatibility).write`.
pub(super) fn explanation<P: Package, V: Version>(root: &Shared<P, V>) -> String {
    FailureWriter::new(root).write()
}

/// An incompatibility's identity as a `Hash` key sees it: `cause` and
/// `terms`, the causes of a conflict by their own identities in turn.
#[derive(PartialEq, Eq, Hash)]
enum Key<P, V> {
    Root,
    Dependency,
    InvalidDependency(P, String),
    NoVersions(Term<P, V>),
    Conflict(usize, usize),
}

/// An incompatibility, shared by the derivations that reach it.
type Shared<P, V> = Rc<Incompatibility<P, V>>;

/// What decides an incompatibility's `eql?` class: its terms and its cause.
type Class<P, V> = (Vec<Term<P, V>>, Key<P, V>);

/// `FailureWriter`.
struct FailureWriter<'r, P, V> {
    root: &'r Shared<P, V>,
    /// Each incompatibility reached, by address, and its `eql?` class.
    ids: HashMap<*const Incompatibility<P, V>, usize>,
    classes: HashMap<Class<P, V>, usize>,
    /// `@derivations`: `{ Incompatibility => Integer }`.
    derivations: HashMap<usize, usize>,
    /// `@lines`: `[ [ String, Integer or nil ] ]`.
    lines: Vec<(String, Option<usize>)>,
    /// `@line_numbers`: `{ Incompatibility => Integer }`, with the order its
    /// keys were first set in, which `values.last` reads.
    line_numbers: HashMap<usize, usize>,
    line_order: Vec<usize>,
}

impl<'r, P: Package, V: Version> FailureWriter<'r, P, V> {
    fn new(root: &'r Shared<P, V>) -> Self {
        let mut writer = FailureWriter {
            root,
            ids: HashMap::new(),
            classes: HashMap::new(),
            derivations: HashMap::new(),
            lines: Vec::new(),
            line_numbers: HashMap::new(),
            line_order: Vec::new(),
        };
        writer.count_derivations(root);
        writer
    }

    /// The `eql?` class of `incompatibility`: the same for any two with
    /// equal causes and terms. Classes are numbered as they are first met,
    /// so a derivation's are worked out once each, however often it is
    /// shared.
    fn id(&mut self, incompatibility: &Shared<P, V>) -> usize {
        let address = Rc::as_ptr(incompatibility);
        if let Some(&id) = self.ids.get(&address) {
            return id;
        }
        let key = match incompatibility.cause() {
            Cause::Root => Key::Root,
            Cause::Dependency => Key::Dependency,
            Cause::InvalidDependency {
                package,
                constraint,
            } => Key::InvalidDependency(package.clone(), constraint.clone()),
            Cause::NoVersions(term) => Key::NoVersions(term.clone()),
            Cause::Conflict {
                incompatibility: conflict,
                satisfier: other,
            } => Key::Conflict(self.id(conflict), self.id(other)),
        };
        let next = self.classes.len();
        let id = *self
            .classes
            .entry((incompatibility.terms().to_vec(), key))
            .or_insert(next);
        self.ids.insert(address, id);
        id
    }

    /// `write`.
    fn write(&mut self) -> String {
        if !self.root.is_conflict() {
            return self.root.to_string();
        }

        let root = self.root;
        self.visit(root, false);

        let padding = self
            .line_order
            .last()
            .map_or(0, |last| format!("({}) ", self.line_numbers[last]).len());

        self.lines
            .iter()
            .map(|(message, number)| {
                if message.is_empty() {
                    return String::new();
                }

                let lead = number.map_or(String::new(), |number| format!("({number}) "));
                let lead = format!("{lead:<padding$}");
                let message = message.replace('\n', &format!("\n{}", " ".repeat(padding + 2)));
                format!("{lead}{message}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `write_line`.
    fn write_line(&mut self, incompatibility: &Shared<P, V>, message: String, numbered: bool) {
        let number = numbered.then(|| {
            let number = self.line_numbers.len() + 1;
            let id = self.id(incompatibility);
            if self.line_numbers.insert(id, number).is_none() {
                self.line_order.push(id);
            }
            number
        });

        self.lines.push((message, number));
    }

    /// `@line_numbers[incompatibility]`.
    fn line_number(&mut self, incompatibility: &Shared<P, V>) -> Option<usize> {
        let id = self.id(incompatibility);
        self.line_numbers.get(&id).copied()
    }

    /// `@derivations[incompatibility]`.
    fn derivations(&mut self, incompatibility: &Shared<P, V>) -> usize {
        let id = self.id(incompatibility);
        self.derivations[&id]
    }

    /// `visit(incompatibility, conclusion:)`.
    fn visit(&mut self, incompatibility: &Shared<P, V>, conclusion: bool) {
        let (conflict, other) = causes(incompatibility);

        let numbered = conclusion || self.derivations(incompatibility) > 1;
        let conjunction = if conclusion || Rc::ptr_eq(incompatibility, self.root) {
            "So,"
        } else {
            "And"
        };

        if conflict.is_conflict() && other.is_conflict() {
            let conflict_line = self.line_number(conflict);
            let other_line = self.line_number(other);

            if let (Some(conflict_line), Some(other_line)) = (conflict_line, other_line) {
                self.write_line(
                    incompatibility,
                    format!(
                        "Because {conflict} ({conflict_line})\nand {other} ({other_line}),\n{incompatibility}."
                    ),
                    numbered,
                );
            } else if conflict_line.is_some() || other_line.is_some() {
                let (with_line, without_line) = if conflict_line.is_some() {
                    (conflict, other)
                } else {
                    (other, conflict)
                };
                let line = self
                    .line_number(with_line)
                    .expect("one of the two has a line");

                self.visit(without_line, false);
                self.write_line(
                    incompatibility,
                    format!("{conjunction} because {with_line} ({line}),\n{incompatibility}."),
                    numbered,
                );
            } else {
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
                    self.write_line(
                        incompatibility,
                        format!("Thus, {incompatibility}."),
                        numbered,
                    );
                } else {
                    self.visit(conflict, true);
                    self.lines.push((String::new(), None));
                    self.visit(other, false);

                    let conflict_line = self
                        .line_number(conflict)
                        .expect("a conclusion is numbered");
                    self.write_line(
                        incompatibility,
                        format!(
                            "{conjunction} because {conflict} ({conflict_line}),\n{incompatibility}."
                        ),
                        numbered,
                    );
                }
            }
        } else if conflict.is_conflict() || other.is_conflict() {
            let (derived, ext) = if conflict.is_conflict() {
                (conflict, other)
            } else {
                (other, conflict)
            };

            if let Some(derived_line) = self.line_number(derived) {
                self.write_line(
                    incompatibility,
                    format!("Because {ext}\nand {derived} ({derived_line}),\n{incompatibility}."),
                    numbered,
                );
            } else if self.collapsible(derived) {
                let (derived_conflict, derived_other) = causes(derived);
                let (collapsed_derived, collapsed_ext) = if derived_conflict.is_conflict() {
                    (derived_conflict, derived_other)
                } else {
                    (derived_other, derived_conflict)
                };

                self.visit(collapsed_derived, false);

                self.write_line(
                    incompatibility,
                    format!(
                        "{conjunction} because {collapsed_ext}\nand {ext},\n{incompatibility}."
                    ),
                    numbered,
                );
            } else {
                self.visit(derived, false);
                self.write_line(
                    incompatibility,
                    format!("{conjunction} because {ext},\n{incompatibility}."),
                    numbered,
                );
            }
        } else {
            self.write_line(
                incompatibility,
                format!("Because {conflict}\nand {other},\n{incompatibility}."),
                numbered,
            );
        }
    }

    /// `collapsible?`.
    fn collapsible(&mut self, incompatibility: &Shared<P, V>) -> bool {
        if self.derivations(incompatibility) > 1 {
            return false;
        }

        let (conflict, other) = causes(incompatibility);
        // If incompatibility is derived from two derived incompatibilities,
        // there are too many transitive causes to display concisely.
        if conflict.is_conflict() && other.is_conflict() {
            return false;
        }

        // If incompatibility is derived from two external incompatibilities,
        // it tends to be confusing to collapse it.
        if !(conflict.is_conflict() || other.is_conflict()) {
            return false;
        }

        // If incompatibility's internal cause is numbered, collapsing it
        // would get too noisy.
        let complex = if conflict.is_conflict() {
            conflict
        } else {
            other
        };

        self.line_number(complex).is_none()
    }

    /// `count_derivations`.
    fn count_derivations(&mut self, incompatibility: &Shared<P, V>) {
        let id = self.id(incompatibility);
        if let Some(count) = self.derivations.get_mut(&id) {
            *count += 1;
        } else {
            self.derivations.insert(id, 1);
            if let Cause::Conflict {
                incompatibility: conflict,
                satisfier: other,
            } = incompatibility.cause()
            {
                self.count_derivations(conflict);
                self.count_derivations(other);
            }
        }
    }
}

/// `cause.conflict` and `cause.other` of a conflict's incompatibility.
fn causes<P: Package, V: Version>(
    incompatibility: &Incompatibility<P, V>,
) -> (&Shared<P, V>, &Shared<P, V>) {
    match incompatibility.cause() {
        Cause::Conflict {
            incompatibility,
            satisfier,
        } => (incompatibility, satisfier),
        _ => unreachable!("only a conflict's incompatibility is visited"),
    }
}

/// `single_line?(incompatibility.cause)`: both causes external.
fn single_line<P: Package, V: Version>(incompatibility: &Incompatibility<P, V>) -> bool {
    let (conflict, other) = causes(incompatibility);
    !conflict.is_conflict() && !other.is_conflict()
}
