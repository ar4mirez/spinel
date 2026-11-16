//! `spinel update` (#87, docs/package-manager.md, "11. `spinel update`"):
//! `bundle update`. With gems named, those gems and every locked gem they
//! reach are unlocked, as Bundler unlocks them by default (not
//! `--conservative`), and each named one is taken to the version a
//! resolution with everything unlocked gives it; every other gem keeps its
//! locked version. With none (or `--all`), every gem is unlocked. Either
//! way no locked gem goes below its locked version unless a conflict takes
//! its floor off, and the resolution is `locked::Definition`'s (#79) with an
//! [`Unlock`]; then the project is installed as `spinel install` installs
//! it. Nothing is written until everything is linked, and the lockfile last,
//! so a failed update leaves it byte for byte as it was.

use std::process::ExitCode;

use crate::did_you_mean;
use crate::install::{self, Session, Stop};
use crate::locked::Unlock;
use crate::lockfile::Lockfile;

const COMMAND: &str = "update";

/// The exit status of Bundler's `GemNotFound`.
const EXIT_GEM_NOT_FOUND: u8 = 7;
/// The exit status of Bundler's `InvalidOption`.
const EXIT_INVALID_OPTION: u8 = 15;
/// The exit status of Bundler's `GemfileLockNotFound`.
const EXIT_LOCKFILE_NOT_FOUND: u8 = 22;

/// The `bundle update` options Spinel does not take yet, each as it was
/// given: `--conservative`, `--patch`, `--minor`, `--major`, `--group`.
pub struct Unsupported(pub Vec<&'static str>);

/// `spinel update [GEM...] [--all]`.
pub fn run(gems: &[String], all: bool, unsupported: &Unsupported) -> ExitCode {
    match update(gems, all, unsupported) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop(code)) => code,
    }
}

fn update(gems: &[String], all: bool, unsupported: &Unsupported) -> Result<(), Stop> {
    // ponytail: the conservative unlock, the patch levels and groups are
    // refused as Bundler refuses an option it does not know of. Upgrade:
    // `Definition`'s `conservative`, `GemVersionPromoter#level` and the
    // groups' gems named (#87's follow-up).
    if let Some(option) = unsupported.0.first() {
        eprintln!("`{option}` is not supported by `spinel update` yet.");
        return Err(Stop(ExitCode::from(EXIT_INVALID_OPTION)));
    }
    if all && !gems.is_empty() {
        eprintln!("Cannot specify --all along with specific options.");
        return Err(Stop(ExitCode::from(EXIT_INVALID_OPTION)));
    }
    let gemfile = install::evaluate_gemfile(COMMAND, None, None)?;
    let mut session = Session::new(COMMAND, &gemfile, false)?;
    let before = session.current_lockfile()?;

    let unlock = if gems.is_empty() {
        Unlock::All
    } else {
        let Some(lock) = &before else {
            eprintln!(
                "This Bundle hasn't been installed yet. \
                 Run `spinel install` to update and install the bundled gems."
            );
            return Err(Stop(ExitCode::from(EXIT_LOCKFILE_NOT_FOUND)));
        };
        // `ensure_all_gems_in_lockfile!`.
        let locked = locked_names(lock);
        let mut missing: Vec<&String> = Vec::new();
        for name in gems.iter().filter(|g| !locked.contains(g)) {
            if !missing.contains(&name) {
                missing.push(name);
            }
        }
        if !missing.is_empty() {
            let messages: Vec<String> = missing
                .iter()
                .map(|name| gem_not_found_message(name, &locked))
                .collect();
            eprintln!("{}", messages.join("\n"));
            return Err(Stop(ExitCode::from(EXIT_GEM_NOT_FOUND)));
        }
        Unlock::Gems(gems.to_vec())
    };
    session.set_unlock(unlock);
    let resolved = session.resolve()?;
    let after = session.written_lockfile(&resolved)?;
    session.install(&gemfile, resolved, None)?;

    // What `CLI::Update` warns of for each gem named that did not go up.
    // ponytail: a gem the update no longer locks is passed over without
    // `installable_on_platform?`'s warning that it is for another platform.
    // Upgrade with that check against the local platform.
    if let Some(before) = &before {
        for name in gems {
            let Some(old) = before
                .sources
                .iter()
                .flat_map(|s| &s.specs)
                .rfind(|s| &s.name == name)
            else {
                continue;
            };
            let Some(new) = after
                .sources
                .iter()
                .flat_map(|s| &s.specs)
                .find(|s| &s.name == name)
            else {
                continue;
            };
            if new.version < old.version {
                eprintln!(
                    "Note: {name} version regressed from {} to {}",
                    old.version, new.version
                );
            } else if new.version == old.version {
                eprintln!("Bundler attempted to update {name} but its version stayed the same");
            }
        }
    }
    Ok(())
}

/// The names of the gems a lockfile locks, once each, in its order.
fn locked_names(lock: &Lockfile) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for spec in lock.sources.iter().flat_map(|s| &s.specs) {
        if !names.contains(&spec.name) {
            names.push(spec.name.clone());
        }
    }
    names
}

/// `CLI::Common.gem_not_found_message`.
fn gem_not_found_message(missing: &str, alternatives: &[String]) -> String {
    let mut message = format!("Could not find gem '{missing}'.");
    let downcased = missing.to_lowercase();
    if alternatives.contains(&downcased) {
        message.push_str(&format!("\nDid you mean '{downcased}'?"));
    } else {
        let suggestions = did_you_mean::correct(alternatives, missing);
        if !suggestions.is_empty() {
            message.push_str(&format!("\nDid you mean {}?", word_list(&suggestions)));
        }
    }
    message
}

/// `CLI::Common.word_list`: `'a'`, `'a' or 'b'`, `'a', 'b' or 'c'`.
fn word_list(words: &[&str]) -> String {
    let quoted: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gem_not_found_is_worded_as_bundler_words_it() {
        let names: Vec<String> = ["rake", "rack", "other"].map(String::from).to_vec();
        assert_eq!(
            gem_not_found_message("rakc", &names),
            "Could not find gem 'rakc'.\nDid you mean 'rake'?"
        );
        assert_eq!(
            gem_not_found_message("Rake", &names),
            "Could not find gem 'Rake'.\nDid you mean 'rake'?"
        );
        assert_eq!(
            gem_not_found_message("nokogiri", &names),
            "Could not find gem 'nokogiri'."
        );
        assert_eq!(word_list(&["a", "b", "c"]), "'a', 'b' or 'c'");
        assert_eq!(word_list(&["a", "b"]), "'a' or 'b'");
    }
}
