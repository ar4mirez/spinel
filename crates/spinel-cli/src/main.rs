//! The `spinel` binary.
//!
//! Phase 0 ships the skeleton: argument parsing, a version banner, and
//! `spinel parse`, which is the debugging window onto `spinel_ast` that every
//! later phase is built through. Subcommands land one issue at a time; see
//! `docs/cli.md` for the full surface.

mod add;
mod app;
mod build;
mod builtin_gem;
mod compact_index;
mod credentials;
mod did_you_mean;
mod gem_home;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "`=~` and the Array constructor; only tests call them until the lockfile reads platforms"
    )
)]
mod gem_platform;
mod gem_requirement;
mod gem_spec;
mod gem_store;
mod gem_substitute;
mod gem_version;
mod gemfile_edit;
mod git_source;
mod home;
mod init;
mod install;
mod local_source;
mod locked;
mod lockfile;
mod macho;
mod marshal;
mod remove;
mod resolver;
mod ruby_args;
mod ruby_version;
mod stdlib;
mod target;
mod test_runner;
mod test_watch;
mod trailer;
mod tree;
mod update;
mod watch;
mod x;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};

const MILESTONES: &str = "https://github.com/ar4mirez/spinel/milestones";

/// Shown under the options list: what this build runs, and where to read what
/// works today, which the README keeps current rather than this text.
const AFTER_HELP: &str = "\
Spinel runs Ruby 4.0 programs; see https://github.com/ar4mirez/spinel for what
works today.

Progress: https://github.com/ar4mirez/spinel/milestones";

#[derive(Parser, Debug)]
#[command(
    name = "spinel",
    about = "A Ruby engine and toolchain, in one binary.",
    disable_version_flag = true,
    arg_required_else_help = true,
    // `spinel app.rb x`: once a file is named, the words after it are its
    // `ARGV`, never a subcommand.
    args_conflicts_with_subcommands = true,
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

        /// Fail when the project's `.spinel-version`, `.ruby-version` or
        /// Gemfile `ruby` directive asks for a Spinel or Ruby this build does
        /// not implement.
        ///
        /// Without it the mismatch is a warning and the program runs.
        #[arg(long)]
        strict_ruby_version: bool,

        /// Run it again whenever a file in the current directory changes.
        ///
        /// Paths `.gitignore` excludes do not count, and a burst of changes
        /// within 100 ms restarts it once. The program runs as a fresh process
        /// each time; nothing is reloaded in place.
        #[arg(long)]
        watch: bool,
    },

    /// Run the project's tests in parallel workers.
    ///
    /// Finds `test/**/*_test.rb` under the current directory, requires
    /// `test/test_helper.rb` once, then forks the workers, so what the helper
    /// boots is booted once. Each worker is handed one file at a time, the
    /// longest first by the times the last run kept in
    /// `.spinel/test_timings.json`. A worker that dies while running a file
    /// is reported with that file.
    /// `spec/**/*_spec.rb` files run after them the same way, `rspec/core`
    /// and `spec/spec_helper.rb` required once, each worker running its
    /// files' examples through RSpec's own runner. Prints a dot for each
    /// test as it passes, then each failure with its `file:line` and
    /// backtrace, gem frames folded, a summary line, and a `spinel test
    /// file:line` command to run each failure again. Exits 1 when anything
    /// failed. `--reporter json` prints one JSON object per event instead,
    /// for tools, and `--reporter junit` JUnit XML, for CI.
    /// `--watch` runs them again as the project changes.
    Test {
        /// How many workers to fork; the physical cores when not given.
        #[arg(short, long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..))]
        jobs: Option<u16>,
        /// What stdout gets; with `json` or `junit` it gets only that, and
        /// what tests print goes to stderr.
        #[arg(long, value_enum, default_value_t = test_runner::Reporter::Progress)]
        reporter: test_runner::Reporter,
        /// Files to run instead of every test; `FILE:LINE` runs only the
        /// test or example defined at or nearest above LINE, and a
        /// directory every `*_test.rb` and `*_spec.rb` under it.
        #[arg(value_name = "FILE[:LINE]|DIR")]
        paths: Vec<String>,
        /// Run the tests again whenever a file in the current directory
        /// changes.
        ///
        /// A changed test or spec file runs itself; a changed
        /// `app/**/NAME.rb` or `lib/**/NAME.rb` runs `test/**/NAME_test.rb`
        /// and `spec/**/NAME_spec.rb`; any other change runs what failed
        /// last. Enter runs every test, whatever paths were given. Paths
        /// `.gitignore` excludes do not count, nor does `.spinel/`.
        #[arg(long)]
        watch: bool,
        /// Where `--watch` has each run record what failed.
        #[arg(long, hide = true, value_name = "FILE")]
        record_failures: Option<PathBuf>,
    },

    /// Install the Gemfile's gems into the project's `.spinel/gem_home/`.
    ///
    /// Resolves the Gemfile, keeping to `Gemfile.lock` as Bundler does when
    /// no update is asked for, fetches each gem into the global store once
    /// for every project, links them into the project as `gem install` lays
    /// out a GEM_HOME, points `.bundle/config` at it, and writes the
    /// lockfile if it changed. Prints `+ gem version` for each gem added and
    /// `- gem version` for each one removed.
    Install {
        /// Fail, and write nothing, when the lockfile would change: Bundler's
        /// frozen mode, for CI. Exits 16 as Bundler does.
        #[arg(long)]
        frozen: bool,
    },

    /// Add gems to the Gemfile and install them.
    ///
    /// Writes a `gem` line for each, as `bundle add` writes it: at the end
    /// of the Gemfile, or at the end of a `group` block naming exactly the
    /// groups given, with the requirement `~>` the version resolved (`~> 4.2`
    /// for 4.2.1) unless one is given after `@`. Then installs, as `spinel
    /// install` does. Nothing is written when the gems do not resolve or
    /// install: the Gemfile and the lockfile stay as they were. A gem the
    /// Gemfile already lists with the same requirement is not added again.
    Add {
        /// Each gem, with an optional version requirement after `@`
        /// (`sinatra`, `sinatra@4.2`, `sinatra@~> 4.1, >= 4.1.1`).
        #[arg(value_name = "GEM[@REQ]", required = true)]
        gems: Vec<String>,

        /// The groups to add them to, separated by commas.
        #[arg(short, long, value_name = "GROUPS")]
        group: Option<String>,

        /// The git repository to take them from, written as `git:` on each
        /// line.
        #[arg(long, value_name = "URL")]
        git: Option<String>,
    },

    /// Remove gems from the Gemfile and install what is left.
    ///
    /// Takes each gem's `gem` lines out of the Gemfile, as `bundle remove`
    /// does, with a `group` block left empty; every other byte of the file
    /// stays. Then installs, as `spinel install` does, keeping every other
    /// locked version, so only the gems removed and those nothing else needs
    /// leave the lockfile. Nothing is written when that fails.
    Remove {
        /// The gems to remove.
        #[arg(value_name = "GEM")]
        gems: Vec<String>,
    },

    /// Update the gems named, or every gem, to the newest versions allowed.
    ///
    /// As `bundle update` does: each gem named is unlocked, with every
    /// locked gem it depends on, and moved to the newest version the Gemfile
    /// allows; every other gem keeps its locked version. With no gems, every
    /// gem is unlocked. No gem goes below its locked version unless a
    /// conflict takes it there. Then installs, as `spinel install` does.
    /// Nothing is written when that fails: the lockfile stays as it was.
    Update {
        /// The gems to update; every gem when none is named.
        #[arg(value_name = "GEM")]
        gems: Vec<String>,

        /// Update every gem, as no gems named does.
        #[arg(long)]
        all: bool,

        /// Not supported yet.
        #[arg(long, hide = true)]
        conservative: bool,

        /// Not supported yet.
        #[arg(long, hide = true)]
        patch: bool,

        /// Not supported yet.
        #[arg(long, hide = true)]
        minor: bool,

        /// Not supported yet.
        #[arg(long, hide = true)]
        major: bool,

        /// Not supported yet.
        #[arg(short, long, hide = true, value_name = "GROUP", num_args = 1..)]
        group: Vec<String>,
    },

    /// Run a gem's executable without installing it into the project.
    ///
    /// Resolves the gem afresh, as `gem exec` does, fetches it and its
    /// dependencies into the global store, links them into a GEM_HOME of
    /// their own under `~/.spinel/x/`, named by what was resolved so a
    /// second run reuses it, and runs the executable there with the
    /// arguments that follow. The current directory's Gemfile, lockfile and
    /// `.spinel/` are neither read nor written. Exits with the command's
    /// status.
    X {
        /// The source to resolve against and fetch from.
        #[arg(long, default_value = compact_index::DEFAULT_SOURCE)]
        source: String,

        /// The gem to run the executable from, when it is named otherwise:
        /// `spinel x --gem rspec-core rspec`. The requirement after `@` is
        /// then this gem's.
        #[arg(short = 'g', long, value_name = "GEM")]
        gem: Option<String>,

        /// The gem, and the executable to run from it, with an optional
        /// version requirement (`rubocop`, `rubocop@1.60.0`,
        /// `rubocop@~> 1.60`), then the executable's arguments, its `ARGV`.
        /// Everything after the gem is the executable's, `--help` included.
        #[arg(
            value_name = "GEM[@REQ] [ARGS]",
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        command: Vec<String>,
    },

    /// Scaffold a project in the current directory.
    ///
    /// Writes a Gemfile, a `.spinel-version` naming this Spinel, a `test/`
    /// directory and `.gitignore` entries for `.spinel/` and `.bundle/`.
    /// Anything already there is kept: nothing is overwritten.
    Init,

    /// Bring the compact index cache up to date, and print what each request
    /// did.
    ///
    /// The debugging window onto the index client the resolver reads through
    /// (docs/package-manager.md, "Index client"): `/versions` always, `/names`
    /// with `--names`, and `/info/<gem>` for each gem named. Hidden: `spinel
    /// install` is the command that uses the index.
    #[command(hide = true)]
    Index {
        /// The compact index to read.
        #[arg(long, default_value = compact_index::DEFAULT_SOURCE)]
        source: String,

        /// Also update `/names`.
        #[arg(long)]
        names: bool,

        /// Gems whose `/info` to update.
        gems: Vec<String>,
    },

    /// Fetch gems into the global store, and print what each one needed.
    ///
    /// The debugging window onto the store `spinel install` links
    /// projects to (docs/package-manager.md, "Global store"): each gem's
    /// checksum is read from the source's compact index, which is brought up
    /// to date first, and a gem already stored with that checksum is not
    /// fetched again. Hidden: `spinel install` is the command that uses the
    /// store.
    #[command(hide = true)]
    Fetch {
        /// The source to fetch from, and whose index lists the checksums.
        #[arg(long, default_value = compact_index::DEFAULT_SOURCE)]
        source: String,

        /// Then link them into this GEM_HOME, laid out as `gem install`
        /// lays one out, and remove what an earlier link made for gems not
        /// named.
        #[arg(long, value_name = "DIR")]
        gem_home: Option<PathBuf>,

        /// Gems, each `name@version` or `name@version-platform`, the version
        /// as the gem's `/info` line spells it.
        #[arg(required = true)]
        gems: Vec<String>,
    },

    /// Resolve a Gemfile's dependencies against its source's compact index,
    /// and print the gems chosen.
    ///
    /// The debugging window onto the resolver `spinel install` runs
    /// (docs/package-manager.md, "Resolver"): it reads the dependency JSON
    /// `shims/gemfile_eval.rb` prints, brings the index up to date for every
    /// gem those dependencies can reach, and prints one line per gem as a
    /// lockfile's `specs:` lists it, or Bundler's explanation of why there
    /// is no resolution. Hidden: `spinel install` is the command that
    /// resolves.
    #[command(hide = true)]
    Resolve {
        /// The dependency JSON, or `-` for standard input.
        deps: PathBuf,

        /// A platform to resolve for, as a lockfile's `PLATFORMS` names it;
        /// repeat for each, the local one first. The local platform when
        /// none is given.
        #[arg(long = "platform", value_name = "PLATFORM")]
        platforms: Vec<String>,

        /// Print the `Gemfile.lock` Bundler would write for the resolution,
        /// as a new lockfile (docs/package-manager.md, "Lockfile"), instead
        /// of the gems chosen.
        #[arg(long)]
        lockfile: bool,

        /// The `Gemfile.lock` the Gemfile was locked to, whose versions are
        /// kept as Bundler keeps them when no update is asked for
        /// (docs/package-manager.md, "Locked preferences"). Its `PLATFORMS`
        /// are the resolution platforms; the first `--platform` is the local
        /// one, and each other is added as `bundle lock --add-platform`
        /// adds it. When nothing changed, the index is not read.
        #[arg(long, value_name = "LOCKFILE")]
        locked: Option<PathBuf>,

        /// With `--locked`, update as `bundle lock --update` does (#87):
        /// `--update` unlocks every gem, `--update=GEM,...` the gems named
        /// and the locked gems they reach.
        #[arg(
            long,
            value_name = "GEM",
            num_args = 0..=1,
            require_equals = true,
            value_delimiter = ',',
            requires = "locked"
        )]
        update: Option<Vec<String>>,

        /// The Ruby to resolve for, which `required_ruby_version` is checked
        /// against. Spinel's `RUBY_VERSION` when not given.
        #[arg(long, value_name = "VERSION")]
        ruby_version: Option<String>,

        /// The RubyGems to resolve for, which `required_rubygems_version` is
        /// checked against. The RubyGems Spinel ships when not given.
        #[arg(long, value_name = "VERSION")]
        rubygems_version: Option<String>,
    },

    /// Read a `Gemfile.lock` and write it back out as Bundler would.
    ///
    /// The debugging window onto the lockfile reader and writer
    /// (docs/package-manager.md, "Lockfile"): a lockfile Bundler wrote comes
    /// back byte for byte. Hidden: `spinel install` is the command that
    /// reads and writes lockfiles.
    #[command(hide = true)]
    Lockfile {
        /// The lockfile, or `-` for standard input.
        path: PathBuf,
    },

    /// Write a require map of the standard library and the directories
    /// given.
    ///
    /// The debugging window onto the map `spinel install` writes for the
    /// installed gems' `require_paths` (docs/cli.md, "Bytecode cache and
    /// require map"), for a GEM_HOME made some other way, and for
    /// `bench/require_map.rb`. Hidden: `spinel install` is the command that
    /// writes the map.
    #[command(hide = true)]
    RequireMap {
        /// Where to write it: a project's `.spinel/require_map.bin` is the
        /// one its programs read.
        out: PathBuf,

        /// Load path directories to list, besides the standard library.
        dirs: Vec<PathBuf>,
    },

    /// Build a single executable: this `spinel` with a project packed in.
    ///
    /// The payload (docs/build.md) holds the project's files as `.gitignore`
    /// keeps them, the gems of the lockfile's `default` group and of each
    /// `--group`, a warmed bytecode cache, and a manifest naming the entry
    /// point. Arguments after `--` are the ones the app's `ARGV` starts with.
    Build {
        /// The program the app runs, a file in the project.
        #[arg(long, value_name = "ENTRY")]
        compile: PathBuf,

        /// Where to write the app.
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,

        /// Take files `.gitignore` leaves out: a pattern as `.gitignore`
        /// writes one, relative to the project. Repeatable.
        #[arg(long, value_name = "PATTERN")]
        include: Vec<String>,

        /// Leave files out, whatever `.gitignore` and `--include` say: a
        /// pattern as `.gitignore` writes one. Repeatable.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,

        /// A Gemfile group whose gems go in besides `default`'s. Repeatable.
        #[arg(long, value_name = "GROUP")]
        group: Vec<String>,

        /// A variable set in the app's environment. Repeatable.
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// Write the app for another platform: x86_64-linux, aarch64-linux,
        /// x86_64-macos or aarch64-macos. The `spinel` release binary of
        /// this version for it is downloaded once, into
        /// `~/.spinel/targets/`, and the payload written into it.
        #[arg(long, value_name = "TRIPLE")]
        target: Option<String>,

        /// The arguments the app's `ARGV` starts with.
        #[arg(last = true, value_name = "ARGS")]
        argv: Vec<String>,
    },

    /// Write a compiled app: a `spinel` with a payload and its trailer.
    ///
    /// The debugging window onto the writer `spinel build --compile` packs
    /// its payload with (docs/build.md, "Format"): on Linux the payload and
    /// trailer are appended, and on macOS they go in the `__SPINEL,__payload`
    /// section and the result is signed again ad hoc with `codesign`. Hidden:
    /// `spinel build` is the command that makes apps.
    #[command(hide = true)]
    Pack {
        /// The payload, any bytes, written as they are.
        payload: PathBuf,

        /// Where to write the app.
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
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
    // A compiled app is this binary with a payload and trailer appended
    // (docs/build.md, "Format"). Checked before anything reads the command
    // line, because a compiled app's command line belongs to the app.
    if let Some(code) = compiled_app() {
        return code;
    }

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
        return ruby_command(&words, &raw, false);
    }
    // `spinel run -e CODE`, `spinel run -w app.rb`: `ruby`'s command line,
    // with the project's gems visible as `spinel run` makes them. `run`'s
    // own flags stay clap's.
    if words.first().is_some_and(|w| w == "run")
        && words.get(1).is_some_and(|second| {
            ruby_args::is_ruby_invocation(second)
                && !matches!(
                    second.split('=').next().unwrap_or_default(),
                    "--watch" | "--dump-bytecode" | "--strict-ruby-version" | "--"
                )
        })
    {
        project_ruby_version_ok(false);
        return ruby_command(&words[1..], &raw[1..], true);
    }

    // `arg_required_else_help` means a bare `spinel` never reaches this point: clap
    // prints help and exits non-zero, so a typo is never a silent success.
    // Every word after the program's file is its `ARGV`, `--version`, `-h`
    // and `--` included, as `ruby show.rb --help` gives `["--help"]` (#895).
    // Clap reads only the words before them, so none can be taken for
    // `spinel`'s or `run`'s own flags; the program gets the rest as they are.
    let cli = match program_args_start(&words) {
        Some(start) => {
            // `start + 1`: the program name comes first in `args_os`.
            let mut cli = Cli::parse_from(std::env::args_os().take(start + 1));
            let program_args = words[start..].to_vec();
            match &mut cli.command {
                Some(Command::Run { args, .. }) => *args = program_args,
                _ => cli.args = program_args,
            }
            cli
        }
        None => Cli::parse(),
    };

    if cli.version {
        println!("{}", spinel_vm::description());
        return ExitCode::SUCCESS;
    }

    match cli.command {
        Some(Command::Parse { path, format }) => return parse_command(&path, format),
        Some(Command::Init) => return init_command(),
        Some(Command::Install { frozen }) => return install::run(frozen),
        Some(Command::Test {
            jobs,
            reporter,
            paths,
            watch,
            record_failures,
        }) => {
            // Each run under `--watch` reads the project's Ruby version
            // itself, so each reports it afresh.
            if watch {
                return test_watch::run(jobs, reporter, &paths);
            }
            project_ruby_version_ok(false);
            return test_runner::run(
                jobs.map(usize::from),
                reporter,
                record_failures.as_deref(),
                &paths,
            );
        }
        Some(Command::Add { gems, group, git }) => {
            return add::run(&add::Request {
                gems: &gems,
                group: group.as_deref(),
                git: git.as_deref(),
            });
        }
        Some(Command::Remove { gems }) => return remove::run(&gems),
        Some(Command::Update {
            gems,
            all,
            conservative,
            patch,
            minor,
            major,
            group,
        }) => {
            let unsupported = [
                (conservative, "--conservative"),
                (patch, "--patch"),
                (minor, "--minor"),
                (major, "--major"),
                (!group.is_empty(), "--group"),
            ]
            .into_iter()
            .filter_map(|(given, option)| given.then_some(option))
            .collect();
            return update::run(&gems, all, &update::Unsupported(unsupported));
        }
        Some(Command::X {
            source,
            gem,
            command,
        }) => {
            // `required` holds one word at least.
            let (command, args) = command.split_first().expect("clap requires the gem");
            return x::run(&x::Request {
                source: &source,
                gem: gem.as_deref(),
                command,
                args,
            });
        }
        Some(Command::Index {
            source,
            names,
            gems,
        }) => return index_command(&source, names, &gems),
        Some(Command::Fetch {
            source,
            gem_home,
            gems,
        }) => return fetch_command(&source, gem_home.as_deref(), &gems),
        Some(Command::Resolve {
            deps,
            platforms,
            lockfile,
            locked,
            update,
            ruby_version,
            rubygems_version,
        }) => {
            let unlock = match update {
                None => locked::Unlock::Nothing,
                Some(gems) if gems.is_empty() => locked::Unlock::All,
                Some(gems) => locked::Unlock::Gems(gems),
            };
            return resolve_command(
                &deps,
                &platforms,
                lockfile,
                locked.as_deref(),
                &unlock,
                ruby_version.as_deref(),
                rubygems_version.as_deref(),
            );
        }
        Some(Command::Lockfile { path }) => return lockfile_command(&path),
        Some(Command::Pack { payload, output }) => return pack_command(&payload, &output),
        Some(Command::Build {
            compile,
            output,
            include,
            exclude,
            group,
            env,
            target,
            argv,
        }) => {
            return build::run(&build::Request {
                entry: &compile,
                output: &output,
                include: &include,
                exclude: &exclude,
                groups: &group,
                env: &env,
                target: target.as_deref(),
                argv: &argv,
            });
        }
        Some(Command::RequireMap { out, dirs }) => {
            let mut roots = dirs;
            roots.extend(stdlib::directory());
            return match spinel_vm::require_map::write(&out, &roots) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("spinel: require-map: {}: {err}", out.display());
                    ExitCode::FAILURE
                }
            };
        }
        Some(Command::Run {
            path,
            args,
            dump_bytecode,
            strict_ruby_version,
            watch,
        }) => {
            if watch {
                return watch_run(&path, &args, dump_bytecode, strict_ruby_version);
            }
            if !project_ruby_version_ok(strict_ruby_version) {
                return ExitCode::FAILURE;
            }
            return run_command(&path, &args, dump_bytecode);
        }
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
            // The same as `spinel run`, which reads the project's Ruby.
            project_ruby_version_ok(false);
            let mut words = vec![argument];
            words.extend(cli.args);
            let raw: Vec<Vec<u8>> = words.iter().map(|w| w.clone().into_bytes()).collect();
            return ruby_command(&words, &raw, true);
        }
        eprintln!("spinel: unknown subcommand `{argument}`.");
        eprintln!(
            "        This build has: run, init, install, add, remove, update, x, test, build, parse. Try `spinel --help`."
        );
        return ExitCode::from(EXIT_USAGE);
    }

    ExitCode::SUCCESS
}

/// What this executable does when it is a compiled app: `None` when it is
/// a plain `spinel`, which costs one seek and one 48-byte read of its own file,
/// after reading its load commands on macOS to find the payload section.
/// A compiled app runs its entry point (`app.rs`), hashing its payload only
/// when it first extracts it.
fn compiled_app() -> Option<ExitCode> {
    // An executable that cannot read itself has no payload it could run.
    let mut file = trailer::open_self().ok()?;
    let found = trailer::find(&mut file);
    if let Ok(None) = found {
        return None;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("spinel"));
    match found {
        Ok(None) => None,
        Ok(Some(found)) => Some(app::run(&mut file, found, &exe)),
        Err(err) => {
            eprintln!("spinel: `{}`: {err}", exe.display());
            Some(ExitCode::FAILURE)
        }
    }
}

/// `spinel pack`: write `payload` into a copy of this `spinel` at `output`.
fn pack_command(payload: &Path, output: &Path) -> ExitCode {
    let binary = match std::env::current_exe() {
        Ok(binary) => binary,
        Err(err) => {
            eprintln!("spinel: pack: cannot find this executable: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut payload = match trailer::Payload::from_file(payload) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("spinel: pack: {}: {err}", payload.display());
            return ExitCode::FAILURE;
        }
    };
    match trailer::write(&binary, &mut payload, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("spinel: pack: {}: {err}", output.display());
            ExitCode::FAILURE
        }
    }
}

/// Where the program's `ARGV` starts in `spinel`'s words (the program name
/// left out), when they name a program file: the word after the file in
/// `spinel <file>`, and in `spinel run [--watch|--dump-bytecode|
/// --strict-ruby-version]... [--] <file>`. `None` for every other command
/// line, which clap reads whole, as it does `run -h` before the file.
fn program_args_start(words: &[String]) -> Option<usize> {
    use clap::CommandFactory;
    let first = words.first()?;
    if first == "run" {
        for (i, word) in words.iter().enumerate().skip(1) {
            match word.as_str() {
                // After `--`, the next word is the file even if named like a
                // flag, as `watch_run`'s child is given it.
                "--" => return (i + 1 < words.len()).then_some(i + 2),
                "--watch" | "--dump-bytecode" | "--strict-ruby-version" => {}
                flag if flag.starts_with('-') => return None,
                _ => return Some(i + 1),
            }
        }
        return None;
    }
    // A subcommand's words are its own; a first word starting with `-` is
    // `spinel`'s own flag, as `ruby`'s were handled before this.
    let subcommand = first == "help" || Cli::command().find_subcommand(first).is_some();
    (!first.starts_with('-') && !subcommand).then_some(1)
}

/// Whether a bare argument is a Ruby file rather than a mistyped subcommand.
/// Named `.rb`, or on disk under any name — `spinel Rakefile` should be read as
/// a file, and `spinel pasre` should not.
fn looks_like_a_subcommand_typo(argument: &str) -> bool {
    ["run", "init", "install", "build", "parse", "help"]
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
// spinel init
// ---------------------------------------------------------------------------

fn init_command() -> ExitCode {
    let dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("spinel: cannot read the current directory: {err}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match init::run(&dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("spinel: init failed in `{}`: {err}", dir.display());
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// spinel fetch
// ---------------------------------------------------------------------------

/// Fetch each gem into the store under the Spinel home and print the store's
/// directory, then one line per gem: `rack-3.1.8: fetched 89088 bytes`, or
/// `rack-3.1.8: in the store`. The checksum each is verified against is its
/// `/info` line's, after the index is brought up to date. With a `gem_home`,
/// and every gem stored, then link them into it. Exits 1 if any gem failed,
/// after reporting every one.
fn fetch_command(source: &str, gem_home: Option<&Path>, specs: &[String]) -> ExitCode {
    let mut gems = Vec::new();
    for spec in specs {
        match gem_store::GemId::parse(spec) {
            Ok(gem) => gems.push(gem),
            Err(err) => {
                eprintln!("spinel: fetch: {err}");
                return ExitCode::from(EXIT_USAGE);
            }
        }
    }
    let Some(home) = home::spinel_home() else {
        eprintln!("spinel: fetch: neither SPINEL_HOME nor HOME is set");
        return ExitCode::FAILURE;
    };
    let client = match compact_index::Client::new(source, &home) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("spinel: fetch: {err}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("spinel: fetch: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let mut ok = true;
        if let Err(err) = client.versions().await {
            eprintln!("spinel: fetch: {err}");
            return ExitCode::FAILURE;
        }
        let names: Vec<String> = gems.iter().map(|g| g.name.clone()).collect();
        match client.info(&names).await {
            Ok(results) => {
                for (_, result) in results {
                    if let Err(err) = result {
                        eprintln!("spinel: fetch: {err}");
                        ok = false;
                    }
                }
            }
            Err(err) => {
                eprintln!("spinel: fetch: {err}");
                return ExitCode::FAILURE;
            }
        }
        let mut wants = Vec::new();
        for gem in gems {
            let info = std::fs::read(client.info_path(&gem.name)).unwrap_or_default();
            match compact_index::version_checksum(&info, &gem.index_version()) {
                Some(Some(sha256)) => wants.push((gem, sha256)),
                Some(None) => {
                    // ponytail: a gem the index gives no checksum for is
                    // refused, since the store is keyed by it. Upgrade: take
                    // the lockfile's CHECKSUMS entry (#80) when there is one.
                    eprintln!(
                        "spinel: fetch: {}: {source} lists no checksum for it",
                        gem.full_name()
                    );
                    ok = false;
                }
                None => {
                    eprintln!(
                        "spinel: fetch: {}: {source} does not list it",
                        gem.full_name()
                    );
                    ok = false;
                }
            }
        }
        let store = gem_store::Store::new(&home);
        println!("store: {}", store.root().display());
        let mut stored = Vec::new();
        for (gem, result) in store.fetch(&client, &wants).await {
            match result {
                Ok(fetched) => {
                    stored.push((gem.clone(), fetched.path.clone()));
                    let entry = fetched
                        .path
                        .strip_prefix(store.root())
                        .unwrap_or(&fetched.path);
                    println!(
                        "{}: {} ({})",
                        gem.full_name(),
                        fetched.outcome,
                        entry.display()
                    );
                    for warning in fetched.warnings {
                        eprintln!("spinel: fetch: warning: {warning}");
                    }
                }
                Err(err) => {
                    eprintln!("spinel: fetch: {err}");
                    ok = false;
                }
            }
        }
        // A GEM_HOME gets every gem or none of them: one that failed to
        // fetch leaves it as it was.
        if let Some(gem_home) = gem_home.filter(|_| ok) {
            ok = link_gem_home(gem_home, &stored);
        }
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    })
}

/// Link stored gems into `gem_home` and print it, then one line per gem
/// (`linked` or `current`) and per gem removed.
fn link_gem_home(gem_home: &Path, stored: &[(gem_store::GemId, PathBuf)]) -> bool {
    // Symlinks name the store by an absolute path, whatever SPINEL_HOME is.
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_owned());
    let stored: Vec<_> = stored
        .iter()
        .map(|(gem, entry)| (gem.clone(), absolute(entry)))
        .collect();
    let gem_home = absolute(gem_home);
    match gem_home::link(&gem_home, &stored) {
        Ok(report) => {
            println!("gem home: {}", gem_home.display());
            for (gem, outcome) in &report.linked {
                println!("{gem}: {outcome}");
            }
            for gem in &report.removed {
                println!("{gem}: removed");
            }
            for warning in &report.warnings {
                eprintln!("spinel: fetch: warning: {warning}");
            }
            true
        }
        Err(err) => {
            eprintln!("spinel: fetch: {err}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// spinel resolve
// ---------------------------------------------------------------------------

/// Resolve the dependency JSON at `deps` (`-`: standard input) and print one
/// line per gem chosen, as `specs:` lists it (or, with `lockfile`, the new
/// `Gemfile.lock`), or the explanation of why there is none on standard
/// error. Exits 1 when there is no resolution, 2 when the input is not one
/// it can resolve.
fn resolve_command(
    deps: &Path,
    platforms: &[String],
    lockfile: bool,
    locked: Option<&Path>,
    unlock: &locked::Unlock,
    ruby_version: Option<&str>,
    rubygems_version: Option<&str>,
) -> ExitCode {
    let usage = |message: String| {
        eprintln!("spinel: resolve: {message}");
        ExitCode::from(EXIT_USAGE)
    };
    let text = if deps == Path::new("-") {
        let mut text = String::new();
        if let Err(err) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
            return usage(format!("standard input: {err}"));
        }
        text
    } else {
        match std::fs::read_to_string(deps) {
            Ok(text) => text,
            Err(err) => return usage(format!("{}: {err}", deps.display())),
        }
    };
    let gemfile: install::GemfileJson = match serde_json::from_str(&text) {
        Ok(gemfile) => gemfile,
        Err(err) => return usage(format!("{}: {err}", deps.display())),
    };
    let install::Prepared {
        remote,
        mut request,
        local,
        sources,
        scoped,
        ..
    } = match install::prepare(&gemfile, platforms, ruby_version, rubygems_version) {
        Ok(prepared) => prepared,
        Err(message) => return usage(message),
    };
    // ponytail: git and path sources are read by `spinel install`, which
    // clones and evaluates them, and so are scoped rubygems sources; this
    // tool resolves against one index only. Upgrade by reading them here as
    // `Session::resolve` does.
    if !sources.is_empty() || !scoped.is_empty() {
        return usage(
            "git, path and scoped rubygems sources are resolved by `spinel install`".to_owned(),
        );
    }
    let Some(remote) = remote else {
        return usage("the Gemfile names no rubygems source".to_owned());
    };

    // A lockfile to keep to (#79): `Definition` reads it, and when nothing
    // changed it is the resolution, with no index read.
    let mut definition = None;
    let mut current = String::new();
    if let Some(path) = locked {
        current = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => return usage(format!("{}: {err}", path.display())),
        };
        let lock = match lockfile::Lockfile::parse(&current, &path.display().to_string(), &local) {
            Ok(lock) => lock,
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::FAILURE;
            }
        };
        let versions = gemfile.ruby_versions();
        let local_platform = request.platforms[0].clone();
        let def = match locked::Definition::unlocking(
            &lock,
            &remote,
            &request.dependencies,
            versions.as_deref(),
            &local_platform,
            &request.platforms[1..],
            unlock,
        ) {
            Ok(def) => def,
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::FAILURE;
            }
        };
        request.platforms = def.platforms().to_vec();
        if !def.resolve_needed() {
            let specs = def.unchanged(&request.dependencies);
            return print_resolution(&def, &current, &remote, &request, None, &specs, lockfile);
        }
        definition = Some(def);
    }

    let Some(home) = home::spinel_home() else {
        eprintln!("spinel: resolve: neither SPINEL_HOME nor HOME is set");
        return ExitCode::FAILURE;
    };
    let client = match compact_index::Client::new(&remote, &home) {
        Ok(client) => client,
        Err(err) => return usage(err.to_string()),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("spinel: resolve: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let roots: Vec<String> = request
        .dependencies
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let index = match runtime.block_on(install::read_index(&client, roots, &local)) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("spinel: resolve: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(mut def) = definition {
        return match def.start_resolution(&request, &index) {
            Ok(specs) => print_resolution(
                &def,
                &current,
                &remote,
                &request,
                Some(&index),
                &specs,
                lockfile,
            ),
            Err(failure) => {
                eprintln!("{}", failure.to_string().trim_end());
                ExitCode::FAILURE
            }
        };
    }
    match resolver::resolve(&request, &index) {
        Ok(resolved) if lockfile => {
            let ruby_directive = gemfile.ruby.as_ref().is_some_and(|r| !r.is_null());
            let lock = lockfile::Lockfile::from_resolution(
                &remote,
                &request,
                &index,
                &resolved,
                ruby_directive,
                &[],
            );
            print!("{}", lock.write());
            ExitCode::SUCCESS
        }
        Ok(resolved) => {
            for spec in resolved {
                println!("{spec}");
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("{}", failure.to_string().trim_end());
            ExitCode::FAILURE
        }
    }
}

/// Print a resolution kept to `current`, the lockfile's text: the
/// `Gemfile.lock` `write_lock` leaves, or one line per build as `specs:`
/// lists it.
fn print_resolution(
    definition: &locked::Definition,
    current: &str,
    remote: &str,
    request: &resolver::Request,
    index: Option<&resolver::Index>,
    specs: &[resolver::Spec],
    lockfile: bool,
) -> ExitCode {
    if lockfile {
        print!(
            "{}",
            definition.write_lock(current, remote, request, index, specs, &[])
        );
    } else {
        let mut lines: Vec<(String, String)> = specs
            .iter()
            .map(|s| {
                let resolved = resolver::Resolved {
                    name: s.name.clone(),
                    version: s.version.clone(),
                    platform: s.platform.clone(),
                };
                (resolved.full_name(), resolved.to_string())
            })
            .collect();
        lines.sort();
        for (_, line) in lines {
            println!("{line}");
        }
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// spinel lockfile
// ---------------------------------------------------------------------------

/// Read the lockfile at `path` (`-`: standard input) and print it as Bundler
/// would write it. Exits 1 when it cannot be read as a lockfile.
fn lockfile_command(path: &Path) -> ExitCode {
    let (text, name) = if path == Path::new("-") {
        let mut text = String::new();
        if let Err(err) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
            eprintln!("spinel: lockfile: standard input: {err}");
            return ExitCode::FAILURE;
        }
        (text, "Gemfile.lock".to_owned())
    } else {
        match std::fs::read_to_string(path) {
            Ok(text) => (text, path.display().to_string()),
            Err(err) => {
                eprintln!("spinel: lockfile: {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        }
    };
    let local = gem_platform::Platform::local(spinel_vm::PLATFORM);
    match lockfile::Lockfile::parse(&text, &name, &local) {
        Ok(lock) => {
            print!("{}", lock.write());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// spinel index
// ---------------------------------------------------------------------------

/// Update the compact index cache for `source` and print its directory, then
/// one line per file: `versions: appended 120 bytes`, `info/rack: current`. Exits 1 if any
/// request failed, after reporting every one.
fn index_command(source: &str, names: bool, gems: &[String]) -> ExitCode {
    let Some(home) = home::spinel_home() else {
        eprintln!("spinel: index: neither SPINEL_HOME nor HOME is set");
        return ExitCode::FAILURE;
    };
    let client = match compact_index::Client::new(source, &home) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("spinel: index: {err}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("spinel: index: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let mut ok = true;
        let mut report =
            |what: &str, result: Result<compact_index::Outcome, compact_index::Error>| match result
            {
                Ok(outcome) => println!("{what}: {outcome}"),
                Err(err) => {
                    eprintln!("spinel: index: {err}");
                    ok = false;
                }
            };
        println!("index: {}", client.directory().display());
        report("versions", client.versions().await);
        if names {
            report("names", client.names().await);
        }
        if !gems.is_empty() {
            match client.info(gems).await {
                Ok(results) => {
                    for (gem, result) in results {
                        report(&format!("info/{gem}"), result);
                    }
                }
                Err(err) => report("info", Err(err)),
            }
        }
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    })
}

// ---------------------------------------------------------------------------
// spinel run
// ---------------------------------------------------------------------------

/// Report each Ruby the project asks for that this build does not implement
/// (docs/cli.md, "Language version"): a warning, or with `strict` an error.
/// Returns false only when `strict` and something did not match.
fn project_ruby_version_ok(strict: bool) -> bool {
    let Ok(dir) = std::env::current_dir() else {
        return true;
    };
    let mismatches = ruby_version::check(&dir);
    for mismatch in &mismatches {
        let file = mismatch.file.display();
        if strict {
            eprintln!(
                "spinel: {file} {} (--strict-ruby-version)",
                mismatch.message
            );
        } else {
            eprintln!("spinel: warning: {file} {}", mismatch.message);
        }
    }
    !(strict && !mismatches.is_empty())
}

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
    let around = Around {
        setup: true,
        ..Around::default()
    };
    run_program(path, &source, args, dump_bytecode, around)
}

/// `spinel run --watch`: the same `spinel run`, without `--watch`, as a child
/// the supervisor restarts when the current directory changes. The child reads
/// the project's Ruby version itself, so each run reports it afresh.
fn watch_run(path: &Path, args: &[String], dump_bytecode: bool, strict: bool) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("spinel: cannot read the current directory: {err}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let mut child: Vec<std::ffi::OsString> = vec!["run".into()];
    if dump_bytecode {
        child.push("--dump-bytecode".into());
    }
    if strict {
        child.push("--strict-ruby-version".into());
    }
    // After `--`, a file named like a flag is still the file.
    child.push("--".into());
    child.push(path.into());
    child.extend(args.iter().map(Into::into));
    watch::run(&root, watch::Program { args: child })
}

/// `spinel` given `ruby`'s flags: the prelude they mean, then the program
/// from `-e`, standard input or a file, as `ruby` would run it. `setup` is
/// for `spinel <file>`, which is `spinel run` and so finds the project's gems;
/// `ruby`'s own command line does not.
fn ruby_command(words: &[String], raw: &[Vec<u8>], setup: bool) -> ExitCode {
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
        // `-S`: a bare name is looked for on `RUBYPATH`, then `PATH`. Under
        // `spinel run` that is the `PATH` the setup shim makes, with the
        // project's `bin/` first; the shim runs only once the program is
        // read, so that `bin/` is looked in here, where it would come.
        if args.search_path && !script.contains('/') && script != "-" {
            let var = |name: &str| -> Vec<String> {
                std::env::var(name)
                    .map(|value| value.split(':').map(str::to_owned).collect())
                    .unwrap_or_default()
            };
            let mut dirs = var("RUBYPATH");
            if setup && let Some(bin) = project_gem_bin() {
                dirs.push(bin.to_string_lossy().into_owned());
            }
            dirs.extend(var("PATH"));
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
    // program starts further down, as if `-x` had been given. CRuby looks
    // for its engine's name, `ruby`; Spinel's is `spinel`, the name its bin
    // stubs start with (`#!/usr/bin/env spinel`), and it runs scripts
    // written for `ruby` too, so either name marks a Ruby line.
    let names_ruby = |line: &[u8]| {
        line.windows(4).any(|w| w == b"ruby") || line.windows(6).any(|w| w == b"spinel")
    };
    if args.skip_to_ruby.is_none()
        && args.code.is_none()
        && source.starts_with(b"#!")
        && !source.split(|&b| b == b'\n').next().is_some_and(names_ruby)
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
            if line.starts_with("#!") && names_ruby(line.as_bytes()) {
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
    let requires = ruby_args::requires(&args);
    run_program(
        Path::new(&name),
        &source,
        &args.argv,
        false,
        Around {
            prelude: Some(&prelude),
            requires: Some(&requires),
            library: None,
            gem_features: ruby_args::gem_features(&args),
            source_encoding: args.source_encoding.as_deref(),
            data,
            loop_template: around.as_deref(),
            frozen_string_literal: ruby_args::feature_setting(&args, "frozen_string_literal"),
            setup,
        },
    )
}

/// The `bin/` of the nearest `.spinel/gem_home/` from the current directory
/// upward: the directory `shims/setup.rb` puts first on `PATH`, found the way
/// it finds it.
fn project_gem_bin() -> Option<PathBuf> {
    let start = std::env::current_dir().ok()?;
    start
        .ancestors()
        .map(|dir| dir.join(".spinel").join("gem_home"))
        .find(|gem_home| gem_home.is_dir())
        .map(|gem_home| gem_home.join("bin"))
}

/// What a `ruby` command line adds around a program; all of it absent for
/// `spinel run`.
#[derive(Default)]
struct Around<'a> {
    /// Ruby the flags mean, run first.
    prelude: Option<&'a str>,
    /// The flags' `-r` libraries, as Ruby, run after the gem prelude.
    requires: Option<&'a str>,
    /// One of Spinel's own libraries, by the name its backtraces give and as
    /// Ruby, run after the `-r` libraries, as one more would be.
    library: Option<(&'a str, &'a str)>,
    /// What `shims/gem_prelude.rb` loads; everything for `spinel run`.
    gem_features: ruby_args::GemFeatures,
    /// `-K`'s source encoding, for a script without a magic comment.
    source_encoding: Option<&'a str>,
    /// The file `DATA` reads and the offset it starts at.
    data: Option<(String, usize)>,
    /// The `-n`/`-p` loop to put the program in.
    loop_template: Option<&'a str>,
    /// `--enable=frozen-string-literal` or `--disable=frozen-string-literal`;
    /// `None` when neither is given.
    frozen_string_literal: Option<bool>,
    /// Run `shims/setup.rb` first: the project's gems, for `spinel run` and
    /// `spinel <file>`.
    setup: bool,
}

/// Spinel's setup shim (docs/cli.md, "What `spinel run` sets up").
const SETUP: &str = include_str!("../../../shims/setup.rb");

/// CRuby's `gem_prelude.rb`, Spinel's (docs/cli.md, "What every program
/// starts with").
const GEM_PRELUDE: &str = include_str!("../../../shims/gem_prelude.rb");

/// The gem prelude's Ruby: the modules for the features left on, defined
/// first as `ruby.c` defines them, then the shim, which loads each one.
fn gem_prelude(features: ruby_args::GemFeatures) -> String {
    let mut code = String::new();
    if features.gems {
        code.push_str("module Gem; end\n");
    }
    if features.did_you_mean {
        code.push_str("module DidYouMean; end\n");
    }
    if !code.is_empty() {
        code.push_str(GEM_PRELUDE);
    }
    code
}

/// The vendored standard library on `$LOAD_PATH`, last, as CRuby has its
/// `rubylibdir` (and `RbConfig` says so, as `bundled_gems.rb` reads it),
/// with `RbConfig`'s `sitelibdir` before it as CRuby has that: RubyGems puts
/// the gems it activates ahead of `sitelibdir`, so after `-I`. The flags'
/// `-I` directories go before both.
///
/// Each of these default entries carries `@gem_prelude_index`, set to the
/// entry itself, as `ruby.c` marks its own: `Gem.load_path_insert_index`
/// finds the first marked entry, and that is where activated gems go.
fn library_path(scope: &mut spinel_vm::HandleScope<'_>) {
    let Some(dir) = stdlib::directory() else {
        return;
    };
    let literal = ruby_literal(&dir.to_string_lossy());
    let code = format!(
        "RbConfig::CONFIG[\"rubylibdir\"] = {literal}.dup\n\
         RbConfig::CONFIG[\"rubyarchdir\"] = File.join({literal}, RUBY_PLATFORM)\n\
         [RbConfig::CONFIG[\"sitelibdir\"].dup, {literal}.dup].each do |path|\n\
           path.instance_variable_set(:@gem_prelude_index, path)\n\
           $LOAD_PATH.push(path.freeze)\n\
         end"
    );
    let parsed = spinel_parse::parse_file("<stdlib>", code.as_bytes());
    let compiled =
        spinel_vm::compile::program(&parsed.program).expect("a push onto $LOAD_PATH compiles");
    let mut frame = spinel_vm::interp::Frame::new(0);
    let _ = spinel_vm::interp::eval_in(scope, &mut frame, &compiled);
}

/// `text` as a double-quoted Ruby String literal. Rust's escapes are Ruby's
/// too, once `#` cannot start an interpolation.
fn ruby_literal(text: &str) -> String {
    format!("{text:?}").replace('#', "\\#")
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
        requires,
        library,
        gem_features,
        source_encoding,
        data,
        loop_template,
        frozen_string_literal,
        setup,
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
    spinel_vm::signal::install_ruby_defaults();
    scope.set_frozen_string_literal(frozen_string_literal);
    library_path(&mut scope);
    // Before the flags' `-r` files, which can already see it.
    if let Err(err) = spinel_vm::interp::define_toplevel_binding(&mut scope) {
        eprintln!("{}: {}", path.display(), describe_error(&err));
        return ExitCode::FAILURE;
    }

    // The flags' meaning runs first, as `ruby` runs `-r` before it reads the
    // program: an `at_exit` a required file registers runs even when the
    // program does not parse.
    let mut outcome = Ok(spinel_vm::Value::NIL);
    // The project's gems before anything is required: a `-r` file or the
    // program's first line may be `require "bundler/setup"`.
    if setup {
        let parsed = spinel_parse::parse_file("<setup>", SETUP.as_bytes());
        let compiled =
            spinel_vm::compile::program(&parsed.program).expect("shims/setup.rb compiles");
        let mut frame = spinel_vm::interp::Frame::new(0);
        outcome = spinel_vm::interp::eval_in(&mut scope, &mut frame, &compiled)
            .map(|_| spinel_vm::Value::NIL);
    }
    if outcome.is_ok()
        && let Some(prelude) = prelude.filter(|p| !p.is_empty())
    {
        // Spinel's own Ruby, so failing to compile it is a bug, not the
        // program's.
        let parsed = spinel_parse::parse_file("<flags>", prelude.as_bytes());
        let compiled = spinel_vm::compile::program(&parsed.program)
            .expect("the prelude the flags make compiles");
        let mut frame = spinel_vm::interp::Frame::new(0);
        outcome = spinel_vm::interp::eval_in(&mut scope, &mut frame, &compiled)
            .map(|_| spinel_vm::Value::NIL);
    }
    // RubyGems and did_you_mean, after `-I` and before `-r`, as `ruby` runs
    // `gem_prelude.rb` between them.
    let gem_prelude = gem_prelude(gem_features);
    let (library_name, library) = library.unwrap_or(("", ""));
    for (name, code) in [
        ("<internal:gem_prelude>", gem_prelude.as_str()),
        ("<flags>", requires.unwrap_or("")),
        (library_name, library),
    ] {
        if outcome.is_err() || code.is_empty() {
            continue;
        }
        let parsed = spinel_parse::parse_file(name, code.as_bytes());
        let compiled =
            spinel_vm::compile::program(&parsed.program).expect("Spinel's own prelude compiles");
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
                // Inside `TOPLEVEL_BINDING`, which is what makes its locals
                // the script's.
                let mut frame = spinel_vm::interp::Frame::new(0);
                outcome = spinel_vm::interp::eval_main(&mut scope, &mut frame, &iseq)
                    .map(|_| spinel_vm::Value::NIL);
            }
        }
    }
    if outcome.is_ok() && !syntax_error {
        outcome = spinel_core::handle_signals(&mut scope).map(|_| spinel_vm::Value::NIL);
    }
    // The `at_exit` blocks run before the error is reported, as CRuby runs
    // them: one may `exit!`, and then nothing is printed.
    let failed = outcome.is_err() || syntax_error;
    let status = spinel_core::exit_status(&mut scope, failed);
    // `exit` is a `SystemExit` that nothing rescued: not an error to report,
    // and its status is the process's. A `SignalException` ends the process
    // by its signal, below, and CRuby prints nothing for one.
    // `STDOUT`'s broken pipe is an `Errno::EPIPE` that ends the process by
    // SIGPIPE, with nothing printed, when nothing rescued it.
    let signalled = if matches!(&outcome, Err(spinel_vm::Error::Uncaught { .. })) {
        spinel_core::uncaught_signal(&mut scope)
    } else {
        None
    };
    let quiet = signalled.is_some()
        || matches!(
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
                let _ = writeln!(
                    std::io::stderr(),
                    "        Spinel's core library is still minimal, so this may be Spinel rather than your program.\n        What exists: {MILESTONES}"
                );
            }
        } else {
            // A report the standard error cannot take is dropped, as CRuby
            // drops it, so a broken STDERR still ends with status 1 rather
            // than `eprintln!`'s panic.
            let _ = writeln!(
                std::io::stderr(),
                "{}: {}",
                path.display(),
                describe_error(err)
            );
        }
    }
    // Threads, finalizers and IOs are finished after the report, so an IO's
    // `xml: :attr` quote closes around it.
    spinel_core::cleanup(&mut scope);
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
    if let Some(number) = signalled
        && let Some((name, _)) = spinel_vm::signal::list().find(|&(_, n)| n == number)
    {
        spinel_vm::signal::die_by(name);
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
