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

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use discover::{Example, Target};

const MILESTONES: &str = "https://github.com/ar4mirez/spinel/milestones";

const AFTER_HELP: &str = "\
No example can pass yet: this build has no VM, so every example is reported
blocked. That is the point — the counts are real, and they move when phase 1
lands the interpreter.

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
}

/// What became of one example.
enum Outcome {
    /// A guard excluded it, or the harness would have had to guess.
    Skipped,
    /// Nothing can run it yet.
    //
    // ponytail: this is the whole VM-shaped hole. Phase 1 replaces it with a
    // real evaluation of the example body, and `Passed`/`Failed` start moving.
    Blocked,
}

fn outcome(example: &Example) -> Outcome {
    if example.skipped.is_some() {
        Outcome::Skipped
    } else {
        Outcome::Blocked
    }
}

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

/// A spec file that did not parse. Not a failing example: a file the harness
/// could not read at all, which is a bug in Spinel rather than a result.
const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let started = Instant::now();
    let target = Target::default();

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

        let examples = discover::examples(&parsed.program, &target);
        if examples.is_empty() {
            without_examples.push(display_path(file));
        }
        if cli.list {
            for example in &examples {
                let reason = match &example.skipped {
                    Some(reason) => format!("\tskipped: {reason}"),
                    None => String::new(),
                };
                println!(
                    "{}\t{}{reason}",
                    display_path(file),
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
            match outcome(example) {
                Outcome::Skipped => counts.skipped += 1,
                Outcome::Blocked => counts.blocked += 1,
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
        println!("blocked: this build has no VM. Running Ruby lands in phase 1: {MILESTONES}");
    }

    if totals.failed > 0 || !unparseable.is_empty() {
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
