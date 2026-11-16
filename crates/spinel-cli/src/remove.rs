//! `spinel remove` (#86, docs/package-manager.md, "10. `spinel remove`"):
//! `bundle remove`, the inverse of `spinel add`. Each gem's `gem` lines are
//! taken out of the Gemfile, as `Injector#remove` takes them out, and the
//! project installed from the Gemfile as it then reads, the lockfile held
//! (#79), so the gems removed and those only they needed leave the lockfile
//! and the GEM_HOME while every other locked version stays.
//!
//! The edit is `gemfile_edit::remove`, which `spinel_ast` places: the lines
//! of each statement and of a block it leaves empty go, and no other byte
//! changes. Unlike `bundle remove`, nothing is written until the install has
//! linked everything, so a failed removal leaves the Gemfile and the
//! lockfile byte for byte as they were.

use std::process::ExitCode;

use crate::gem_requirement::Requirement;
use crate::gemfile_edit;
use crate::install::{self, GemfileJson, GemfileJsonDependency, GemfileWrite, Session, Stop};

const COMMAND: &str = "remove";

/// The exit status of Bundler's `GemfileError`.
const EXIT_GEMFILE_ERROR: u8 = 4;
/// The exit status of Bundler's `InvalidOption`.
const EXIT_INVALID_OPTION: u8 = 15;

/// `spinel remove GEM...`.
pub fn run(gems: &[String]) -> ExitCode {
    match remove(gems) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop(code)) => code,
    }
}

fn remove(gems: &[String]) -> Result<(), Stop> {
    if gems.is_empty() {
        eprintln!("Please specify gems to remove.");
        return Err(Stop(ExitCode::from(EXIT_INVALID_OPTION)));
    }
    let before = install::evaluate_gemfile(COMMAND, None, None)?;
    let Some(path) = before.gemfiles.first().map(std::path::PathBuf::from) else {
        return Err(Stop::failure(
            COMMAND,
            "the Gemfile evaluator named no Gemfile",
        ));
    };
    let shown = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
    println!("Removing gems from {}", shown.display());

    // `remove_gems_from_dependencies`: each name must be a dependency, and
    // is taken off the list once, so a name given twice is not one the
    // second time.
    let mut left: Vec<&GemfileJsonDependency> = before.dependencies.iter().collect();
    let mut asked: Vec<&GemfileJsonDependency> = Vec::new();
    for name in gems {
        let Some(at) = left.iter().position(|d| d.name == *name) else {
            eprintln!(
                "`{name}` is not specified in {} so it could not be removed.",
                shown.display()
            );
            return Err(Stop(ExitCode::from(EXIT_GEMFILE_ERROR)));
        };
        asked.push(left[at]);
        left.retain(|d| d.name != *name);
    }

    let old = std::fs::read(&path)
        .map_err(|err| Stop::failure(COMMAND, format!("{}: {err}", path.display())))?;
    let new = gemfile_edit::remove(&old, gems);
    let evaluated;
    let after: &GemfileJson = if new == old {
        &before
    } else {
        evaluated = install::evaluate_gemfile(COMMAND, Some(&new), None)?;
        &evaluated
    };

    // `cross_check_for_errors`: a gem not asked for that would go with them
    // stops it, and one asked for still listed could not be removed.
    let listed = |name: &str| after.dependencies.iter().any(|d| d.name == name);
    let extra: Vec<String> = left
        .iter()
        .filter(|d| !listed(&d.name))
        .map(|d| format!("{} ({})", d.name, requirement(d)))
        .collect();
    if !extra.is_empty() {
        eprintln!(
            "Gems could not be removed. {} would also have been removed. Bundler cannot continue.",
            extra.join(", ")
        );
        return Err(Stop(ExitCode::from(EXIT_INVALID_OPTION)));
    }
    let (errored, removed): (Vec<_>, Vec<_>) = asked.into_iter().partition(|d| listed(&d.name));
    if !errored.is_empty() {
        let names: Vec<&str> = errored.iter().map(|d| d.name.as_str()).collect();
        println!("{} could not be removed.", names.join(", "));
    }
    if removed.is_empty() {
        println!("No gems were removed from the gemfile.");
    }
    for dep in &removed {
        println!("{} was removed.", pretty_dependency(dep));
    }

    let mut session = Session::new(COMMAND, after, false)?;
    let resolved = session.resolve()?;
    let write = (new != old).then_some(GemfileWrite {
        old: &old,
        new: &new,
    });
    session.install(after, resolved, write)
}

/// `Gem::Requirement#to_s`: the list, `>= 0` when there is none.
fn requirement(dep: &GemfileJsonDependency) -> String {
    match Requirement::new(dep.requirements.iter().map(String::as_str)) {
        Ok(requirement) => requirement
            .requirements()
            .iter()
            .map(|(op, version)| format!("{} {version}", op.as_str()))
            .collect::<Vec<_>>()
            .join(", "),
        Err(_) => dep.requirements.join(", "),
    }
}

/// `SharedHelpers.pretty_dependency`: the name, the requirement unless it
/// is the default, and the platforms unless they are only `ruby`.
fn pretty_dependency(dep: &GemfileJsonDependency) -> String {
    let mut out = dep.name.clone();
    let requirement = requirement(dep);
    if requirement != ">= 0" {
        out.push_str(&format!(" ({requirement})"));
    }
    let platforms = dep.platforms.join(", ");
    if !platforms.is_empty() && platforms != "ruby" {
        out.push(' ');
        out.push_str(&platforms);
    }
    out
}
