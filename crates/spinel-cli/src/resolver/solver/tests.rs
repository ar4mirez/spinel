//! The port against pubgrub-ruby itself: `solver_oracle.txt` is what
//! Bundler 4.0.20's vendored pub_grub printed on CRuby 4.0.9 for the same
//! inputs (`tests/fixtures/resolver/solver_oracle.rb` writes it), and each
//! test prints the port's answers in the same lines.

use std::cell::RefCell;
use std::rc::Rc;

use crate::gem_requirement::Requirement;
use crate::gem_version::Version as GemVersion;

use super::super::Candidate;
use super::rubygems::parse_range;
use super::static_package_source::{StaticPackage, StaticPackageSource, root_version};
use super::{
    Cause, DefaultStrategy, Incompatibility, Level, RangeSet, VersionSolver, candidate_range,
};

const ORACLE: &str = include_str!("../../../tests/fixtures/resolver/solver_oracle.txt");

/// The oracle's section headed `== {name}`, as lines.
fn section(name: &str) -> Vec<&'static str> {
    let heading = format!("== {name}");
    let mut lines = ORACLE.lines().skip_while(|line| *line != heading);
    assert_eq!(lines.next(), Some(heading.as_str()), "no section {name}");
    lines.take_while(|line| !line.starts_with("== ")).collect()
}

/// `expected` and `actual` line for line, the first difference shown.
fn assert_lines(name: &str, expected: &[&str], actual: &[String]) {
    for (i, (want, got)) in expected.iter().zip(actual).enumerate() {
        assert_eq!(
            got,
            want,
            "{name}: line {} differs from pubgrub-ruby's\nwhole output:\n{}",
            i + 1,
            actual.join("\n")
        );
    }
    assert_eq!(
        actual.len(),
        expected.len(),
        "{name}: {} lines where pubgrub-ruby printed {}\nwhole output:\n{}",
        actual.len(),
        expected.len(),
        actual.join("\n")
    );
}

/// The ranges the oracle writes as requirements: `|` between the ones
/// unioned, `, ` between the pairs of one requirement.
const SETS: &[&str] = &[
    "~> 1.0",
    ">= 1.0",
    "> 1.0",
    "< 2.0",
    "<= 2.0",
    "= 1.5",
    "!= 1.5",
    ">= 1.0, < 2.0",
    "~> 1.5.1",
    ">= 0",
    "!= 1.0, != 2.0",
    "~> 2.0 | ~> 1.0",
    "< 1.0 | >= 2.0",
    "= 1.0 | = 1.5 | = 2.0",
    "< 1.0 | > 1.5, < 2.0 | >= 3.0",
];

const VERSIONS: &[&str] = &[
    "0.9", "1.0", "1.0.1", "1.5", "1.5.1", "1.6", "2.0.a", "2.0", "2.1", "3.0",
];

fn requirement(text: &str) -> Requirement {
    Requirement::new(text.split(", ")).unwrap()
}

/// `text`'s range, its requirements unioned in order (`inject(:union)`).
fn set<V: super::Version>(text: &str, range: impl Fn(&Requirement) -> RangeSet<V>) -> RangeSet<V> {
    let mut sets = text.split(" | ").map(|req| range(&requirement(req)));
    let first = sets.next().unwrap();
    sets.fold(first, |acc, set| acc.union(&set))
}

fn version_set(text: &str) -> RangeSet<GemVersion> {
    set(text, |req| {
        let pairs: Vec<String> = req
            .requirements()
            .iter()
            .map(|(op, v)| format!("{} {v}", op.as_str()))
            .collect();
        let pairs: Vec<&str> = pairs.iter().map(String::as_str).collect();
        parse_range(&pairs)
    })
}

#[test]
fn ranges_operate_as_pubgrub_ruby_s() {
    let mut versions: Vec<GemVersion> = VERSIONS
        .iter()
        .map(|v| GemVersion::new(v).unwrap())
        .collect();
    versions.sort();
    let mut out = Vec::new();
    for a in SETS {
        let ra = version_set(a);
        out.push(format!("to_s | {a} | | {ra}"));
        out.push(format!("invert | {a} | | {}", ra.invert()));
        out.push(format!("upper_invert | {a} | | {}", ra.upper_invert()));
        let selected: Vec<String> = ra
            .select_versions(&versions)
            .iter()
            .map(ToString::to_string)
            .collect();
        out.push(format!("select | {a} | | {}", selected.join(" ")));
        for b in SETS {
            let rb = version_set(b);
            out.push(format!("intersect | {a} | {b} | {}", ra.intersect(&rb)));
            out.push(format!("union | {a} | {b} | {}", ra.union(&rb)));
            out.push(format!("allows_all | {a} | {b} | {}", ra.allows_all(&rb)));
            out.push(format!("intersects | {a} | {b} | {}", ra.intersects(&rb)));
        }
    }
    assert_lines("ranges", &section("ranges"), &out);
}

/// Every version's two candidates, sorted.
fn candidates() -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = VERSIONS
        .iter()
        .flat_map(|v| [-1, 1].map(|p| Candidate::new(GemVersion::new(v).unwrap(), p)))
        .collect();
    candidates.sort();
    candidates
}

#[test]
fn candidate_ranges_are_bundler_s() {
    let candidates = candidates();
    let mut out = Vec::new();
    for text in SETS {
        let range = set(text, candidate_range);
        out.push(format!("to_s | {text} | | {range}"));
        let selected: Vec<String> = range
            .select_versions(&candidates)
            .iter()
            .map(|c| format!("{}/{}", c.version, c.priority))
            .collect();
        out.push(format!("select | {text} | | {}", selected.join(" ")));
    }
    assert_lines("candidates", &section("candidates"), &out);
}

/// Dart pub's `doc/solver.md` examples, and the oracle's three more, as
/// `solver_oracle.rb` states them.
fn example(name: &str) -> StaticPackageSource {
    let s = StaticPackageSource::new();
    match name {
        "no_conflicts" => s
            .root(&[("foo", "~> 1.0")])
            .add("foo", "1.0.0", &[("bar", "~> 1.0")])
            .add("bar", "1.0.0", &[])
            .add("bar", "2.0.0", &[]),
        "avoiding_conflict_during_decision_making" => s
            .root(&[("foo", "~> 1.0"), ("bar", "~> 1.0")])
            .add("foo", "1.1.0", &[("bar", "~> 2.0")])
            .add("foo", "1.0.0", &[])
            .add("bar", "1.0.0", &[])
            .add("bar", "1.1.0", &[])
            .add("bar", "2.0.0", &[]),
        "performing_conflict_resolution" => s
            .root(&[("foo", ">= 1.0.0")])
            .add("foo", "2.0.0", &[("bar", "~> 1.0")])
            .add("foo", "1.0.0", &[])
            .add("bar", "1.0.0", &[("foo", "~> 1.0")]),
        "conflict_resolution_with_a_partial_satisfier" => s
            .root(&[("foo", "~> 1.0"), ("target", "~> 2.0")])
            .add("foo", "1.1.0", &[("left", "~> 1.0"), ("right", "~> 1.0")])
            .add("foo", "1.0.0", &[])
            .add("left", "1.0.0", &[("shared", ">= 1.0.0")])
            .add("right", "1.0.0", &[("shared", "< 2.0.0")])
            .add("shared", "2.0.0", &[])
            .add("shared", "1.0.0", &[("target", "~> 1.0")])
            .add("target", "2.0.0", &[])
            .add("target", "1.0.0", &[]),
        "linear_error_reporting" => s
            .root(&[("foo", "~> 1.0"), ("baz", "~> 1.0")])
            .add("foo", "1.0.0", &[("bar", "~> 2.0")])
            .add("bar", "2.0.0", &[("baz", "~> 3.0")])
            .add("baz", "1.0.0", &[])
            .add("baz", "3.0.0", &[]),
        "branching_error_reporting" => s
            .root(&[("foo", "~> 1.0")])
            .add("foo", "1.0.0", &[("a", "~> 1.0"), ("b", "~> 1.0")])
            .add("foo", "1.1.0", &[("x", "~> 1.0"), ("y", "~> 1.0")])
            .add("a", "1.0.0", &[("b", "~> 2.0")])
            .add("b", "1.0.0", &[])
            .add("b", "2.0.0", &[])
            .add("x", "1.0.0", &[("y", "~> 2.0")])
            .add("y", "1.0.0", &[])
            .add("y", "2.0.0", &[]),
        "unknown_package" => s
            .root(&[("foo", ">= 1.0")])
            .add("foo", "2.0.0", &[("missing", ">= 1.0")])
            .add("foo", "1.0.0", &[]),
        "no_versions" => s
            .root(&[("foo", "~> 1.0")])
            .add("foo", "1.0.0", &[("bar", "~> 2.0")])
            .add("bar", "1.0.0", &[]),
        "knew_incompatibility" => s
            .root(&[("foo", ">= 1.0"), ("baz", "~> 1.0")])
            .add("foo", "1.1.0", &[("bar", "~> 1.0"), ("qux", "~> 1.0")])
            .add("foo", "1.0.0", &[("bar", "~> 1.0")])
            .add("qux", "1.0.0", &[("baz", "~> 2.0")])
            .add("bar", "1.0.0", &[("bar", ">= 1.0")])
            .add("baz", "1.0.0", &[])
            .add("baz", "2.0.0", &[]),
        _ => panic!("no example {name}"),
    }
}

/// The incompatibility, then indented, the two it was derived from.
fn derivation(
    incompatibility: &Incompatibility<StaticPackage, GemVersion>,
    depth: usize,
    out: &mut Vec<String>,
) {
    out.push(format!("{}{incompatibility}", "  ".repeat(depth)));
    if let Cause::Conflict {
        incompatibility,
        satisfier,
    } = incompatibility.cause()
    {
        derivation(incompatibility, depth + 1, out);
        derivation(satisfier, depth + 1, out);
    }
}

/// The example solved as the oracle runs it, printed as the oracle prints
/// it: the debug log, then the solution, or the failure's derivation and
/// its explanation.
fn solve(name: &str) -> Vec<String> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let mut solver = VersionSolver::new(
        example(name),
        StaticPackage::Root,
        DefaultStrategy::new(StaticPackage::Root, root_version()),
        Some(Box::new(move |_: Level, line: &str| {
            sink.borrow_mut().push(line.to_owned())
        })),
    );
    let mut result = Vec::new();
    match solver.solve() {
        Ok(solution) => {
            result.push("solution:".to_owned());
            for (package, version) in solution {
                result.push(format!("{package} {version}"));
            }
        }
        Err(failure) => {
            result.push("failure:".to_owned());
            derivation(&failure.incompatibility, 0, &mut result);
            // `SolveFailure#explanation`, by the port of `FailureWriter`.
            result.push("-- explanation".to_owned());
            let explanation = super::super::report::explanation(&failure.incompatibility);
            result.extend(explanation.split('\n').map(str::to_owned));
        }
    }
    let mut out = vec!["-- log".to_owned()];
    out.extend(log.borrow().iter().cloned());
    out.push("-- result".to_owned());
    out.extend(result);
    out
}

fn assert_example(name: &str) {
    assert_lines(name, &section(&format!("example {name}")), &solve(name));
}

#[test]
fn no_conflicts() {
    assert_example("no_conflicts");
}

#[test]
fn avoiding_conflict_during_decision_making() {
    assert_example("avoiding_conflict_during_decision_making");
}

#[test]
fn performing_conflict_resolution() {
    assert_example("performing_conflict_resolution");
}

#[test]
fn conflict_resolution_with_a_partial_satisfier() {
    assert_example("conflict_resolution_with_a_partial_satisfier");
}

#[test]
fn linear_error_reporting() {
    assert_example("linear_error_reporting");
}

#[test]
fn branching_error_reporting() {
    assert_example("branching_error_reporting");
}

#[test]
fn a_dependency_on_an_unknown_package_rules_the_version_out() {
    assert_example("unknown_package");
}

#[test]
fn a_requirement_no_version_meets_is_a_no_versions_fact() {
    assert_example("no_versions");
}

#[test]
fn a_fact_already_known_is_not_added_again() {
    assert_example("knew_incompatibility");
}

/// pubgrub-ruby shares a derivation's causes by reference, which
/// `FailureWriter` numbers by: the failure's causes are the incompatibilities
/// conflict resolution started from, not copies.
#[test]
fn a_derivation_shares_its_causes() {
    let mut solver = VersionSolver::new(
        example("linear_error_reporting"),
        StaticPackage::Root,
        DefaultStrategy::new(StaticPackage::Root, root_version()),
        None,
    );
    let failure = solver.solve().unwrap_err();
    let Cause::Conflict {
        incompatibility, ..
    } = failure.incompatibility.cause()
    else {
        panic!("a derived failure");
    };
    let Cause::Conflict {
        incompatibility: first,
        ..
    } = incompatibility.cause()
    else {
        panic!("derived");
    };
    let Cause::Conflict {
        incompatibility: dependency,
        ..
    } = first.cause()
    else {
        panic!("derived");
    };
    assert!(matches!(dependency.cause(), Cause::Dependency));
    assert_eq!(
        dependency.to_string(),
        "every version of foo depends on bar ~> 2.0"
    );
    // One instance: two references from the solver's own lists, the
    // derivation's own, and none copied.
    assert!(Rc::strong_count(dependency) >= 2);
}
