//! What the capture-vector clone in `spinel-regex` costs, if anything.
//!
//! [#184] proposes replacing snapshot-and-restore with Onigmo's restore
//! records: today `exec.rs` clones the whole capture vector at every backtrack
//! point, so a pattern with `n` groups pays `O(n)` allocation per alternative
//! tried. The issue makes a benchmark the gating item and says, in as many
//! words, that if the clone cost cannot be shown to matter the right outcome is
//! to close it rather than do the rewrite.
//!
//! So this measures the one thing that decides it: **hold the backtracking
//! fixed and vary the group count.** If the clone dominates, time grows with
//! the number of groups. If it is noise beside the matching itself, the line is
//! flat and the rewrite buys nothing.
//!
//! `GROUPS` is that experiment. Every pattern in it does the same catastrophic
//! walk over the same subject — `(?:a|a)*` repeated, which forces an
//! exponential number of alternatives — and differs only in how many capture
//! groups are along for the ride. Group `n` is never referenced, so the only
//! thing extra groups change is the size of the vector each `Split` clones.
//!
//! `REAL` is the sanity check beside it: patterns with the shape real code
//! uses, timed against system `ruby --yjit` the way `CLAUDE.md` requires, so a
//! conclusion drawn from the synthetic case can be checked against something
//! anyone would actually write.
//!
//! Not criterion, for the reason `method_cache.rs` gives: the question is
//! "order of magnitude or noise", and `Instant` answers it.
//!
//! [#184]: https://github.com/ar4mirez/spinel/issues/184

use std::time::{Duration, Instant};

use spinel_regex::{Flags, Regex};

/// Min of five runs, as in `method_cache.rs`: the quantity wanted is the time
/// with nothing else interfering, and a mean folds in the scheduler.
fn time(iterations: u32, mut body: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..iterations {
            body();
        }
        best = best.min(start.elapsed() / iterations);
    }
    best
}

/// The same backtracking walk, with a growing number of unreferenced groups.
///
/// The subject fails to match, so the machine explores every alternative before
/// giving up — which is exactly the shape that pays for a clone per `Split`.
fn groups_experiment() {
    println!("clone cost: the same backtracking, more groups");
    println!(
        "  {:>7}  {:>12}  {:>10}",
        "groups", "per match", "vs 1 group"
    );
    // Fourteen `a`s and a `b`: the `b` can never match, so the machine explores
    // every one of the 2^14 paths before giving up.
    let subject = "aaaaaaaaaaaaaab";
    let mut baseline = Duration::ZERO;
    for groups in [1usize, 2, 4, 8, 16, 32] {
        // `()` repeated `groups` times, then the backtracking tail. Empty
        // groups on purpose: they consume nothing, so every row does the *same*
        // walk over the same subject and the only thing that changes is how
        // many slots each `Split` clones. `(a)` would have eaten the input and
        // measured a different, shorter search each time.
        let prefix: String = std::iter::repeat_n("()", groups).collect();
        let source = format!("^{prefix}(?:a|a)*$");
        let re = Regex::new(&source, Flags::default()).expect("a valid pattern");
        let per = time(20, || {
            let _ = std::hint::black_box(re.is_match(std::hint::black_box(subject)));
        });
        if groups == 1 {
            baseline = per;
        }
        let ratio = per.as_secs_f64() / baseline.as_secs_f64();
        println!("  {groups:>7}  {:>12?}  {ratio:>9.2}x", per);
    }
    println!();
}

/// Patterns with the shape real code uses, for the `ruby --yjit` comparison.
///
/// Read from `bench/regex-patterns.txt` rather than written here, because
/// `scripts/bench.sh --ruby` times the same file on system Ruby. Two copies of
/// a pattern drift, and a comparison whose sides measure different work is
/// worse than no comparison.
const PATTERNS: &str = include_str!("regex-patterns.txt");

fn real_patterns() {
    println!("real patterns, {REAL_ITERATIONS} iterations each");
    println!("  {:<20}  {:>12}  {:>7}", "pattern", "per match", "groups");
    for line in PATTERNS.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(name), Some(source), Some(subject)) =
            (fields.next(), fields.next(), fields.next())
        else {
            panic!("regex-patterns.txt: expected name<TAB>pattern<TAB>subject: {line:?}");
        };
        let re = Regex::new(source, Flags::default()).expect("a valid pattern");
        let groups = re.group_count();
        let per = time(REAL_ITERATIONS, || {
            let _ = std::hint::black_box(re.find_at(std::hint::black_box(subject), 0));
        });
        println!("  {name:<20}  {per:>12?}  {groups:>7}");
    }
    println!();
}

const REAL_ITERATIONS: u32 = 20_000;

/// The same real patterns with every capture group turned non-capturing.
///
/// This is the measurement that decides #184 on something anyone would write,
/// rather than on a synthetic worst case: the walk is identical, so whatever
/// time the capturing version costs *extra* is the whole capture machinery —
/// the per-`Split` clone included. If that share is small, a cheaper clone
/// cannot make the engine meaningfully faster.
fn capture_share() {
    println!("what captures cost on the same walk");
    println!(
        "  {:<20}  {:>12}  {:>12}  {:>9}",
        "pattern", "capturing", "non-capturing", "captures"
    );
    for line in PATTERNS.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(name), Some(source), Some(subject)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // `(` that does not already start a `(?...)` group becomes `(?:`. The
        // patterns in the file have no parenthesis inside a character class, so
        // this stays a text substitution rather than needing the parser.
        let mut bare = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                bare.push(c);
                if let Some(next) = chars.next() {
                    bare.push(next);
                }
                continue;
            }
            bare.push(c);
            if c == '(' && chars.peek() != Some(&'?') {
                bare.push_str("?:");
            }
        }
        let with = Regex::new(source, Flags::default()).expect("a valid pattern");
        let without = Regex::new(&bare, Flags::default()).expect("a valid pattern");
        let a = time(REAL_ITERATIONS, || {
            let _ = std::hint::black_box(with.find_at(std::hint::black_box(subject), 0));
        });
        let b = time(REAL_ITERATIONS, || {
            let _ = std::hint::black_box(without.find_at(std::hint::black_box(subject), 0));
        });
        let share = 100.0 * (a.as_secs_f64() - b.as_secs_f64()) / a.as_secs_f64();
        println!("  {name:<20}  {a:>12?}  {b:>12?}  {share:>8.0}%");
    }
    println!();
}

fn main() {
    groups_experiment();
    real_patterns();
    capture_share();
    println!("  Compare against system ruby with:  scripts/bench.sh --ruby");
}
