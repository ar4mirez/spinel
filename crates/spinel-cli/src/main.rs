//! The `spinel` binary.
//!
//! Phase 0 ships the skeleton: argument parsing, a version banner, and
//! `spinel parse`, which is the debugging window onto `spinel_ast` that every
//! later phase is built through. Subcommands land one issue at a time; see
//! `docs/cli.md` for the full surface.

mod tree;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};

const MILESTONES: &str = "https://github.com/ar4mirez/spinel/milestones";

/// Shown under the options list. States plainly what this build does and does
/// not do, so nobody installs it and wonders why `spinel app.rb` does nothing.
const AFTER_HELP: &str = "\
This build runs Ruby. The core library is minimal — Kernel, Object, Integer,
String, Symbol, Array, Hash, Comparable — and a program that reaches past it
says which method is missing rather than failing quietly.

Planned surface (docs/cli.md): x, init, install, add, remove, update, test, build.
Progress: https://github.com/ar4mirez/spinel/milestones";

#[derive(Parser, Debug)]
#[command(
    name = "spinel",
    about = "A Ruby engine and toolchain, in one binary.",
    disable_version_flag = true,
    arg_required_else_help = true,
    override_usage = "spinel [OPTIONS] [COMMAND]",
    after_help = AFTER_HELP,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Print version and exit.
    // `-v` is how `ruby` spells it and is the spelling most users will reach for;
    // `-V` is the convention everywhere else. Both, and both visible.
    #[arg(short = 'V', visible_short_alias = 'v', long = "version")]
    version: bool,

    /// Hidden so `--help` does not advertise a surface that is not built. It exists
    /// only to turn `spinel app.rb` from "unexpected argument" into a real answer.
    #[arg(hide = true)]
    file: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a Ruby file.
    Run {
        /// A Ruby file.
        path: PathBuf,

        /// Print the compiled bytecode instead of running it.
        ///
        /// The debugging window onto the compiler that `spinel parse` is onto
        /// the syntax tree.
        #[arg(long)]
        dump_bytecode: bool,
    },

    /// Parse Ruby and print the syntax tree.
    ///
    /// Give it a file to see one tree. Give it a directory to sweep every `.rb`
    /// file under it and report only what failed, which is how the parser is
    /// checked against a real corpus.
    Parse {
        /// A Ruby file, or a directory to sweep.
        path: PathBuf,

        /// How to print the tree.
        #[arg(long, value_enum, default_value_t = Format::Tree)]
        format: Format,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// One line per node, spans in a column. The readable one.
    Tree,
    /// The derived `Debug`, every field, nothing elided.
    Debug,
}

fn main() -> ExitCode {
    // `arg_required_else_help` means a bare `spinel` never reaches this point: clap
    // prints help and exits non-zero, so a typo is never a silent success.
    let cli = Cli::parse();

    if cli.version {
        println!("{}", spinel_vm::description());
        return ExitCode::SUCCESS;
    }

    match cli.command {
        Some(Command::Parse { path, format }) => return parse_command(&path, format),
        Some(Command::Run {
            path,
            dump_bytecode,
        }) => return run_command(&path, dump_bytecode),
        None => {}
    }

    if let Some(argument) = cli.file {
        // A Ruby file is the likeliest first thing a Ruby developer types; a
        // mistyped subcommand is the second. They need opposite answers, and
        // running a typo as a filename would report "no such file" about a word
        // that was never meant to be one.
        if looks_like_a_ruby_file(&argument) {
            return run_command(Path::new(&argument), false);
        }
        eprintln!("spinel: unknown subcommand `{argument}`.");
        eprintln!("        This build has two: run, parse. Try `spinel --help`.");
        return ExitCode::from(EXIT_USAGE);
    }

    ExitCode::SUCCESS
}

/// Whether a bare argument is a Ruby file rather than a mistyped subcommand.
/// Named `.rb`, or on disk under any name — `spinel Rakefile` should be read as
/// a file, and `spinel pasre` should not.
fn looks_like_a_ruby_file(argument: &str) -> bool {
    let path = Path::new(argument);
    path.extension().is_some_and(|e| e == "rb") || path.is_file()
}

// ---------------------------------------------------------------------------
// spinel run
// ---------------------------------------------------------------------------

/// Exit code for a Ruby program that ended in an uncaught exception. Ruby exits
/// 1, and a script wrapping `spinel run` should not have to tell the two apart.
const EXIT_RAISED: u8 = 1;

fn run_command(path: &Path, dump_bytecode: bool) -> ExitCode {
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("spinel: cannot read `{}`: {err}", path.display());
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let parsed = spinel_parse::parse(&source);
    let color = use_color();
    for error in &parsed.errors {
        eprintln!(
            "{}",
            render_diagnostic(path, &source, error, "error", color)
        );
    }
    if !parsed.is_ok() {
        return ExitCode::from(EXIT_SYNTAX);
    }

    let iseq = match spinel_vm::compile::program(&parsed.program) {
        Ok(iseq) => iseq,
        Err(unsupported) => {
            // A construct the compiler does not implement yet is not a syntax
            // error and must not be reported as one: the file is valid Ruby and
            // Spinel is the thing that is unfinished. Saying which construct is
            // what makes the next slice choosable.
            eprintln!("spinel: cannot run `{}` yet: {unsupported}", path.display());
            eprintln!("        Progress: {MILESTONES}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    if dump_bytecode {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{iseq:#?}");
        return ExitCode::SUCCESS;
    }

    let mut heap = spinel_vm::Heap::new();
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);

    let mut frame = spinel_vm::interp::Frame::new(0);
    match spinel_vm::interp::eval_in(&mut scope, &mut frame, &iseq) {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            // Ruby prints `file:line: message (Class)`. Spinel has no line
            // numbers on a raise yet — backtraces are PRD 0012's non-goal — so
            // the file is named and the line is not invented.
            eprintln!("{}: {}", path.display(), describe_error(&err));
            ExitCode::from(EXIT_RAISED)
        }
    }
}

/// One line for an error that ended the program.
///
/// A raise is Ruby's own `message (Class)`. Everything else is the VM saying it
/// cannot run the program — a missing method, an unfinished construct — and
/// those read as engine limits, with the milestone to follow.
fn describe_error(err: &spinel_vm::Error) -> String {
    match err {
        // The exception object reached the top with no handler. This is the
        // ordinary end of a Ruby program that raised, and reads as Ruby's does.
        spinel_vm::Error::Uncaught { class, message } => format!("{message} ({class})"),
        // The VM decided to raise and the unwinder never got to build an
        // object — an arity error at the outermost frame, say. Same shape.
        spinel_vm::Error::Raise { class, message } => format!("{message} ({class})"),
        // A method this build does not have. Ruby's own wording, because that
        // is what the reader is looking for, plus the one line that says the
        // difference: this is Spinel being unfinished, not the program being
        // wrong, and `rescue NoMethodError` deliberately did not catch it.
        spinel_vm::Error::NoSuchMethod { name, class } => format!(
            "undefined method '{name}' for an instance of {class} (NoMethodError)\n\
             \x20       Spinel's core library is still minimal, so this may be Spinel \
             rather than your program.\n\
             \x20       What exists: {MILESTONES}"
        ),
        // Everything else is the engine saying it cannot run the program
        // rather than the program failing, so it points at the milestones.
        other => format!("{other} — see {MILESTONES}"),
    }
}

// ---------------------------------------------------------------------------
// spinel parse
// ---------------------------------------------------------------------------

/// Exit code for a file that did not parse. Ruby exits 1 on a syntax error, so
/// a script wrapping `spinel parse` can treat both the same way.
const EXIT_SYNTAX: u8 = 1;
/// Exit code for a problem with the invocation itself: no such file, unreadable.
const EXIT_USAGE: u8 = 2;

fn parse_command(path: &Path, format: Format) -> ExitCode {
    if path.is_dir() {
        return sweep(path);
    }
    parse_one(path, format)
}

fn parse_one(path: &Path, format: Format) -> ExitCode {
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("spinel: cannot read `{}`: {err}", path.display());
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let parsed = spinel_parse::parse(&source);
    let color = use_color();

    for warning in &parsed.warnings {
        eprintln!(
            "{}",
            render_diagnostic(path, &source, warning, "warning", color)
        );
    }
    for error in &parsed.errors {
        eprintln!(
            "{}",
            render_diagnostic(path, &source, error, "error", color)
        );
    }

    // The tree is printed even when the parse failed: Prism recovers, and the
    // half-tree is usually what the reader came to look at.
    let mut stdout = std::io::stdout().lock();
    let rendered = match format {
        Format::Tree => tree::render(&parsed.program, color),
        Format::Debug => format!("{:#?}\n", parsed.program),
    };
    let _ = stdout.write_all(rendered.as_bytes());

    if parsed.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_SYNTAX)
    }
}

/// How many failing files to name before summarising. A sweep over a corpus that
/// is broken in a thousand places should still fit on a screen.
const MAX_REPORTED: usize = 20;

/// Sweep every `.rb` file under a directory.
///
/// The two kinds of failure are counted apart on purpose. A syntax error is a
/// property of the corpus — ruby/spec ships files that are deliberately invalid
/// — while an unhandled node is a bug in `spinel-parse`. Only the second fails
/// the sweep, which is what makes this runnable in CI against a real corpus.
fn sweep(root: &Path) -> ExitCode {
    let started = Instant::now();

    let mut files = Vec::new();
    if let Err(err) = collect_ruby_files(root, &mut files) {
        eprintln!("spinel: cannot read `{}`: {err}", root.display());
        return ExitCode::from(EXIT_USAGE);
    }
    files.sort();

    let mut unhandled: Vec<String> = Vec::new();
    let mut syntax: Vec<String> = Vec::new();
    let mut unreadable = 0usize;

    for file in &files {
        let Ok(source) = std::fs::read(file) else {
            unreadable += 1;
            continue;
        };
        let parsed = spinel_parse::parse(&source);
        if parsed.is_ok() {
            continue;
        }
        let at = |d: &spinel_parse::Diagnostic| {
            let (line, column) = line_and_column(&source, d.span.start as usize);
            format!("{}:{line}:{column}: {}", file.display(), d.message)
        };
        if let Some(bug) = parsed.lowering_bugs().next() {
            unhandled.push(at(bug));
        }
        if let Some(error) = parsed.syntax_errors().next() {
            syntax.push(at(error));
        }
    }

    report("unhandled nodes", &unhandled);
    report("syntax errors", &syntax);

    println!(
        "{} files · {} unhandled · {} syntax errors · {:.1}s",
        files.len(),
        unhandled.len(),
        syntax.len(),
        started.elapsed().as_secs_f64(),
    );
    if unreadable > 0 {
        eprintln!("spinel: {unreadable} files could not be read and were skipped");
    }

    if unhandled.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_SYNTAX)
    }
}

fn report(heading: &str, lines: &[String]) {
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
    println!();
}

fn collect_ruby_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            // A directory that cannot be read is skipped rather than fatal: a
            // corpus with one unreadable subtree should still be swept.
            let _ = collect_ruby_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

fn use_color() -> bool {
    // The de facto standard, honoured before anything else.
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// One-based line and column of a byte offset. Columns count bytes, which is
/// what an editor's `:line:col` jump expects for a non-UTF-8 file.
fn line_and_column(source: &[u8], offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let line = source[..offset].iter().filter(|b| **b == b'\n').count() + 1;
    let line_start = source[..offset]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    (line, offset - line_start + 1)
}

/// `path:line:col: message`, then the source line with a caret under it. The
/// caret is why this is worth rendering rather than printing offsets: an offset
/// tells you where to look, a caret shows you.
fn render_diagnostic(
    path: &Path,
    source: &[u8],
    diagnostic: &spinel_parse::Diagnostic,
    severity: &str,
    color: bool,
) -> String {
    let start = diagnostic.span.start as usize;
    let (line, column) = line_and_column(source, start);

    let line_start = source[..start.min(source.len())]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let line_end = source[line_start..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(source.len(), |i| line_start + i);
    let text = String::from_utf8_lossy(&source[line_start..line_end]);

    let (bold, dim, reset) = if color {
        ("\x1b[1m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "")
    };

    let gutter = line.to_string();
    let pad = " ".repeat(gutter.len());
    // Tabs in the source would otherwise put the caret in the wrong column.
    let caret_pad: String = text
        .chars()
        .take(column - 1)
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    let width = diagnostic
        .span
        .end
        .saturating_sub(diagnostic.span.start)
        .max(1) as usize;
    let caret = "^".repeat(width.min(64));

    format!(
        "{bold}{}:{line}:{column}: {severity}: {}{reset}\n\
         {dim}{gutter} |{reset} {text}\n\
         {dim}{pad} |{reset} {caret_pad}{caret}",
        path.display(),
        diagnostic.message,
    )
}
