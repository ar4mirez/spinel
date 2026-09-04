//! The `spinel` binary.
//!
//! Phase 0 ships the skeleton: this parses arguments and reports its version.
//! Subcommands land one issue at a time; see `docs/cli.md` for the full surface.

use std::process::ExitCode;

use clap::Parser;

const MILESTONES: &str = "https://github.com/ar4mirez/spinel/milestones";

/// Shown under the options list. States plainly that this build does not run Ruby
/// yet, so nobody installs it and wonders why `spinel app.rb` does nothing.
const AFTER_HELP: &str = "\
This build does not run Ruby yet. It is the Phase 0 skeleton: it compiles, and it
reports its version.

Planned surface (docs/cli.md): run, x, init, install, add, remove, update, test, build.
Progress: https://github.com/ar4mirez/spinel/milestones";

#[derive(Parser, Debug)]
#[command(
    name = "spinel",
    about = "A Ruby engine and toolchain, in one binary.",
    disable_version_flag = true,
    arg_required_else_help = true,
    override_usage = "spinel [OPTIONS]",
    after_help = AFTER_HELP,
)]
struct Cli {
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

fn main() -> ExitCode {
    // `arg_required_else_help` means a bare `spinel` never reaches this point: clap
    // prints help and exits non-zero, so a typo is never a silent success.
    let cli = Cli::parse();

    if cli.version {
        println!("{}", spinel_vm::description());
        return ExitCode::SUCCESS;
    }

    if let Some(file) = cli.file {
        // The single likeliest first thing a Ruby developer types. Say what is true.
        eprintln!("spinel: cannot run `{file}` — this build has no VM yet.");
        eprintln!("        Running Ruby lands in phase 1. Progress: {MILESTONES}");
        return ExitCode::from(2);
    }

    ExitCode::SUCCESS
}
