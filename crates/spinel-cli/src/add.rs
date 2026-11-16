//! `spinel add` (#85, docs/package-manager.md, "9. `spinel add`"): `bundle
//! add`, a `gem` line written into the Gemfile and the project installed, in
//! Bundler's `Injector#inject` order. The gems the Gemfile already lists
//! with the same requirement are dropped; the rest are evaluated after the
//! Gemfile as `injected gems` and resolved with the lockfile held, which
//! is where a duplicate or a conflict stops it; then each line is written
//! with the requirement `bundle add` writes (`~> 4.2` from the version
//! resolved, unless one was given) and the project is installed from the
//! Gemfile as it now reads.
//!
//! Unlike `bundle add`, nothing is written until the install has fetched and
//! linked everything: a failed resolution or install leaves the Gemfile and
//! the lockfile as they were, byte for byte. The Gemfile is written just
//! before the lockfile, and put back if the lockfile cannot be.

use std::process::ExitCode;

use crate::gem_requirement::Requirement;
use crate::gem_version::Version;
use crate::gemfile_edit::{self, NewGem};
use crate::install::{self, GemfileWrite, Session, Stop};

const COMMAND: &str = "add";

/// The exit status of Bundler's `InvalidOption`.
const EXIT_INVALID_OPTION: u8 = 15;

pub struct Request<'a> {
    /// Each `name` or `name@requirement`, several requirements separated by
    /// commas.
    pub gems: &'a [String],
    /// `--group`: group names separated by commas.
    pub group: Option<&'a str>,
    /// `--git`.
    pub git: Option<&'a str>,
}

/// `spinel add`.
pub fn run(request: &Request) -> ExitCode {
    match add(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop(code)) => code,
    }
}

/// A gem asked for.
struct Wanted {
    name: String,
    requirement: Requirement,
    /// The requirement as written, `as_list`, when one was given.
    given: Option<Vec<String>>,
}

fn add(request: &Request) -> Result<(), Stop> {
    let mut wanted = Vec::new();
    for word in request.gems {
        let (name, list) = match word.split_once('@') {
            Some((name, list)) => (name, Some(list)),
            None => (word.as_str(), None),
        };
        if name.is_empty() {
            return Err(Stop::failure(
                COMMAND,
                format!("`{word}` names no gem (e.g. spinel add sinatra@4.2)"),
            ));
        }
        let given: Option<Vec<&str>> = list.map(|l| l.split(',').map(str::trim).collect());
        for item in given.iter().flatten() {
            // `Gem::Requirement::PATTERN`, as `CLI::Add#validate_options!`
            // checks it.
            if Requirement::parse_one(item).is_err() {
                eprintln!("Invalid gem requirement pattern '{item}'");
                return Err(Stop(ExitCode::from(EXIT_INVALID_OPTION)));
            }
        }
        let requirement = Requirement::new(given.iter().flatten().copied())
            .map_err(|err| Stop::failure(COMMAND, err))?;
        let given = given.map(|_| as_list(&requirement));
        wanted.push(Wanted {
            name: name.to_owned(),
            requirement,
            given,
        });
    }
    // `Dependency#groups`: `default` alone is no group at all.
    let mut groups: Vec<String> = request
        .group
        .map(|g| {
            g.split(',')
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if groups == ["default"] {
        groups.clear();
    }

    // `@deps -= builder.dependencies`: a gem the Gemfile already lists with
    // the same requirement is not added again, whatever its groups.
    let before = install::evaluate_gemfile(COMMAND, None, None)?;
    wanted.retain(|w| {
        !before.dependencies.iter().any(|d| {
            d.name == w.name
                && Requirement::new(d.requirements.iter().map(String::as_str))
                    .is_ok_and(|r| r == w.requirement)
        })
    });
    if wanted.is_empty() {
        let mut session = Session::new(COMMAND, &before, false)?;
        let resolved = session.resolve()?;
        return session.install(&before, resolved, None);
    }

    // The new lines with the requirement asked for (`>= 0` when none was),
    // after the Gemfile, resolved: the version each one gets.
    let git = request.git.map(str::to_owned);
    let unversioned: Vec<String> = wanted
        .iter()
        .map(|w| {
            let gem = NewGem {
                name: w.name.clone(),
                requirements: as_list(&w.requirement),
                git: git.clone(),
            };
            gemfile_edit::line(&gem, &groups)
        })
        .collect();
    let injected = install::evaluate_gemfile(COMMAND, None, Some(&unversioned.join("\n")))?;
    let mut session = Session::new(COMMAND, &injected, false)?;
    let resolved = session.resolve()?;
    let specs = session.locked_specs(&resolved)?;
    let gems: Vec<NewGem> = wanted
        .iter()
        .map(|w| {
            let requirements = w.given.clone().unwrap_or_else(|| {
                let version = specs.iter().find(|s| s.name == w.name).map(|s| &s.version);
                vec![conservative_version(version)]
            });
            NewGem {
                name: w.name.clone(),
                requirements,
                git: git.clone(),
            }
        })
        .collect();

    // The Gemfile with those lines, read back as the install will read it.
    let path = session.gemfile_path().to_owned();
    let old = std::fs::read(&path)
        .map_err(|err| Stop::failure(COMMAND, format!("{}: {err}", path.display())))?;
    let new = gemfile_edit::add(&old, &gems, &groups);
    let after = install::evaluate_gemfile(COMMAND, Some(&new), None)?;
    let in_groups: Vec<String> = if groups.is_empty() {
        vec!["default".to_owned()]
    } else {
        groups.clone()
    };
    for gem in &gems {
        let read_back = after.dependencies.iter().any(|d| {
            d.name == gem.name
                && Requirement::new(d.requirements.iter().map(String::as_str)).ok()
                    == Requirement::new(gem.requirements.iter().map(String::as_str)).ok()
                && same_set(&d.groups, &in_groups)
        });
        if !read_back {
            return Err(Stop::failure(
                COMMAND,
                format!(
                    "{}: the `gem \"{}\"` line would not read back as written there; the Gemfile is unchanged",
                    path.display(),
                    gem.name
                ),
            ));
        }
    }

    let mut session = Session::new(COMMAND, &after, false)?;
    let resolved = session.resolve()?;
    session.install(
        &after,
        resolved,
        Some(GemfileWrite {
            old: &old,
            new: &new,
        }),
    )
}

/// `Gem::Requirement#as_list`: each pair `op version`.
fn as_list(requirement: &Requirement) -> Vec<String> {
    requirement
        .requirements()
        .iter()
        .map(|(op, version)| format!("{} {version}", op.as_str()))
        .collect()
}

/// `Injector#conservative_version`, `~>` as `bundle add` writes it with
/// neither `--strict` nor `--optimistic`: the first two segments of the
/// version resolved (three below 1.0), and any prerelease part.
fn conservative_version(version: Option<&Version>) -> String {
    let Some(version) = version else {
        return ">= 0".to_owned();
    };
    let one = Version::new("1.0").expect("1.0 is a version");
    let take = if *version >= one { 2 } else { 3 };
    let segments: Vec<&str> = version
        .segments()
        .iter()
        .take(take)
        .map(|s| s.as_str())
        .collect();
    let prerelease = if version.prerelease() {
        let release = version.release();
        version
            .as_str()
            .strip_prefix(release.as_str())
            .unwrap_or(version.as_str())
            .to_owned()
    } else {
        String::new()
    };
    format!("~> {}{prerelease}", segments.join("."))
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&String> = a.iter().collect();
    let mut b: Vec<&String> = b.iter().collect();
    a.sort_unstable();
    a.dedup();
    b.sort_unstable();
    b.dedup();
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_versions_are_bundlers() {
        let of = |v: &str| conservative_version(Some(&Version::new(v).unwrap()));
        assert_eq!(of("4.2.1"), "~> 4.2");
        assert_eq!(of("13.4.2"), "~> 13.4");
        assert_eq!(of("1.0"), "~> 1.0");
        assert_eq!(of("2"), "~> 2");
        assert_eq!(of("0.3.0"), "~> 0.3.0");
        assert_eq!(of("0.15.2.1"), "~> 0.15.2");
        assert_eq!(of("8.1.0.beta1"), "~> 8.1.beta1");
        assert_eq!(of("1.0.0.pre.1"), "~> 1.0.0.pre.1");
        assert_eq!(of("1.1.0.pre.1"), "~> 1.1.pre.1");
        assert_eq!(conservative_version(None), ">= 0");
    }

    #[test]
    fn requirements_are_written_as_listed() {
        let list = |s: &[&str]| as_list(&Requirement::new(s.iter().copied()).unwrap());
        assert_eq!(list(&["13.4.2"]), ["= 13.4.2"]);
        assert_eq!(list(&["> 1", "< 14"]), ["> 1", "< 14"]);
        assert_eq!(list(&["~>13.0"]), ["~> 13.0"]);
        assert_eq!(list(&[]), [">= 0"]);
    }
}
