//! `spinel x <gem>[@req] [args]` (#68, docs/cli.md, "`spinel x`"): run a
//! gem's executable from a GEM_HOME of its own under `~/.spinel/x/`, leaving
//! the project untouched.
//!
//! RubyGems' `gem exec` and uv's `uvx` are the models. As `gem exec` does,
//! every run resolves the gem afresh against the source, so an unpinned gem
//! is its newest version, and the executable is chosen as its `load!`
//! chooses it. As `uvx` does, the environment is kept between runs, named by
//! what was resolved: the GEM_HOME is `<spinel home>/x/<gem>-<version>-<key>/`,
//! the key a hash of every gem's full name and `.gem` checksum, so a second
//! run that resolves to the same gems fetches nothing and links nothing.
//!
//! Every step is a piece `spinel install` uses: the compact index read
//! ([`install::read_index`]), the resolver, the global store and the GEM_HOME
//! link. Nothing is read from or written to the current directory: no
//! Gemfile, no lockfile, no `.spinel/`.

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use sha2::{Digest as _, Sha256};

use crate::install::{self, GemfileJson, GemfileJsonDependency, GemfileSource};
use crate::resolver::Resolved;
use crate::{compact_index, gem_home, gem_requirement, gem_store, home, resolver};

/// `gem exec`'s status when no gem satisfies the request
/// (`Gem::GemNotFoundException`, `UnsatisfiableDependencyError`).
const EXIT_NOT_FOUND: u8 = 2;

/// Why `spinel x` stopped before running anything: the exit status, after
/// what to say was printed.
struct Stop(ExitCode);

impl Stop {
    fn failure(message: impl std::fmt::Display) -> Stop {
        eprintln!("spinel: x: {message}");
        Stop(ExitCode::FAILURE)
    }

    fn not_found(message: impl std::fmt::Display) -> Stop {
        eprintln!("spinel: x: {message}");
        Stop(ExitCode::from(EXIT_NOT_FOUND))
    }
}

/// What to run: `command` is the first word after `spinel x`, `gem[@req]`;
/// `gem`, from `--gem`, names the gem when the executable is named otherwise,
/// as `gem exec -g` does, and then the requirement in `command` is the gem's.
pub struct Request<'a> {
    pub source: &'a str,
    pub gem: Option<&'a str>,
    pub command: &'a str,
    pub args: &'a [String],
}

/// `spinel x`. Returns only when the executable could not be started; once
/// it starts, it replaces this process, so its exit status is the command's.
pub fn run(request: &Request) -> ExitCode {
    match prepare(request) {
        Ok((gem_home, stub)) => exec(&gem_home, &stub, request.args),
        Err(Stop(code)) => code,
    }
}

/// `name@requirement` split, as `gem exec`'s `extract_gem_name_and_version`
/// splits `name:version`. Several requirements are separated by commas, as
/// a Gemfile's `gem` line lists them: `rubocop@~> 1.60, >= 1.60.2`.
fn split_command(command: &str) -> (&str, Vec<&str>) {
    match command.split_once('@') {
        Some((name, requirement)) => (name, requirement.split(',').collect()),
        None => (command, Vec::new()),
    }
}

/// Resolve, fetch and link; the GEM_HOME and the stub to run.
fn prepare(request: &Request) -> Result<(PathBuf, PathBuf), Stop> {
    let (executable, requirements) = split_command(request.command);
    let gem = request.gem.unwrap_or(executable);
    if executable.is_empty() {
        return Err(Stop::failure(
            "name an executable to run (e.g. spinel x rubocop)",
        ));
    }
    // Only the words before the gem are `spinel x`'s, so an option in its
    // place is one it does not have, not a gem.
    if gem.starts_with('-') {
        return Err(Stop::not_found(format!(
            "unexpected argument `{gem}`: name the gem first, then the executable's arguments"
        )));
    }
    if !compact_index::valid_name(gem) {
        return Err(Stop::not_found(format!("`{gem}` is not a gem name")));
    }
    // A Gemfile of one `gem` line, as the resolver reads one.
    let gemfile = GemfileJson {
        gemfiles: Vec::new(),
        source: Some(GemfileSource {
            kind: "rubygems".to_owned(),
            remote: Some(request.source.to_owned()),
        }),
        ruby: None,
        optional_groups: Vec::new(),
        gemspecs: Vec::new(),
        dependencies: vec![GemfileJsonDependency {
            name: gem.to_owned(),
            requirements: requirements.iter().map(|r| r.trim().to_owned()).collect(),
            groups: Vec::new(),
            platforms: Vec::new(),
            source: None,
            install_if: true,
            env: None,
            force_ruby_platform: None,
        }],
    };
    let mut prepared = install::prepare(&gemfile, &[], None, None).map_err(|message| {
        eprintln!("spinel: x: {message}");
        Stop(ExitCode::from(EXIT_NOT_FOUND))
    })?;
    // The root of the resolver's explanations, where a Gemfile is named.
    prepared.request.gemfile = "spinel x".to_owned();
    let requirement = prepared.request.dependencies[0].requirement.clone();

    let Some(home) = home::spinel_home() else {
        return Err(Stop::failure("neither SPINEL_HOME nor HOME is set"));
    };
    // The request above names a rubygems source.
    let remote = prepared.remote.clone().unwrap_or_default();
    let client = compact_index::Client::new(&remote, &home).map_err(Stop::failure)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| Stop::failure(format!("cannot start the async runtime: {err}")))?;
    let index = runtime
        .block_on(install::read_index(
            &client,
            vec![gem.to_owned()],
            &prepared.local,
        ))
        .map_err(Stop::failure)?;

    // `gem exec`'s lookup failure, before the resolver explains a conflict
    // deeper down.
    let shown = shown_requirement(&requirement);
    let builds = index.get(gem).map(Vec::as_slice).unwrap_or_default();
    if !builds.iter().any(|b| requirement.satisfied_by(&b.version)) {
        return Err(Stop::not_found(format!(
            "could not find a valid gem '{gem}' ({shown}) in {}",
            remote
        )));
    }
    let resolved = resolver::resolve(&prepared.request, &index).map_err(|failure| {
        eprintln!("{}", failure.to_string().trim_end());
        Stop(ExitCode::from(EXIT_NOT_FOUND))
    })?;

    // Each build's checksum, from the `/info` line it was resolved from.
    let mut wants = Vec::new();
    for spec in &resolved {
        let id = gem_store::GemId::new(
            &spec.name,
            &spec.version.to_string(),
            &spec.platform.to_string(),
        )
        .map_err(Stop::failure)?;
        let sha = index
            .get(&spec.name)
            .and_then(|builds| {
                builds
                    .iter()
                    .find(|b| b.version == spec.version && b.platform == spec.platform)
            })
            .and_then(|b| b.checksum.clone());
        match sha {
            Some(sha) => wants.push((id, sha.to_ascii_lowercase())),
            // ponytail: a gem with no checksum is refused, since the store
            // is keyed by it, as `spinel install` refuses one. Upgrade: hash
            // the `.gem` as it arrives and key the entry by that.
            None => {
                return Err(Stop::failure(format!(
                    "{}: {} lists no checksum for it",
                    id.full_name(),
                    remote
                )));
            }
        }
    }

    let store = gem_store::Store::new(&home);
    let mut stored = Vec::new();
    let mut ok = true;
    for (gem, result) in runtime.block_on(store.fetch(&client, &wants)) {
        match result {
            Ok(fetched) => {
                for warning in fetched.warnings {
                    eprintln!("spinel: x: warning: {warning}");
                }
                stored.push((gem, absolute(&fetched.path)));
            }
            Err(err) => {
                eprintln!("spinel: x: {err}");
                ok = false;
            }
        }
    }
    if !ok {
        return Err(Stop(ExitCode::FAILURE));
    }
    drop(runtime);

    let root = resolved
        .iter()
        .find(|r| r.name == gem)
        .ok_or_else(|| Stop::failure(format!("the resolution has no {gem}")))?;
    let gem_home = absolute(&home.join("x").join(directory_name(root, &wants)));
    let report = gem_home::link(&gem_home, &stored).map_err(Stop::failure)?;
    let choice = choose_executable(&report.executables, gem, executable);
    // What this run installed, uv-style, on standard error so the
    // command's own output is all that standard output carries, with what
    // `link` warned of. A run that reuses the GEM_HOME prints nothing. The
    // chosen executable runs from its own gem's stub, so which gem `bin/`
    // gave it to is left out.
    let linked = report
        .linked
        .iter()
        .any(|(_, outcome)| matches!(outcome, gem_home::Outcome::Linked));
    if linked {
        let run_from = choice.as_ref().map(|(_, exe)| format!("bin/{exe}: ")).ok();
        for warning in &report.warnings {
            if run_from.as_ref().is_none_or(|p| !warning.starts_with(p)) {
                eprintln!("spinel: x: warning: {warning}");
            }
        }
    }
    for (full_name, outcome) in &report.linked {
        if matches!(outcome, gem_home::Outcome::Linked) {
            match resolved.iter().find(|r| r.full_name() == *full_name) {
                Some(spec) => eprintln!("+ {} {}", spec.name, version_of(spec)),
                None => eprintln!("+ {full_name}"),
            }
        }
    }

    let (owner, chosen) = choice.map_err(Stop::failure)?;
    let stub = stub_for(&gem_home, &owner, &chosen).map_err(Stop::failure)?;
    Ok((gem_home, stub))
}

/// The stub that runs `owner`'s `exe`: the one in `bin/` when it is
/// `owner`'s, as it is unless another gem resolved has an executable of the
/// same name and `link` gave `bin/` to that one. Then `owner`'s own stub,
/// written once to `x-bin/<owner>/<exe>`, where no link looks, so the
/// command's `$0` is still named like the executable.
fn stub_for(gem_home: &Path, owner: &str, exe: &str) -> Result<PathBuf, String> {
    let text = gem_home::app_script_text(spinel_vm::ENGINE, owner, exe);
    let shared = gem_home.join("bin").join(exe);
    if std::fs::read(&shared).is_ok_and(|t| t == text.as_bytes()) {
        return Ok(shared);
    }
    let own = gem_home.join("x-bin").join(owner).join(exe);
    if !std::fs::read(&own).is_ok_and(|t| t == text.as_bytes()) {
        let dir = own.parent().unwrap_or(gem_home);
        std::fs::create_dir_all(dir)
            .and_then(|()| install::write_atomically(&own, text.as_bytes()))
            .map_err(|err| format!("{}: {err}", own.display()))?;
    }
    Ok(own)
}

/// `Gem::Requirement#to_s`: `>= 0` when none was given.
fn shown_requirement(requirement: &gem_requirement::Requirement) -> String {
    requirement
        .requirements()
        .iter()
        .map(|(op, version)| format!("{} {version}", op.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The GEM_HOME's directory under `x/`: the gem asked for and its version,
/// to be read by a person, then the first 16 hex digits of the SHA-256 of
/// every gem's full name and checksum, which is what makes two runs share
/// it. A gem republished under the same version with another checksum is a
/// different GEM_HOME, as it is a different store entry.
fn directory_name(root: &Resolved, wants: &[(gem_store::GemId, String)]) -> String {
    let mut lines: Vec<String> = wants
        .iter()
        .map(|(id, sha)| format!("{} {sha}\n", id.full_name()))
        .collect();
    lines.sort();
    let digest = Sha256::digest(lines.concat().as_bytes());
    let key: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{key}", root.full_name())
}

/// A resolved build's version as `+` lines show it: `1.16.0`, or
/// `1.16.0-x86_64-linux` for a platform build.
fn version_of(spec: &Resolved) -> String {
    match &spec.platform {
        crate::gem_platform::GemPlatform::Ruby => spec.version.to_string(),
        platform => format!("{}-{platform}", spec.version),
    }
}

/// The executable to run, as `gem exec`'s `load!` chooses it among the
/// activated gems (here, every gem resolved): the gems that have an
/// executable named `executable`, narrowed to the one named `executable`
/// too when it is among them. With none, the gem named `executable` (the
/// gem asked for, unless `--gem` named another) and its only executable.
/// Returns the gem the executable is chosen from, and the executable. Its
/// messages are `gem exec`'s.
fn choose_executable(
    executables: &[(String, Vec<String>)],
    gem: &str,
    executable: &str,
) -> Result<(String, String), String> {
    let mut containing: Vec<&str> = executables
        .iter()
        .filter(|(_, exes)| exes.iter().any(|e| e == executable))
        .map(|(name, _)| name.as_str())
        .collect();
    if containing.contains(&executable) {
        containing.retain(|name| *name == executable);
    }
    match containing.as_slice() {
        [owner] => Ok(((*owner).to_owned(), executable.to_owned())),
        [] => {
            let own = executables
                .iter()
                .find(|(name, _)| name == executable)
                .map(|(_, exes)| exes.as_slice())
                .unwrap_or_default();
            match own {
                [] => Err(format!(
                    "failed to load executable `{executable}`, are you sure the gem `{gem}` contains it?"
                )),
                [only] => Ok((executable.to_owned(), only.clone())),
                several => {
                    let mut options = several.to_vec();
                    options.sort();
                    Err(format!(
                        "ambiguous which executable from gem `{executable}` should be run: \
                         the options are {}; name one, with the gem in --gem \
                         (spinel x --gem {executable} {})",
                        options.join(", "),
                        options[0]
                    ))
                }
            }
        }
        several => Err(format!(
            "ambiguous which gem `{executable}` should come from: the options are {}, \
             specify one with --gem",
            several.join(", ")
        )),
    }
}

/// Replace this process with the stub run by this `spinel`, with the
/// GEM_HOME as the whole of GEM_HOME and GEM_PATH and its `bin/` first on
/// PATH, as `spinel run` sets up a project's. The stub is run as `ruby`'s
/// command line runs a script, `spinel -- <stub>`, so the project setup
/// (docs/cli.md, "What `spinel run` sets up") does not put the current
/// directory's gems in place of these.
fn exec(gem_home: &Path, stub: &Path, args: &[String]) -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("spinel: x: cannot find the spinel executable: {err}");
            return ExitCode::FAILURE;
        }
    };
    let bin = gem_home.join("bin");
    let mut path = vec![bin.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&existing).filter(|entry| *entry != bin));
    }
    let path = match std::env::join_paths(path) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("spinel: x: PATH: {err}");
            return ExitCode::FAILURE;
        }
    };
    let err = Command::new(exe)
        .arg("--")
        .arg(stub)
        .args(args)
        .env("GEM_HOME", gem_home)
        .env("GEM_PATH", gem_home)
        .env("PATH", path)
        .exec();
    eprintln!("spinel: x: cannot run {}: {err}", stub.display());
    ExitCode::FAILURE
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_splits_at_its_requirement() {
        assert_eq!(split_command("rubocop"), ("rubocop", vec![]));
        assert_eq!(split_command("rubocop@1.60.0"), ("rubocop", vec!["1.60.0"]));
        assert_eq!(
            split_command("rubocop@~> 1.60, >= 1.60.2"),
            ("rubocop", vec!["~> 1.60", " >= 1.60.2"])
        );
    }

    fn exes(list: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        list.iter()
            .map(|(gem, exes)| {
                (
                    (*gem).to_owned(),
                    exes.iter().map(|e| (*e).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_executable_is_chosen_as_gem_exec_chooses_it() {
        // Named like the gem.
        let set = exes(&[("rubocop", &["rubocop"]), ("parser", &["ruby-parse"])]);
        assert_eq!(
            choose_executable(&set, "rubocop", "rubocop"),
            Ok(("rubocop".into(), "rubocop".into()))
        );
        // From a dependency: `gem exec rspec` runs rspec-core's.
        let set = exes(&[("rspec", &[]), ("rspec-core", &["rspec"])]);
        assert_eq!(
            choose_executable(&set, "rspec", "rspec"),
            Ok(("rspec-core".into(), "rspec".into()))
        );
        // The gem's only executable, named otherwise.
        let set = exes(&[("haml_lint", &["haml-lint"])]);
        assert_eq!(
            choose_executable(&set, "haml_lint", "haml_lint"),
            Ok(("haml_lint".into(), "haml-lint".into()))
        );
        // The gem of the same name wins over another that has it too.
        let set = exes(&[("a", &["a"]), ("b", &["a"])]);
        assert_eq!(
            choose_executable(&set, "a", "a"),
            Ok(("a".into(), "a".into()))
        );
        // `--gem` names the gem the executable comes from.
        let set = exes(&[("pair", &["pair-a", "pair-b"]), ("other", &["pair-a"])]);
        assert_eq!(
            choose_executable(&set, "pair", "pair-b"),
            Ok(("pair".into(), "pair-b".into()))
        );
        // Several, and none named like the gem.
        let set = exes(&[("pair", &["pair-b", "pair-a"])]);
        let err = choose_executable(&set, "pair", "pair").unwrap_err();
        assert!(err.contains("the options are pair-a, pair-b"), "{err}");
        // Two other gems with it.
        let set = exes(&[("x", &[]), ("b", &["run"]), ("c", &["run"])]);
        let err = choose_executable(&set, "x", "run").unwrap_err();
        assert!(err.contains("the options are b, c"), "{err}");
        // None at all.
        let set = exes(&[("lib", &[])]);
        let err = choose_executable(&set, "lib", "lib").unwrap_err();
        assert!(err.contains("the gem `lib` contains it"), "{err}");
    }
}
