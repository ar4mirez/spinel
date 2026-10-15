//! The `spinel` binary.
//!
//! Phase 0 ships the skeleton: argument parsing, a version banner, and
//! `spinel parse`, which is the debugging window onto `spinel_ast` that every
//! later phase is built through. Subcommands land one issue at a time; see
//! `docs/cli.md` for the full surface.

mod ruby_args;
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

    /// Arguments after a bare file: the program's `ARGV`.
    #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a Ruby file.
    Run {
        /// A Ruby file.
        path: PathBuf,

        /// Arguments for the program: its `ARGV`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,

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
    // `spinel -e CODE`, `spinel -w script.rb`: a `ruby` command line, parsed
    // as `ruby` parses one (#62). `spinel`'s own `-h` and `-V` stay clap's.
    // The words as bytes, so a `-e` in another encoding arrives unmangled,
    // and as UTF-8 for everything else.
    let raw: Vec<Vec<u8>> = std::env::args_os()
        .skip(1)
        .map(std::os::unix::ffi::OsStringExt::into_vec)
        .collect();
    let words: Vec<String> = raw
        .iter()
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    if words
        .first()
        .is_some_and(|first| ruby_args::is_ruby_invocation(first))
    {
        return ruby_command(&words, &raw);
    }

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
            args,
            dump_bytecode,
        }) => return run_command(&path, &args, dump_bytecode),
        None => {}
    }

    if let Some(argument) = cli.file {
        // A Ruby file is the likeliest first thing a Ruby developer types; a
        // mistyped subcommand is the second. They need opposite answers, and
        // running a typo as a filename would report "no such file" about a word
        // that was never meant to be one.
        // A word a couple of keystrokes from a subcommand is taken for a typo
        // of it; anything else is a script, as `ruby` takes it, and a missing
        // one is `ruby`'s LoadError.
        if looks_like_a_ruby_file(&argument) || !looks_like_a_subcommand_typo(&argument) {
            let mut words = vec![argument];
            words.extend(cli.args);
            let raw: Vec<Vec<u8>> = words.iter().map(|w| w.clone().into_bytes()).collect();
            return ruby_command(&words, &raw);
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
fn looks_like_a_subcommand_typo(argument: &str) -> bool {
    ["run", "parse", "help"]
        .iter()
        .any(|name| edit_distance(argument, name) <= 2)
}

/// Levenshtein distance, over characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

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

fn run_command(path: &Path, args: &[String], dump_bytecode: bool) -> ExitCode {
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("spinel: cannot read `{}`: {err}", path.display());
            return ExitCode::from(EXIT_USAGE);
        }
    };
    run_program(path, &source, args, dump_bytecode, Around::default())
}

/// `spinel` given `ruby`'s flags: the prelude they mean, then the program
/// from `-e`, standard input or a file, as `ruby` would run it.
fn ruby_command(words: &[String], raw: &[Vec<u8>]) -> ExitCode {
    let parsed = ruby_args::parse(words).and_then(|mut args| {
        // `RUBYOPT`'s switches come after the command line's, and
        // `RUBYLIB`'s directories after every `-I`.
        if let Ok(value) = std::env::var("RUBYOPT") {
            ruby_args::apply_rubyopt(&mut args, &value)?;
        }
        if let Ok(value) = std::env::var("RUBYLIB") {
            args.library_path = value
                .split(':')
                .filter(|d| !d.is_empty())
                .map(str::to_owned)
                .collect();
        }
        Ok(args)
    });
    let mut args = match parsed {
        Ok(args) => args,
        Err(message) => {
            eprintln!("spinel: {message} (RuntimeError)");
            return ExitCode::from(EXIT_RAISED);
        }
    };
    for warning in &args.warnings {
        eprintln!("spinel: {warning}");
    }
    if args.help {
        eprintln!("Usage: spinel [switches] [--] [programfile] [arguments]");
        eprintln!("       spinel --help for Spinel's own commands");
        return ExitCode::SUCCESS;
    }
    if args.print_version {
        println!("{}", spinel_vm::description());
        if args.version_only {
            return ExitCode::SUCCESS;
        }
    }
    // `-x dir`, like `-C`, changes directory before the program is read.
    if let Some(Some(dir)) = &args.skip_to_ruby
        && let Err(err) = std::env::set_current_dir(dir)
    {
        eprintln!("spinel: Can't chdir to {dir} ({err}) (fatal)");
        return ExitCode::from(EXIT_RAISED);
    }
    // `-C` changes directory before the program is even read.
    if let Some(dir) = &args.chdir
        && let Err(err) = std::env::set_current_dir(dir)
    {
        eprintln!("spinel: Can't chdir to {dir} ({err}) (fatal)");
        return ExitCode::from(EXIT_RAISED);
    }
    let (name, mut source) = if args.code.is_some() {
        // Each `-e` line's own bytes, joined as `ruby` joins them.
        let mut code = Vec::new();
        for (index, &(word, at)) in args.code_at.iter().enumerate() {
            if index > 0 {
                code.push(b'\n');
            }
            code.extend_from_slice(raw.get(word).and_then(|w| w.get(at..)).unwrap_or_default());
        }
        ("-e".to_owned(), code)
    } else {
        let mut script = args.script.clone().unwrap_or_else(|| "-".to_owned());
        // `-S`: a bare name is looked for on `RUBYPATH`, then `PATH`.
        if args.search_path && !script.contains('/') && script != "-" {
            let dirs = ["RUBYPATH", "PATH"]
                .iter()
                .filter_map(|name| std::env::var(name).ok())
                .flat_map(|value| value.split(':').map(str::to_owned).collect::<Vec<_>>());
            for dir in dirs {
                let candidate = Path::new(if dir.is_empty() { "." } else { &dir }).join(&script);
                if candidate.is_file() {
                    script = candidate.to_string_lossy().into_owned();
                    break;
                }
            }
        }
        let read = if script == "-" {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes).map(|_| bytes)
        } else {
            std::fs::read(&script)
        };
        match read {
            Ok(bytes) => (script, bytes),
            Err(_) => {
                eprintln!("spinel: No such file or directory -- {script} (LoadError)");
                return ExitCode::from(EXIT_RAISED);
            }
        }
    };
    // A first line that is a `#!` for something other than ruby means the
    // program starts further down, as if `-x` had been given.
    if args.skip_to_ruby.is_none()
        && args.code.is_none()
        && source.starts_with(b"#!")
        && !source
            .split(|&b| b == b'\n')
            .next()
            .is_some_and(|line| line.windows(4).any(|w| w == b"ruby"))
    {
        args.skip_to_ruby = Some(None);
    }
    let source_before_skip = source.clone();
    let mut skipped = 0;
    if args.skip_to_ruby.is_some() {
        // `-x`: everything before the `#!` line naming ruby is not the program.
        let text = String::from_utf8_lossy(&source).into_owned();
        let mut offset = 0;
        let mut found = None;
        for line in text.split_inclusive('\n') {
            if line.starts_with("#!") && line.contains("ruby") {
                found = Some(offset);
                break;
            }
            offset += line.len();
        }
        let Some(start) = found else {
            eprintln!("spinel: no Ruby script found in input (LoadError)");
            return ExitCode::from(EXIT_RAISED);
        };
        source = source[start..].to_vec();
        skipped = start;
    }
    if args.syntax_only {
        let parsed = spinel_parse::parse_file(&name, &source);
        if parsed.is_ok() {
            println!("Syntax OK");
            return ExitCode::SUCCESS;
        }
        let color = use_color();
        for error in &parsed.errors {
            eprintln!(
                "{}",
                render_diagnostic(Path::new(&name), &source, error, "error", color)
            );
        }
        return ExitCode::from(EXIT_SYNTAX);
    }
    // `DATA` is the main script's file, read from just past `__END__`.
    let data = (args.code.is_none() && name != "-")
        .then(|| data_offset(&source_before_skip))
        .flatten()
        .map(|offset| (name.clone(), offset + skipped));
    // `-n` and `-p`: a program that parses on its own is put in the loop
    // after parsing, its `BEGIN`s outside it; one that does not — a bare
    // `next` at the top level is a syntax error without the loop — is
    // wrapped as text first.
    let mut around = ruby_args::loop_template(&args);
    let source = if around.is_some() && !spinel_parse::parse_file(&name, &source).is_ok() {
        around = None;
        ruby_args::wrap(&args, &source)
    } else {
        source
    };
    let prelude = ruby_args::prelude(&args);
    run_program(
        Path::new(&name),
        &source,
        &args.argv,
        false,
        Around {
            prelude: Some(&prelude),
            source_encoding: args.source_encoding.as_deref(),
            data,
            loop_template: around.as_deref(),
            frozen_string_literal: ruby_args::feature_enabled(
                &args,
                "frozen_string_literal",
                false,
            ),
        },
    )
}

/// What a `ruby` command line adds around a program; all of it absent for
/// `spinel run`.
#[derive(Default)]
struct Around<'a> {
    /// Ruby the flags mean, run first.
    prelude: Option<&'a str>,
    /// `-K`'s source encoding, for a script without a magic comment.
    source_encoding: Option<&'a str>,
    /// The file `DATA` reads and the offset it starts at.
    data: Option<(String, usize)>,
    /// The `-n`/`-p` loop to put the program in.
    loop_template: Option<&'a str>,
    /// `--enable=frozen-string-literal`.
    frozen_string_literal: bool,
}

/// Compile and run one program, after the prelude when there is one.
fn run_program(
    path: &Path,
    source: &[u8],
    args: &[String],
    dump_bytecode: bool,
    around: Around<'_>,
) -> ExitCode {
    let Around {
        prelude,
        source_encoding,
        data,
        loop_template,
        frozen_string_literal,
    } = around;
    let source = source.to_vec();
    if dump_bytecode {
        let parsed = spinel_parse::parse_file(&path.to_string_lossy(), &source);
        if !report_syntax_errors(path, &source, &parsed) {
            return ExitCode::from(EXIT_SYNTAX);
        }
        return match spinel_vm::compile::program(&parsed.program) {
            Ok(iseq) => {
                let mut stdout = std::io::stdout().lock();
                let _ = writeln!(stdout, "{iseq:#?}");
                ExitCode::SUCCESS
            }
            Err(unsupported) => cannot_run(path, &unsupported),
        };
    }

    let mut heap = spinel_vm::Heap::new();
    let mut scope = heap.scope();
    scope.bootstrap();
    scope.set_argv(&path.to_string_lossy(), args);
    scope.set_budget(None);
    spinel_core::boot(&mut scope);
    scope.set_frozen_string_literal(frozen_string_literal);

    // The flags' meaning runs first, as `ruby` runs `-r` before it reads the
    // program: an `at_exit` a required file registers runs even when the
    // program does not parse.
    let mut outcome = Ok(spinel_vm::Value::NIL);
    if let Some(prelude) = prelude.filter(|p| !p.is_empty()) {
        // Spinel's own Ruby, so failing to compile it is a bug, not the
        // program's.
        let parsed = spinel_parse::parse_file("<flags>", prelude.as_bytes());
        let compiled = spinel_vm::compile::program(&parsed.program)
            .expect("the prelude the flags make compiles");
        let mut frame = spinel_vm::interp::Frame::new(0);
        outcome = spinel_vm::interp::eval_in(&mut scope, &mut frame, &compiled)
            .map(|_| spinel_vm::Value::NIL);
    }
    let mut syntax_error = false;
    if outcome.is_ok() {
        let mut parsed = spinel_parse::parse_file(&path.to_string_lossy(), &source);
        // `-K`: the source encoding of a script with no magic comment.
        if let Some(encoding) = source_encoding
            && let Some(map) = std::sync::Arc::get_mut(&mut parsed.program.source)
            && map.encoding.is_none()
        {
            map.encoding = Some(encoding.into());
        }
        if let Some(template) = loop_template
            && parsed.is_ok()
        {
            loop_around(&mut parsed.program, template);
        }
        // `TOPLEVEL_BINDING` is the main script's own frame, so the call that
        // records it is the first thing the script does.
        let here = spinel_ast::Span { start: 0, end: 0 };
        let capture = |name: &str, args: Vec<spinel_ast::Expr>| {
            spinel_ast::Expr::new(
                here,
                spinel_ast::ExprKind::Call(Box::new(spinel_ast::Call {
                    receiver: None,
                    name: name.into(),
                    name_span: here,
                    args,
                    block: None,
                    flags: spinel_ast::CallFlags::default(),
                })),
            )
        };
        let binding = capture("binding", Vec::new());
        parsed
            .program
            .body
            .insert(0, capture("__toplevel_binding__", vec![binding]));
        if !report_syntax_errors(path, &source, &parsed) {
            syntax_error = true;
        } else {
            let iseq = match spinel_vm::compile::program_with(
                &parsed.program,
                "<main>",
                frozen_string_literal,
            ) {
                Ok(iseq) => iseq,
                Err(unsupported) => return cannot_run(path, &unsupported),
            };
            // The parser's warnings about the main script, through the same
            // `Warning.warn` a required file's go through.
            // The literal-in-condition warnings are not given for a program
            // from `-e`, as CRuby's `e_option_supplied` leaves them out.
            let from_e = path.as_os_str() == "-e";
            let warnings: String = parsed
                .warnings
                .iter()
                .filter(|warning| {
                    !(from_e
                        && (warning.message == "integer literal in flip-flop"
                            || warning.message.starts_with("string literal in ")
                            || warning.message.starts_with("regex literal in ")))
                })
                .map(|warning| {
                    let at = parsed.program.source.line(warning.span.start);
                    let text = format!("{}:{at}: warning: {}\n", path.display(), warning.message);
                    let verbose = spinel_parse::is_verbose_warning(&warning.message);
                    format!(
                        "Kernel.__send__(:__parse_warning__, {}, {verbose})\n",
                        ruby_args::ruby_string(&text)
                    )
                })
                .collect();
            if !warnings.is_empty() {
                let parsed = spinel_parse::parse_file("<warnings>", warnings.as_bytes());
                if let Ok(compiled) = spinel_vm::compile::program(&parsed.program) {
                    let mut frame = spinel_vm::interp::Frame::new(0);
                    outcome = spinel_vm::interp::eval_in(&mut scope, &mut frame, &compiled)
                        .map(|_| spinel_vm::Value::NIL);
                }
            }
            if let Some((file, offset)) = &data {
                let define = format!(
                    "DATA = File.open({file:?}, \"rb:\" + Encoding.default_external.name)\nDATA.seek({offset})\n"
                );
                let parsed = spinel_parse::parse_file("<data>", define.as_bytes());
                let compiled = spinel_vm::compile::program(&parsed.program)
                    .expect("the DATA definition compiles");
                let mut frame = spinel_vm::interp::Frame::new(0);
                outcome = spinel_vm::interp::eval_in(&mut scope, &mut frame, &compiled)
                    .map(|_| spinel_vm::Value::NIL);
            }
            if outcome.is_ok() {
                let mut frame = spinel_vm::interp::Frame::new(0);
                outcome = spinel_vm::interp::eval_in(&mut scope, &mut frame, &iseq)
                    .map(|_| spinel_vm::Value::NIL);
            }
        }
    }
    // The `at_exit` blocks run before the error is reported, as CRuby runs
    // them: one may `exit!`, and then nothing is printed.
    let failed = outcome.is_err() || syntax_error;
    let status = spinel_core::exit_status(&mut scope, failed);
    // `exit` is a `SystemExit` that nothing rescued: not an error to report,
    // and its status is the process's. A `SignalException` ends the process
    // by its signal, below, and CRuby prints nothing for one.
    let quiet = matches!(
        &outcome,
        Err(spinel_vm::Error::Uncaught { class, .. }) if class == "SystemExit" || class == "SignalException"
    );
    // An exception is reported by Ruby, as CRuby's top level reports it;
    // anything else is the engine saying it could not go on.
    if let Err(err) = &outcome
        && !quiet
    {
        if matches!(err, spinel_vm::Error::Uncaught { .. })
            && spinel_core::report_uncaught(&mut scope)
        {
            // The one hint Spinel adds to Ruby's report: a missing method may
            // be its own, while the core library is still growing.
            if matches!(err, spinel_vm::Error::Uncaught { class, .. } if class == "NoMethodError") {
                eprintln!(
                    "        Spinel's core library is still minimal, so this may be Spinel rather than your program.\n        What exists: {MILESTONES}"
                );
            }
        } else {
            eprintln!("{}: {}", path.display(), describe_error(err));
        }
    }
    // A `SignalException` that reached the top ends the process by its
    // signal, so whoever waits for it sees that, not an exit status.
    if let Err(spinel_vm::Error::Uncaught { class, message }) = &outcome {
        let name = match class.as_str() {
            "Interrupt" => Some("INT"),
            "SignalException" => Some(message.as_str()),
            _ => None,
        };
        if let Some(name) = name {
            spinel_vm::signal::die_by(name);
        }
    }
    let fallback = if syntax_error {
        EXIT_SYNTAX
    } else if outcome.is_ok() {
        0
    } else {
        EXIT_RAISED
    };
    ExitCode::from(status.unwrap_or(fallback))
}

/// Put a program's statements in the `-n`/`-p` loop `template` holds, its
/// `BEGIN`s first and outside it, where they run once.
fn loop_around(program: &mut spinel_ast::Program, template: &str) {
    let parsed = spinel_parse::parse_file("<loop>", template.as_bytes());
    let Some(mut looped) = parsed.program.body.into_iter().next() else {
        return;
    };
    let body = std::mem::take(&mut program.body);
    let (begins, rest): (Vec<_>, Vec<_>) = body.into_iter().partition(|expr| {
        matches!(&expr.kind, spinel_ast::ExprKind::Exec(exec) if exec.kind == spinel_ast::ExecKind::Pre)
    });
    if let spinel_ast::ExprKind::While(node) = &mut looped.kind {
        let mut statements = Vec::with_capacity(node.body.len() + rest.len());
        let mut rest = Some(rest);
        for statement in node.body.drain(..) {
            let placeholder = matches!(&statement.kind, spinel_ast::ExprKind::Call(call) if call.name.as_ref() == "__spinel_body__");
            if placeholder {
                statements.extend(rest.take().unwrap_or_default());
            } else {
                statements.push(statement);
            }
        }
        node.body = statements;
    }
    program.body = begins;
    program.body.push(looped);
}

/// Where a script's data starts: just past its `__END__` line, the newline
/// included when there is one.
/// ponytail: a line, not a token, so an `__END__` line inside a heredoc counts;
/// Prism knows the real one and `spinel_parse` does not hand it over yet.
fn data_offset(source: &[u8]) -> Option<usize> {
    let mut start = 0;
    for line in source.split_inclusive(|&b| b == b'\n') {
        let bare = line.strip_suffix(b"\n").unwrap_or(line);
        let bare = bare.strip_suffix(b"\r").unwrap_or(bare);
        if bare == b"__END__" {
            return Some(start + line.len());
        }
        start += line.len();
    }
    None
}

/// Print a parse's errors, `ruby`'s one-line summary first; true when there
/// were none.
fn report_syntax_errors(path: &Path, source: &[u8], parsed: &spinel_parse::Parsed) -> bool {
    if parsed.is_ok() {
        return true;
    }
    let color = use_color();
    if let Some(first) = parsed.errors.first() {
        let line = render_diagnostic(path, source, first, "error", false);
        let at = line.split(": error:").next().unwrap_or("").to_owned();
        eprintln!("{}: {at}: syntax error found (SyntaxError)", path.display());
    }
    for error in &parsed.errors {
        eprintln!("{}", render_diagnostic(path, source, error, "error", color));
    }
    false
}

/// A construct the compiler does not implement yet is not a syntax error
/// and must not be reported as one: the file is valid Ruby and Spinel is the
/// thing that is unfinished. Saying which construct is what makes the next
/// slice choosable.
fn cannot_run(path: &Path, unsupported: &impl std::fmt::Display) -> ExitCode {
    eprintln!("spinel: cannot run `{}` yet: {unsupported}", path.display());
    eprintln!("        Progress: {MILESTONES}");
    ExitCode::from(EXIT_USAGE)
}

/// One line for an error that ended the program.
///
/// A raise is Ruby's own `message (Class)`. Everything else is the VM saying it
/// cannot run the program — an unfinished construct, a question it declines to
/// guess at — and those read as engine limits, with the milestone to follow.
fn describe_error(err: &spinel_vm::Error) -> String {
    let raised = match err {
        // The exception object reached the top with no handler. This is the
        // ordinary end of a Ruby program that raised, and reads as Ruby's does.
        spinel_vm::Error::Uncaught { class, message } => Some((class.as_str(), message.as_str())),
        // The VM decided to raise and the unwinder never got to build an
        // object — an arity error at the outermost frame, say. Same shape.
        spinel_vm::Error::Raise { class, message } => Some((*class, message.as_str())),
        // Everything else is the engine saying it cannot run the program
        // rather than the program failing, so it points at the milestones.
        _ => None,
    };
    let Some((class, message)) = raised else {
        return format!("{err} — see {MILESTONES}");
    };
    let line = format!("{message} ({class})");
    // Since #170 a missing method is an ordinary rescuable raise, so an
    // uncaught one is indistinguishable from a program's own `NoMethodError`.
    // The hint the issue asked to keep lives here, on the outcome rather than
    // on an error variant: it says *may* be Spinel, which is the truth now.
    if class == "NoMethodError" {
        return format!(
            "{line}\n\
             \x20       Spinel's core library is still minimal, so this may be Spinel \
             rather than your program.\n\
             \x20       What exists: {MILESTONES}"
        );
    }
    line
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

    let parsed = spinel_parse::parse_file(&path.to_string_lossy(), &source);
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
        let parsed = spinel_parse::parse_file(&file.to_string_lossy(), &source);
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
