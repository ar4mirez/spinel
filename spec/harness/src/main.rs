//! `spec-harness` — runs ruby/spec against Spinel, until Spinel can run mspec.
//!
//! mspec is Ruby. The moment Spinel executes enough Ruby to run it, this binary
//! is deleted and `spec/ruby/spec_helper.rb` takes over; that is the phase 2
//! milestone. Everything here is written to be thrown away, and to be honest in
//! the meantime about the one thing it cannot do yet: run any Ruby at all.
//!
//! So the report has a `blocked` column. An example that cannot be executed is
//! not a pass and not a failure, and calling it either would make the pass
//! count — the project's progress bar — a lie.

mod discover;
mod run;
mod tags;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use discover::Target;
use run::Outcome;

const MILESTONES: &str = "https://github.com/ar4mirez/spinel/milestones";

const AFTER_HELP: &str = "\
An example runs when every construct in it compiles. One that mentions
something the VM cannot mean yet is reported blocked rather than failed, and
the run ends by ranking what the blocking constructs were — which is how the
next slice gets chosen from data.

This harness is temporary. mspec replaces it at the end of phase 2.";

#[derive(Parser, Debug)]
#[command(
    name = "spec-harness",
    about = "Run ruby/spec against Spinel.",
    after_help = AFTER_HELP
)]
struct Cli {
    /// Spec files, or directories to run every `*_spec.rb` under.
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Print every example's full description instead of counting them.
    /// A skipped example carries the reason, which is how a guard's effect is
    /// checked without reading the harness's source.
    #[arg(long)]
    list: bool,

    /// How many "blocked by" reasons to rank. The default is a readable
    /// summary; planning a slice wants the whole tail, so `--blocked 0` prints
    /// every reason.
    #[arg(long, value_name = "N", default_value_t = MAX_BLOCKED_REASONS)]
    blocked: usize,

    /// Where `<path>_tags.txt` files live. The default is the repository's
    /// `spec/tags/`, which is the only one anybody runs; the flag exists so a
    /// test can point at a corpus of its own.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_TAGS)]
    tags: PathBuf,
}

/// `spec/tags/`, relative to this crate. Resolved at compile time, like the
/// corpus path in `scripts/spec.sh`, so the binary works from any directory.
const DEFAULT_TAGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/tags");

#[derive(Default)]
struct Counts {
    files: usize,
    examples: usize,
    passed: usize,
    failed: usize,
    blocked: usize,
    skipped: usize,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
        self.files += other.files;
        self.examples += other.examples;
        self.passed += other.passed;
        self.failed += other.failed;
        self.blocked += other.blocked;
        self.skipped += other.skipped;
    }

    /// The one line every report ends with, and the only line a whole-corpus
    /// run prints. Same shape as `spinel parse`'s sweep, on purpose: one house
    /// style for "here is what happened to a pile of files".
    fn line(&self) -> String {
        format!(
            "{} examples · {} passed · {} failed · {} blocked · {} skipped",
            self.examples, self.passed, self.failed, self.blocked, self.skipped
        )
    }
}

/// How many per-file lines are worth printing before they stop being a report
/// and start being a wall. Above this, only the summary and the problems.
const MAX_LISTED_FILES: usize = 20;
/// How many unreadable files to name before summarising.
const MAX_REPORTED: usize = 20;
/// How many distinct "blocked by" reasons to rank before summarising. Long
/// enough to plan the next few slices from, short enough to read.
const MAX_BLOCKED_REASONS: usize = 15;

/// A spec file that did not parse. Not a failing example: a file the harness
/// could not read at all, which is a bug in Spinel rather than a result.
const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let started = Instant::now();
    let target = Target::default();
    // The default is written relative to this crate, so without this every tag
    // problem would name `spec/harness/../../spec/tags/...`. A tags directory
    // that is not there resolves to nothing and skips nothing, which is the
    // right answer for a checkout that has none.
    let tags_root = std::fs::canonicalize(&cli.tags).unwrap_or_else(|_| cli.tags.clone());

    let mut files = Vec::new();
    for path in &cli.paths {
        if let Err(err) = collect(path, &mut files) {
            eprintln!("spec-harness: cannot read `{}`: {err}", path.display());
            return ExitCode::from(EXIT_USAGE);
        }
    }
    files.sort();
    files.dedup();

    if files.is_empty() {
        // Silence here would look like a clean run over an empty corpus, which
        // is the one result a spec runner must never fake. The likeliest cause
        // is the submodule: a fresh clone has `spec/ruby` empty until it is
        // initialised.
        eprintln!("spec-harness: no `*_spec.rb` files under the given paths.");
        eprintln!("              If `spec/ruby` is empty: git submodule update --init");
        return ExitCode::from(EXIT_USAGE);
    }

    let mut totals = Counts::default();
    let mut unparseable: Vec<String> = Vec::new();
    // Files that parsed and yielded nothing. Almost always a spec that builds
    // its examples inside an `eval` string, which is Ruby this harness can see
    // but not read. Reported rather than counted as zero in silence: a run over
    // a directory that printed "0 examples" and nothing else would look like a
    // broken checkout instead of a known blind spot.
    let mut without_examples: Vec<String> = Vec::new();
    // Every expectation that disagreed with Ruby. A failure is a real result and
    // the one thing a spec run must never summarise away.
    let mut failures: Vec<String> = Vec::new();
    // What each blocked example was blocked *by*, counted. This is the run's
    // most useful output while the VM is partial: it names the next slice, in
    // order of how many examples it would unblock, from data rather than from a
    // guess about which corner of Ruby matters.
    let mut blocked_by: BTreeMap<String, usize> = BTreeMap::new();
    // Everything wrong with a `spec/tags/` file: a line the reader cannot use, a
    // tag with no reason, a tag naming an example that is no longer there. Each
    // one is a skip that has silently stopped happening, so they fail the run
    // rather than being reported and shrugged at.
    let mut tag_problems: Vec<String> = Vec::new();
    // A single file needs no per-file line: the summary names it. Above the
    // cap the lines stop being a report and become a wall.
    let show_files = (2..=MAX_LISTED_FILES).contains(&files.len()) && !cli.list;

    for file in &files {
        let Ok(source) = std::fs::read(file) else {
            unparseable.push(format!("{}: cannot read", display_path(file)));
            continue;
        };
        let parsed = spinel_parse::parse(&source);
        // A `*_spec.rb` Spinel cannot parse is a parser bug, not a spec result,
        // so it is reported apart from the counts and fails the run.
        if let Some(error) = parsed.errors.first() {
            unparseable.push(format!("{}: {}", display_path(file), error.message));
            continue;
        }

        let mut examples = discover::examples(&parsed.program, &target);
        // An example named in this file's `spec/tags/<path>_tags.txt` is
        // reported skipped with the reason written there, rather than run. A tag
        // is a debt, not a result: see `spec/tags/README.md`.
        let tag_file = tags::load(file, &tags_root);
        let tags_path = display_path(&tags::path_for(file, &tags_root));
        for problem in &tag_file.problems {
            tag_problems.push(format!("{tags_path}: {problem}"));
        }
        let mut reached = vec![false; tag_file.tags.len()];
        for example in &mut examples {
            let description = example.full_description();
            for (tag, reached) in tag_file.tags.iter().zip(&mut reached) {
                if tag.description == description {
                    *reached = true;
                    // A guard that already excluded this example keeps its own
                    // reason: it is the more specific one, and the tag is still
                    // reached, so it does not read as stale.
                    if example.skipped.is_none() {
                        example.skipped = Some(tag.reason.clone());
                    }
                }
            }
        }
        // A tag naming an example that is not there any more skips nothing, and
        // says it does. Upstream rewording one `it` is all it takes, which is why
        // this is checked on every run rather than trusted to review.
        for (tag, reached) in tag_file.tags.iter().zip(&reached) {
            if !reached {
                tag_problems.push(format!(
                    "{tags_path}: no example named `{}` in {}",
                    tag.description,
                    display_path(file)
                ));
            }
        }
        if examples.is_empty() {
            without_examples.push(display_path(file));
        }
        if cli.list {
            for example in &examples {
                // `outcome<TAB>start-end` before the description, because the
                // byte range is what `scripts/verify-passes.rb` slices out of
                // the file to replay an example on a real Ruby. Everything a
                // reader wants is still on the line.
                let outcome = match run::run(example) {
                    Outcome::Passed => "passed".to_owned(),
                    Outcome::Failed(why) => format!("failed: {why}"),
                    Outcome::Skipped => match &example.skipped {
                        Some(reason) => format!("skipped: {reason}"),
                        None => "skipped".to_owned(),
                    },
                    Outcome::Blocked(why) => format!("blocked: {why}"),
                };
                // Spans, comma-separated: the `before` bodies that ran first,
                // then the example's own. `verify-passes.rb` concatenates them
                // to rebuild exactly what was run.
                let spans: Vec<String> = example
                    .setup_spans
                    .iter()
                    .chain(std::iter::once(&example.span))
                    .map(|span| format!("{}-{}", span.start, span.end))
                    .collect();
                println!(
                    "{}\t{}\t{}\t{}",
                    display_path(file),
                    outcome,
                    spans.join(","),
                    example.full_description()
                );
            }
        }

        let mut counts = Counts {
            files: 1,
            examples: examples.len(),
            ..Counts::default()
        };
        for example in &examples {
            match run::run(example) {
                Outcome::Passed => counts.passed += 1,
                Outcome::Failed(why) => {
                    counts.failed += 1;
                    failures.push(format!(
                        "{} {}: {why}",
                        display_path(file),
                        example.full_description()
                    ));
                }
                Outcome::Skipped => counts.skipped += 1,
                Outcome::Blocked(why) => {
                    counts.blocked += 1;
                    *blocked_by.entry(why).or_insert(0usize) += 1;
                }
            }
        }
        if show_files {
            println!("{} · {}", display_path(file), counts.line());
        }
        totals.add(&counts);
    }

    // A blank line separates the detail above from the summary below. With no
    // detail — one file, nothing wrong with it — there is nothing to separate.
    if show_files {
        println!();
    }
    report("could not be parsed", &unparseable, None);
    report("failed", &failures, None);
    report(
        "tag problems",
        &tag_problems,
        Some("a tag is `fails(reason):full description` — see spec/tags/README.md"),
    );
    report(
        "no examples found",
        &without_examples,
        // Naming the cause matters more than the list. All three are Ruby that
        // has to run before the examples exist, which is exactly what this
        // harness cannot do and what mspec will.
        Some("built with `it_behaves_like`, `eval`, or a runtime `if` — all need a VM to expand"),
    );

    // Multi-file runs keep the `N files` prefix: CI reads the count off this
    // line, and a single run names its file instead so nothing is repeated.
    let subject = match files.as_slice() {
        [only] => display_path(only),
        many => format!("{} files", many.len()),
    };
    println!(
        "{subject} · {} · {:.1}s",
        totals.line(),
        started.elapsed().as_secs_f64(),
    );
    if totals.blocked > 0 {
        // Ranked by how many examples each reason accounts for, because that is
        // the order the remaining phase-1 slices are worth doing in.
        let mut ranked: Vec<(&String, &usize)> = blocked_by.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!();
        println!("blocked by, most examples first ({MILESTONES}):");
        // `--blocked 0` means "no cap": the whole tail, which is what planning
        // the next slice reads.
        let cap = if cli.blocked == 0 {
            ranked.len()
        } else {
            cli.blocked
        };
        for (reason, count) in ranked.iter().take(cap) {
            println!("  {count:>5}  {reason}");
        }
        if ranked.len() > cap {
            println!("  ... and {} more reasons", ranked.len() - cap);
        }
    }

    if totals.failed > 0 || !unparseable.is_empty() || !tag_problems.is_empty() {
        ExitCode::from(EXIT_FAILED)
    } else {
        ExitCode::SUCCESS
    }
}

/// A path as short as it can be without becoming ambiguous: relative to the
/// working directory when it is below it, absolute otherwise.
///
/// `scripts/spec.sh core/array` hands the harness an absolute path, and ninety
/// columns of it before the first count is not a report anyone reads.
fn display_path(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A named list of problem files, capped so a broken corpus still fits a screen.
fn report(heading: &str, lines: &[String], note: Option<&str>) {
    if lines.is_empty() {
        return;
    }
    println!("{heading} ({}):", lines.len());
    for line in lines.iter().take(MAX_REPORTED) {
        println!("  {line}");
    }
    if lines.len() > MAX_REPORTED {
        println!("  ... and {} more", lines.len() - MAX_REPORTED);
    }
    if let Some(note) = note {
        println!("  {note}");
    }
    println!();
}

/// Every `*_spec.rb` under a path, or the path itself if it is a file.
///
/// Only `*_spec.rb`: ruby/spec's `fixtures/` and `shared/` directories are Ruby
/// that specs load, not specs, and several fixtures are deliberately invalid
/// syntax. Sweeping those is `spinel parse`'s job, not this one's.
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let child = entry?.path();
        if child.is_dir() {
            collect(&child, out)?;
        } else if child
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_spec.rb"))
        {
            out.push(child);
        }
    }
    Ok(())
}
