//! `spinel install` (#84, docs/package-manager.md, "Pipeline"): the
//! Gemfile evaluated, resolved as Bundler 4 resolves it with the lockfile
//! held (#79), the gems fetched into the global store (#81) and linked into
//! the project's `.spinel/gem_home/` (#82), then `.bundle/config` and the
//! lockfile written. With `--frozen`, a lockfile that would change is
//! Bundler's frozen-mode error, and nothing is resolved or written.
//!
//! Every step is one of the pieces those issues built; this module only
//! puts them in order. `spinel resolve` shares the Gemfile reading
//! ([`prepare`]) and the index reading ([`read_index`]).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::gem_platform::{self, GemPlatform};
use crate::local_source::{self, Declared, DeclaredSource, LocalGem, Materialized};
use crate::lockfile::{LockSource, Lockfile, SourceKind};
use crate::resolver::{self, GemfileDependency, Spec};
use crate::{compact_index, gem_home, gem_requirement, gem_spec, gem_store, gem_version, home};
use crate::{locked, lockfile};

/// Spinel's Gemfile evaluator (docs/package-manager.md, "1. Gemfile
/// evaluation"), run on the VM by a child `spinel`.
const GEMFILE_EVAL: &str = include_str!("../../../shims/gemfile_eval.rb");

/// The exit status of Bundler's `ProductionError`, which frozen mode raises,
/// so a CI script that knows Bundler's status knows Spinel's.
const EXIT_FROZEN: u8 = 16;

/// Where `.bundle/config` points Bundler, relative to the project
/// (docs/cli.md, "Bundler running on Spinel").
const GEM_HOME: &str = ".spinel/gem_home";

/// The parts of `shims/gemfile_eval.rb`'s JSON the resolver and the
/// installer read (docs/package-manager.md, "1. Gemfile evaluation").
#[derive(serde::Deserialize)]
pub(crate) struct GemfileJson {
    pub gemfiles: Vec<String>,
    pub source: Option<GemfileSource>,
    /// The `ruby` directive: whether there is one, and its `versions`, which
    /// decide whether a locked `RUBY VERSION` still holds.
    #[serde(default)]
    pub ruby: Option<serde_json::Value>,
    #[serde(default)]
    pub optional_groups: Vec<String>,
    #[serde(default)]
    pub gemspecs: Vec<serde_json::Value>,
    pub dependencies: Vec<GemfileJsonDependency>,
}

#[derive(serde::Deserialize)]
pub(crate) struct GemfileSource {
    #[serde(rename = "type")]
    pub kind: String,
    pub remote: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct GemfileJsonDependency {
    pub name: String,
    pub requirements: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
    pub source: Option<serde_json::Value>,
    /// The `install_if` conditions, already evaluated.
    #[serde(default = "yes")]
    pub install_if: bool,
    /// The enclosing `env` block's variable, or a Hash of one.
    #[serde(default)]
    pub env: Option<serde_json::Value>,
    pub force_ruby_platform: Option<bool>,
}

fn yes() -> bool {
    true
}

impl GemfileJson {
    /// The `ruby` directive's version requirements, or `None` when there is
    /// no directive.
    pub fn ruby_versions(&self) -> Option<Vec<String>> {
        self.ruby.as_ref().filter(|r| !r.is_null()).map(|r| {
            r.get("versions")
                .and_then(serde_json::Value::as_array)
                .map(|vs| {
                    vs.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        })
    }
}

/// A Gemfile ready to resolve.
pub(crate) struct Prepared {
    /// The global rubygems source, as written; `None` when the Gemfile has
    /// none, its gems all coming from git and path sources.
    pub remote: Option<String>,
    pub request: resolver::Request,
    /// `Platform.local`.
    pub local: gem_platform::Platform,
    /// The git and path sources (#88), in `SourceList#all_sources`' order.
    pub sources: Vec<DeclaredSource>,
    /// For each of the request's dependencies, the source it is pinned to.
    pub pins: Vec<Option<usize>>,
}

/// Read the Gemfile's JSON into a resolver request for `platforms` (the
/// local one when empty, else the first is taken as local), resolving for
/// `ruby_version` and `rubygems_version` (Spinel's own when `None`). An
/// error is a Gemfile Spinel cannot resolve yet, or a bad argument.
pub(crate) fn prepare(
    gemfile: &GemfileJson,
    platforms: &[String],
    ruby_version: Option<&str>,
    rubygems_version: Option<&str>,
) -> Result<Prepared, String> {
    let remote = match &gemfile.source {
        Some(GemfileSource {
            kind,
            remote: Some(remote),
        }) if kind == "rubygems" => Some(remote.clone()),
        None => None,
        _ => return Err("the Gemfile names no rubygems source".to_owned()),
    };
    // ponytail: one global rubygems source beside git and path ones.
    // Upgrade: a rubygems source scoped to some gems, resolved with its own
    // index.
    if !gemfile.gemspecs.is_empty() {
        return Err("`gemspec` is read by `spinel install`".to_owned());
    }
    let local = gem_platform::Platform::local(spinel_vm::PLATFORM);
    let project = gemfile
        .gemfiles
        .first()
        .and_then(|f| Path::new(f).parent())
        .map_or_else(|| PathBuf::from("."), Path::to_owned);
    let declared: Vec<Option<&serde_json::Value>> = gemfile
        .dependencies
        .iter()
        .map(|d| d.source.as_ref().filter(|s| !s.is_null()))
        .collect();
    let names: Vec<&str> = gemfile
        .dependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let (sources, pins) = local_source::declared(&declared, &names, &project)?;
    let mut dependencies = Vec::new();
    for (dep, pin) in gemfile.dependencies.iter().zip(&pins) {
        if pin.is_none() && dep.source.as_ref().is_some_and(|s| !s.is_null()) {
            return Err(format!(
                "{}: a source of its own other than git or path is not supported yet",
                dep.name
            ));
        }
        let requirement =
            gem_requirement::Requirement::new(dep.requirements.iter().map(String::as_str))
                .map_err(|err| format!("{}: {err}", dep.name))?;
        dependencies.push(GemfileDependency {
            name: dep.name.clone(),
            requirement,
            platforms: dep.platforms.clone(),
            force_ruby_platform: dep.force_ruby_platform.unwrap_or(false),
        });
    }
    let platforms: Vec<GemPlatform> = if platforms.is_empty() {
        vec![GemPlatform::Platform(local.clone())]
    } else {
        platforms
            .iter()
            .map(|p| GemPlatform::new(Some(p), &local))
            .collect()
    };
    let version =
        |given: Option<&str>, default: &str| gem_version::Version::new(given.unwrap_or(default));
    let ruby_version =
        version(ruby_version, spinel_vm::LANGUAGE_VERSION).map_err(|e| e.to_string())?;
    let rubygems_version =
        version(rubygems_version, gem_spec::RUBYGEMS_VERSION).map_err(|e| e.to_string())?;
    let gemfile_name = gemfile
        .gemfiles
        .first()
        .and_then(|f| Path::new(f).file_name())
        .map_or_else(
            || "Gemfile".to_owned(),
            |f| f.to_string_lossy().into_owned(),
        );
    // Bundler names a rubygems source by its remote with a trailing `/`.
    let shown_source = match &remote {
        Some(remote) if remote.ends_with('/') => format!("rubygems repository {remote}"),
        Some(remote) => format!("rubygems repository {remote}/"),
        None => "locally installed gems".to_owned(),
    };
    Ok(Prepared {
        request: resolver::Request {
            gemfile: gemfile_name,
            source: shown_source,
            dependencies,
            platforms,
            ruby_version,
            rubygems_version,
        },
        remote,
        local,
        sources,
        pins,
    })
}

/// The builds of every gem `roots` can reach through any version's
/// dependencies, as Bundler reads them before it resolves: `/versions`, then
/// `/info` of the roots, then of every gem those name, and so on, each round
/// fetched at once. A gem `/versions` does not list has no builds. `bundler`
/// is never read: it resolves to the Bundler the lockfile is written for.
pub(crate) async fn read_index(
    client: &compact_index::Client,
    roots: Vec<String>,
    local: &gem_platform::Platform,
) -> Result<resolver::Index, Box<dyn std::error::Error>> {
    read_index_over(Some(client), roots, local, resolver::Index::new()).await
}

/// [`read_index`] with the builds of some gems already known: those of the
/// git and path sources (`overlay`), which the index is not read for, and
/// whose dependencies it is. With no `client`, a Gemfile with no rubygems
/// source, a gem they do not provide has no builds.
pub(crate) async fn read_index_over(
    client: Option<&compact_index::Client>,
    roots: Vec<String>,
    local: &gem_platform::Platform,
    overlay: resolver::Index,
) -> Result<resolver::Index, Box<dyn std::error::Error>> {
    let mut index = overlay;
    let mut roots = roots;
    for specs in index.values() {
        for dep in resolver::dependency_names(specs) {
            if !roots.iter().any(|r| r == dep) {
                roots.push(dep.to_owned());
            }
        }
    }
    let mut wanted: Vec<String> = roots
        .into_iter()
        .filter(|r| r != "bundler" && !index.contains_key(r))
        .collect();
    // Only git and path gems, needing nothing from the index: it is not
    // read at all.
    if wanted.is_empty() && !index.is_empty() {
        return Ok(index);
    }
    let Some(client) = client else {
        for gem in wanted {
            index.insert(gem, Vec::new());
        }
        return Ok(index);
    };
    client.versions().await?;
    while !wanted.is_empty() {
        let mut next = Vec::new();
        for (gem, result) in client.info(&wanted).await? {
            let specs = match result? {
                compact_index::Outcome::Unknown => Vec::new(),
                _ => {
                    let path = client.info_path(&gem);
                    let data =
                        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    resolver::parse_info(&gem, &data, local)?
                }
            };
            for dep in resolver::dependency_names(&specs) {
                if dep != "bundler"
                    && !index.contains_key(dep)
                    && !next.iter().any(|n: &String| n == dep)
                {
                    next.push(dep.to_owned());
                }
            }
            index.insert(gem, specs);
        }
        next.retain(|gem| !index.contains_key(gem));
        wanted = next;
    }
    Ok(index)
}

/// Why `spinel install` (or `add`, or `remove`) stopped: the exit status,
/// after what to say was printed.
pub(crate) struct Stop(pub ExitCode);

impl Stop {
    /// `spinel: <command>: <message>`, exit 1.
    pub(crate) fn failure(command: &str, message: impl std::fmt::Display) -> Stop {
        eprintln!("spinel: {command}: {message}");
        Stop(ExitCode::FAILURE)
    }
}

/// `spinel install [--frozen]`.
pub fn run(frozen: bool) -> ExitCode {
    let install = || {
        let gemfile = evaluate_gemfile("install", None, None)?;
        let mut session = Session::new("install", &gemfile, frozen)?;
        let resolved = session.resolve()?;
        session.install(&gemfile, resolved, None)
    };
    match install() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop(code)) => code,
    }
}

/// A new Gemfile `spinel add` or `spinel remove` writes once everything it
/// installs is in place, just before the lockfile, and puts back if the
/// lockfile cannot be written.
pub(crate) struct GemfileWrite<'a> {
    pub old: &'a [u8],
    pub new: &'a [u8],
}

/// What a resolution leaves: the lockfile to write, and the index when it
/// was read.
pub(crate) struct Resolved {
    pub written: String,
    index: Option<resolver::Index>,
}

/// The git and path sources a resolution read: what each has, and the
/// sources as the lockfile writes them.
#[derive(Default)]
struct LocalState {
    read: Vec<Materialized>,
    /// Which source each gem a git or path source provides comes from.
    providers: Vec<(String, usize)>,
    lock_sources: Vec<LockSource>,
    /// Git sources whose locked revision was let go (`unlock!`).
    unlocked: Vec<bool>,
}

/// One evaluated Gemfile on its way to the GEM_HOME: where its project and
/// lockfile are, the resolver's request, and the index client.
pub(crate) struct Session {
    /// The command whose messages these are: `install`, `add`, `remove` or
    /// `update`.
    command: &'static str,
    frozen: bool,
    /// What `spinel update` unlocks; nothing for the others.
    unlock: locked::Unlock,
    gemfile_path: PathBuf,
    root: PathBuf,
    lock_path: PathBuf,
    lock_name: String,
    prepared: Prepared,
    ruby: Option<Vec<String>>,
    /// The lockfile as it is on disk, if there is one.
    current: Option<String>,
    home: PathBuf,
    /// The rubygems source's index; `None` when the Gemfile has none.
    client: Option<compact_index::Client>,
    runtime: tokio::runtime::Runtime,
    /// What the git and path sources had, once resolved.
    locals: LocalState,
}

impl Session {
    pub(crate) fn new(
        command: &'static str,
        gemfile: &GemfileJson,
        frozen: bool,
    ) -> Result<Session, Stop> {
        let fail = |message: String| Stop::failure(command, message);
        let gemfile_path = gemfile
            .gemfiles
            .first()
            .map(PathBuf::from)
            .ok_or_else(|| fail("the Gemfile evaluator named no Gemfile".to_owned()))?;
        let root = gemfile_path
            .parent()
            .map(Path::to_owned)
            .unwrap_or_else(|| PathBuf::from("."));
        let lock_path = lockfile_path(&gemfile_path);
        let lock_name = relative_to_cwd(&lock_path);
        let prepared = prepare(gemfile, &[], None, None).map_err(|message| {
            eprintln!("spinel: {command}: {message}");
            Stop(ExitCode::from(2))
        })?;

        let current = match std::fs::read_to_string(&lock_path) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(fail(format!("{lock_name}: {err}"))),
        };
        if frozen && current.is_none() {
            eprintln!("Frozen mode is set, but there's no lockfile");
            return Err(Stop(ExitCode::from(EXIT_FROZEN)));
        }

        let Some(home) = home::spinel_home() else {
            return Err(fail("neither SPINEL_HOME nor HOME is set".to_owned()));
        };
        let client = prepared
            .remote
            .as_deref()
            .map(|remote| compact_index::Client::new(remote, &home))
            .transpose()
            .map_err(|err| fail(err.to_string()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| fail(format!("cannot start the async runtime: {err}")))?;
        Ok(Session {
            command,
            frozen,
            unlock: locked::Unlock::Nothing,
            gemfile_path,
            root,
            lock_path,
            lock_name,
            prepared,
            ruby: gemfile.ruby_versions(),
            current,
            home,
            client,
            runtime,
            locals: LocalState::default(),
        })
    }

    fn fail(&self, message: impl std::fmt::Display) -> Stop {
        Stop::failure(self.command, message)
    }

    /// The Gemfile this session read, as the evaluator found it.
    pub(crate) fn gemfile_path(&self) -> &Path {
        &self.gemfile_path
    }

    /// Resolve as an update that unlocks `unlock` (`spinel update`, #87).
    pub(crate) fn set_unlock(&mut self, unlock: locked::Unlock) {
        self.unlock = unlock;
    }

    /// The lockfile as it was on disk when the session began, read, or
    /// `None` when there is none.
    pub(crate) fn current_lockfile(&self) -> Result<Option<Lockfile>, Stop> {
        let Some(text) = &self.current else {
            return Ok(None);
        };
        Lockfile::parse(text, &self.lock_name, &self.prepared.local)
            .map(Some)
            .map_err(|err| {
                eprintln!("{err}");
                Stop(ExitCode::FAILURE)
            })
    }

    /// The lockfile a resolution writes, read back.
    pub(crate) fn written_lockfile(&self, resolved: &Resolved) -> Result<Lockfile, Stop> {
        self.read_written(&resolved.written)
            .map(|(lock, _, _)| lock)
    }

    fn read_index(&self) -> Result<resolver::Index, Stop> {
        let roots: Vec<String> = self
            .prepared
            .request
            .dependencies
            .iter()
            .map(|d| d.name.clone())
            .collect();
        // The gems the git and path sources provide come only from them.
        // ponytail: a conflict's explanation names the rubygems source for
        // them too. Upgrade with each package's source in the resolver.
        let mut overlay = resolver::Index::new();
        for (name, i) in &self.locals.providers {
            let specs: Vec<Spec> = self.locals.read[*i]
                .gems
                .iter()
                .filter(|g| &g.spec.name == name)
                .map(|g| g.spec.clone())
                .collect();
            overlay.insert(name.clone(), specs);
        }
        self.runtime
            .block_on(read_index_over(
                self.client.as_ref(),
                roots,
                &self.prepared.local,
                overlay,
            ))
            .map_err(|err| self.fail(err))
    }

    /// Read the path sources, where they are, as `Definition` compares them
    /// with the lockfile before anything resolves.
    fn read_paths(&self) -> Result<Vec<Option<Materialized>>, Stop> {
        let paths: Vec<&Declared> = self
            .prepared
            .sources
            .iter()
            .map(|s| &s.kind)
            .filter(|k| matches!(k, Declared::Path { .. }))
            .collect();
        let mut read = local_source::read_paths(&paths, &self.prepared.local)
            .map_err(|message| self.gemspec_failure(message))?
            .into_iter();
        Ok(self
            .prepared
            .sources
            .iter()
            .map(|s| match s.kind {
                Declared::Path { .. } => read.next(),
                Declared::Git { .. } => None,
            })
            .collect())
    }

    /// A git or path source that cannot be read: Bundler's message alone,
    /// as it prints one, exit 1.
    fn gemspec_failure(&self, message: String) -> Stop {
        if !message.is_empty() {
            eprintln!("{message}");
        }
        Stop(ExitCode::FAILURE)
    }

    /// Read every git source at the revision `revisions` holds it to (a new
    /// one where that is `None`), with the path sources already `read`;
    /// work out which gems each provides, and how the lockfile writes them.
    fn read_locals(
        &mut self,
        read: Vec<Option<Materialized>>,
        revisions: &[Option<String>],
        definition: Option<&locked::Definition>,
    ) -> Result<(), Stop> {
        let mut all = Vec::new();
        for (i, (source, read)) in self.prepared.sources.iter().zip(read).enumerate() {
            let materialized = match read {
                Some(read) => read,
                None => local_source::read_git(
                    &self.home,
                    &source.kind,
                    revisions.get(i).and_then(Option::as_deref),
                    &self.prepared.local,
                )
                .map_err(|message| self.gemspec_failure(message))?,
            };
            all.push(materialized);
        }
        let shown_revisions: Vec<Option<String>> = all.iter().map(|m| m.revision.clone()).collect();
        let providers = local_source::providers(&self.prepared.sources, &all, &shown_revisions)
            .map_err(|message| self.gemspec_failure(message))?;
        // A gem pinned to a source that does not have it.
        for (i, source) in self.prepared.sources.iter().enumerate() {
            for name in &source.pinned {
                if !all[i].gems.iter().any(|g| &g.spec.name == name) {
                    let described = source.kind.describe(all[i].revision.as_deref());
                    return Err(self.gemspec_failure(format!(
                        "Could not find gem '{name}' in {described}.\n\n\
                         The source does not contain any versions of '{name}'"
                    )));
                }
            }
        }
        let mut lock_sources = Vec::new();
        let mut unlocked = Vec::new();
        for (i, source) in self.prepared.sources.iter().enumerate() {
            let held = definition.and_then(|d| d.locked_revision(i));
            let is_git = matches!(source.kind, Declared::Git { .. });
            unlocked.push(is_git && held.is_none());
            lock_sources.push(LockSource {
                kind: source.kind.to_lock(all[i].revision.as_deref()),
                identifier: source.kind.identifier(held),
                names: providers
                    .iter()
                    .filter(|(_, at)| *at == i)
                    .map(|(n, _)| n.clone())
                    .collect(),
                pinned: source.pinned.clone(),
            });
        }
        self.locals = LocalState {
            read: all,
            providers,
            lock_sources,
            unlocked,
        };
        Ok(())
    }

    /// Whether `name` comes from a git source whose locked revision this
    /// resolution let go, so its source is not the one it was locked from.
    pub(crate) fn git_source_unlocked(&self, name: &str) -> bool {
        self.locals
            .providers
            .iter()
            .any(|(n, i)| n == name && self.locals.unlocked.get(*i).copied().unwrap_or(false))
    }

    /// Resolve, keeping to the lockfile when there is one (#79), into the
    /// lockfile Bundler would write. Nothing is written.
    pub(crate) fn resolve(&mut self) -> Result<Resolved, Stop> {
        let local = GemPlatform::Platform(self.prepared.local.clone());
        let paths = self.read_paths()?;
        let Some(text) = self.current.clone() else {
            self.read_locals(paths, &[], None)?;
            let index = self.read_index()?;
            let resolved =
                resolver::resolve(&self.prepared.request, &index).map_err(|failure| {
                    eprintln!("{}", failure.to_string().trim_end());
                    Stop(ExitCode::FAILURE)
                })?;
            let lock = Lockfile::from_resolution(
                self.prepared.remote.as_deref().unwrap_or_default(),
                &self.prepared.request,
                &index,
                &resolved,
                self.ruby.is_some(),
                &self.locals.lock_sources,
            );
            return Ok(Resolved {
                written: lock.write(),
                index: Some(index),
            });
        };
        let lock =
            Lockfile::parse(&text, &self.lock_name, &self.prepared.local).map_err(|err| {
                eprintln!("{err}");
                Stop(ExitCode::FAILURE)
            })?;
        let mut definition = locked::Definition::with_locals(
            &lock,
            self.prepared.remote.as_deref().unwrap_or_default(),
            &self.prepared.request.dependencies,
            self.ruby.as_deref(),
            &local,
            &[],
            &self.unlock,
            locked::Locals {
                sources: &self.prepared.sources,
                read: &paths,
                pins: &self.prepared.pins,
                project: &self.root,
            },
        )
        .map_err(|err| {
            eprintln!("{err}");
            Stop(ExitCode::FAILURE)
        })?;
        if self.frozen
            && let Some(message) = definition.frozen_summary(
                &lock,
                &self.prepared.request.dependencies,
                &self.lock_name,
            )
        {
            eprint!("{message}");
            if !message.ends_with('\n') {
                eprintln!();
            }
            return Err(Stop(ExitCode::from(EXIT_FROZEN)));
        }
        self.prepared.request.platforms = definition.platforms().to_vec();
        let revisions: Vec<Option<String>> = (0..self.prepared.sources.len())
            .map(|i| definition.locked_revision(i).map(str::to_owned))
            .collect();
        self.read_locals(paths, &revisions, Some(&definition))?;
        if definition.resolve_needed() {
            let index = self.read_index()?;
            let specs = definition
                .start_resolution(&self.prepared.request, &index)
                .map_err(|failure| {
                    eprintln!("{}", failure.to_string().trim_end());
                    Stop(ExitCode::FAILURE)
                })?;
            let written = definition.write_lock(
                &text,
                self.prepared.remote.as_deref().unwrap_or_default(),
                &self.prepared.request,
                Some(&index),
                &specs,
                &self.locals.lock_sources,
            );
            Ok(Resolved {
                written,
                index: Some(index),
            })
        } else {
            let specs = definition.unchanged(&self.prepared.request.dependencies);
            let written = definition.write_lock(
                &text,
                self.prepared.remote.as_deref().unwrap_or_default(),
                &self.prepared.request,
                None,
                &specs,
                &self.locals.lock_sources,
            );
            Ok(Resolved {
                written,
                index: None,
            })
        }
    }

    /// The specs a resolution locks, every platform's build, from every
    /// source.
    pub(crate) fn locked_specs(&self, resolved: &Resolved) -> Result<Vec<Spec>, Stop> {
        self.read_written(&resolved.written)
            .map(|(_, specs, _)| specs)
    }

    /// The lockfile a resolution writes, read back, its specs, and the
    /// full names of those locked from a git or path source.
    fn read_written(&self, written: &str) -> Result<(Lockfile, Vec<Spec>, HashSet<String>), Stop> {
        let lock = Lockfile::parse(written, &self.lock_name, &self.prepared.local)
            .map_err(|err| self.fail(format!("the lockfile written does not read back: {err}")))?;
        let specs = lock
            .sources
            .iter()
            .flat_map(|s| s.specs.iter().map(locked::locked_spec))
            .collect();
        let local = lock
            .sources
            .iter()
            .filter(|s| matches!(s.kind, SourceKind::Git { .. } | SourceKind::Path { .. }))
            .flat_map(|s| s.specs.iter().map(lockfile::LockedSpec::full_name))
            .collect();
        Ok((lock, specs, local))
    }

    /// The gem a git or path source has for a spec the lockfile locks from
    /// it.
    fn local_gem(&self, spec: &Spec) -> Option<&LocalGem> {
        let full_name = spec.full_name();
        self.locals
            .providers
            .iter()
            .filter(|(n, _)| *n == spec.name)
            .flat_map(|(_, i)| &self.locals.read[*i].gems)
            .find(|g| g.full_name() == full_name)
    }

    /// Fetch what `resolved` locks for this machine into the store, link it
    /// into the GEM_HOME, and write `.bundle/config`, then `gemfile_write`'s
    /// Gemfile, then the lockfile; print what changed.
    pub(crate) fn install(
        &self,
        gemfile: &GemfileJson,
        resolved: Resolved,
        gemfile_write: Option<GemfileWrite<'_>>,
    ) -> Result<(), Stop> {
        let Resolved { written, index } = resolved;
        let prepared = &self.prepared;
        let lock_name = &self.lock_name;
        let local = GemPlatform::Platform(prepared.local.clone());

        // What this machine installs: the lockfile's builds for the local
        // platform, from the dependencies Bundler installs here.
        let (lock, specs, local_names) = self.read_written(&written)?;
        let wanted = installed_dependencies(gemfile, &prepared.request.dependencies, &local);
        let mut to_install = lockfile::spec_set_for(&specs, &wanted, std::slice::from_ref(&local));
        to_install.sort_by_cached_key(Spec::full_name);
        // A git or path gem is linked where its source has it; the rest come
        // from the store.
        let (local_specs, to_fetch): (Vec<Spec>, Vec<Spec>) = to_install
            .iter()
            .cloned()
            .partition(|s| local_names.contains(&s.full_name()));
        let mut local_links = Vec::new();
        for spec in &local_specs {
            let Some(gem) = self.local_gem(spec) else {
                return Err(self.fail(format!(
                    "{}: its git or path source does not have it",
                    spec.full_name()
                )));
            };
            local_links.push(gem_home::LocalLink {
                full_name: gem.full_name(),
                name: gem.spec.name.clone(),
                gem_dir: gem.gem_dir.clone(),
                gemspec: gem.gemspec.clone(),
                require_paths: gem.require_paths.clone(),
                bindir: gem.bindir.clone(),
                executables: gem.executables.clone(),
                extensions: gem.extensions.clone(),
            });
        }
        // Where Bundler looks for each git source's checkout.
        let checkouts: Vec<(String, PathBuf)> = self
            .prepared
            .sources
            .iter()
            .zip(&self.locals.read)
            .filter_map(
                |(source, read)| match (&source.kind, &read.revision, &read.checkout) {
                    (Declared::Git { uri, .. }, Some(revision), Some(checkout)) => Some((
                        local_source::bundler_checkout_name(uri, revision),
                        checkout.clone(),
                    )),
                    _ => None,
                },
            )
            .collect();
        let to_install = to_fetch;

        // Each one's checksum: the lockfile's, else the index's.
        let mut sums: HashMap<String, String> = lock
            .checksums
            .iter()
            .flatten()
            .filter_map(|c| {
                let sha = c.checksums.iter().find(|(algo, _)| algo == "sha256")?;
                Some((c.lock_name.clone(), sha.1.clone()))
            })
            .collect();
        let lock_name_of = |s: &Spec| lockfile::lock_name(&s.name, &s.version, &s.platform);
        if let Some(index) = &index {
            for spec in &to_install {
                let found = index.get(&spec.name).and_then(|builds| {
                    builds
                        .iter()
                        .find(|b| b.version == spec.version && b.platform == spec.platform)
                });
                if let Some(sha) = found.and_then(|b| b.checksum.clone()) {
                    sums.entry(lock_name_of(spec)).or_insert(sha);
                }
            }
        }
        let unsummed: Vec<String> = to_install
            .iter()
            .filter(|s| !sums.contains_key(&lock_name_of(s)))
            .map(|s| s.name.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        // A Gemfile with no rubygems source has nothing to fetch, unless its
        // lockfile names gems from one.
        let mut stored = Vec::new();
        let mut ok = true;
        match &self.client {
            None if to_install.is_empty() => {}
            None => return Err(self.fail("the Gemfile names no rubygems source")),
            Some(client) => {
                let store = gem_store::Store::new(&self.home);
                let mut wants = Vec::new();
                self.runtime.block_on(async {
                    if !unsummed.is_empty() {
                        // A lockfile without `CHECKSUMS`, and nothing resolved: the
                        // index's `/info` lines have them.
                        let fetched = match client.versions().await {
                            Ok(_) => client.info(&unsummed).await.map(|_| ()),
                            Err(err) => Err(err),
                        };
                        if let Err(err) = fetched {
                            self.fail(err);
                            ok = false;
                            return;
                        }
                    }
                    for spec in &to_install {
                        let platform = spec.platform.to_string();
                        let gem =
                            match gem_store::GemId::new(&spec.name, &spec.version.to_string(), &platform) {
                                Ok(gem) => gem,
                                Err(err) => {
                                    self.fail(err);
                                    ok = false;
                                    continue;
                                }
                            };
                        let sha = sums.get(&lock_name_of(spec)).cloned().or_else(|| {
                            let info = std::fs::read(client.info_path(&spec.name)).unwrap_or_default();
                            compact_index::version_checksum(&info, &gem.index_version()).flatten()
                        });
                        match sha {
                            Some(sha) => wants.push((gem, sha)),
                            None => {
                                // ponytail: a gem with no checksum anywhere is
                                // refused, since the store is keyed by it. Upgrade:
                                // hash the `.gem` as it arrives and key the entry by
                                // that.
                                self.fail(format!(
                                    "{}: neither {lock_name} nor {} lists a checksum for it",
                                    gem.full_name(),
                                    prepared.remote.as_deref().unwrap_or_default()
                                ));
                                ok = false;
                            }
                        }
                    }
                });
                if !ok {
                    return Err(Stop(ExitCode::FAILURE));
                }

                // Fetch what the store does not have, then link it all.
                for (gem, result) in self.runtime.block_on(store.fetch(client, &wants)) {
                    match result {
                        Ok(fetched) => {
                            for warning in fetched.warnings {
                                eprintln!("spinel: {}: warning: {warning}", self.command);
                            }
                            stored.push((gem, fetched.path));
                        }
                        Err(err) => {
                            self.fail(err);
                            ok = false;
                        }
                    }
                }
            }
        }
        if !ok {
            return Err(Stop(ExitCode::FAILURE));
        }
        let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_owned());
        let gem_home_dir = absolute(&self.root.join(GEM_HOME));
        let before = linked_gems(&gem_home_dir);
        let stored: Vec<_> = stored
            .into_iter()
            .map(|(gem, entry)| (gem, absolute(&entry)))
            .collect();
        let report = gem_home::link_all(&gem_home_dir, &stored, &local_links)
            .map_err(|err| self.fail(err))?;
        gem_home::link_checkouts(&gem_home_dir, &checkouts).map_err(|err| self.fail(err))?;
        for warning in &report.warnings {
            eprintln!("spinel: {}: warning: {warning}", self.command);
        }

        // Post-install (docs/package-manager.md, "7. Post-install").
        // ponytail: the bytecode cache is not warmed; a first `require`
        // compiles each file. Upgrade: warm it here, from the files the map
        // lists.
        // ponytail: no gem is substituted, so `.spinel/substitutions.json` is
        // not written. Upgrade: built-in gems are #83, Spinel-native ones
        // phase 5.
        for scope in bundler_scopes() {
            for warning in
                gem_home::scope_view(&gem_home_dir, &scope).map_err(|err| self.fail(err))?
            {
                eprintln!("spinel: {}: warning: {warning}", self.command);
            }
        }
        // The require map (#66), written every time: it is what the GEM_HOME
        // and the standard library hold now.
        let mut roots = report.require_dirs.clone();
        roots.extend(crate::stdlib::directory());
        let map = self
            .root
            .join(".spinel")
            .join(spinel_vm::require_map::FILE_NAME);
        // A map that cannot be written is a slower run, never a wrong one: an
        // old map still in place is checked against the disk as it is read.
        if let Err(err) = spinel_vm::require_map::write(&map, &roots) {
            eprintln!(
                "spinel: {}: warning: {}: {err}",
                self.command,
                map.display()
            );
        }
        write_bundle_config(&self.root)
            .map_err(|err| self.fail(format!(".bundle/config: {err}")))?;
        let gemfile_name = relative_to_cwd(&self.gemfile_path);
        if let Some(write) = &gemfile_write {
            write_in_place(&self.gemfile_path, write.new)
                .map_err(|err| self.fail(format!("{gemfile_name}: {err}")))?;
        }
        if !self.frozen && self.current.as_deref() != Some(written.as_str()) {
            write_atomically(&self.lock_path, written.as_bytes()).map_err(|err| {
                // The Gemfile goes back as it was, so the two still agree.
                if let Some(write) = &gemfile_write
                    && let Err(err) = write_in_place(&self.gemfile_path, write.old)
                {
                    self.fail(format!("{gemfile_name}: cannot put it back: {err}"));
                }
                self.fail(format!("{lock_name}: {err}"))
            })?;
        }

        // What changed, uv-style.
        let mut lines: Vec<(String, String)> = Vec::new();
        for spec in to_install.iter().chain(&local_specs) {
            if !before.contains(&spec.full_name()) {
                lines.push((
                    spec.name.clone(),
                    format!("+ {} {}", spec.name, version_of(spec)),
                ));
            }
        }
        let previous: HashMap<String, (String, String)> = self
            .current
            .as_deref()
            .and_then(|text| Lockfile::parse(text, lock_name, &prepared.local).ok())
            .map(|lock| {
                lock.sources
                    .iter()
                    .flat_map(|s| &s.specs)
                    .map(|s| {
                        let spec = locked::locked_spec(s);
                        (spec.full_name(), (spec.name.clone(), version_of(&spec)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for full_name in &report.removed {
            let (name, version) = previous
                .get(full_name)
                .cloned()
                .unwrap_or_else(|| split_full_name(full_name));
            lines.push((name.clone(), format!("- {name} {version}")));
        }
        lines.sort();
        for (_, line) in lines {
            println!("{line}");
        }
        Ok(())
    }
}

/// Evaluate the Gemfile as Ruby, in a child `spinel` running
/// `shims/gemfile_eval.rb`, and read the JSON it prints. Its errors are
/// Bundler's, on standard error, as it prints them. `contents` is read in
/// place of the Gemfile's own, and `injected` evaluated after it as Bundler's
/// `Injector` evaluates the lines `bundle add` adds (`spinel add`).
pub(crate) fn evaluate_gemfile(
    command: &str,
    contents: Option<&[u8]>,
    injected: Option<&str>,
) -> Result<GemfileJson, Stop> {
    let exe = std::env::current_exe().map_err(|err| {
        Stop::failure(command, format!("cannot find the spinel executable: {err}"))
    })?;
    let mut child = Command::new(exe)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| Stop::failure(command, format!("cannot evaluate the Gemfile: {err}")))?;
    let mut script = String::new();
    if let Some(contents) = contents {
        script.push_str(&format!(
            "$spinel_gemfile_contents = String.new({}, encoding: \"UTF-8\")\n",
            crate::gemfile_edit::dump_bytes(contents)
        ));
    }
    if let Some(injected) = injected {
        script.push_str(&format!(
            "$spinel_injected_gems = String.new({}, encoding: \"UTF-8\")\n",
            crate::gemfile_edit::dump_bytes(injected.as_bytes())
        ));
    }
    script.push_str(GEMFILE_EVAL);
    if let Some(mut stdin) = child.stdin.take() {
        // A child that exits early closes its end; its status says why.
        let _ = stdin.write_all(script.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|err| Stop::failure(command, format!("cannot evaluate the Gemfile: {err}")))?;
    if !out.status.success() {
        return Err(Stop(ExitCode::FAILURE));
    }
    let mut gemfile: GemfileJson = serde_json::from_slice(&out.stdout).map_err(|err| {
        Stop::failure(
            command,
            format!("the Gemfile evaluator printed no JSON: {err}"),
        )
    })?;
    expand_gemspecs(&mut gemfile)?;
    Ok(gemfile)
}

/// What Bundler's `Dsl#gemspec` adds for each `gemspec` directive, once its
/// gemspec is evaluated: the gem itself, from a path source at the
/// directive's path, and its development dependencies in the directive's
/// development group. A gem the Gemfile lists itself keeps its own line.
// ponytail: these come after the Gemfile's own gems, where Bundler adds them
// where the directive is, which can change the order the resolver meets
// them in. Upgrade by having the evaluator mark the directive's place.
fn expand_gemspecs(gemfile: &mut GemfileJson) -> Result<(), Stop> {
    let local = gem_platform::Platform::local(spinel_vm::PLATFORM);
    for directive in std::mem::take(&mut gemfile.gemspecs) {
        let text = |key: &str| directive.get(key).and_then(serde_json::Value::as_str);
        let path = text("path").unwrap_or(".");
        let root = text("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
        let group = text("development_group").unwrap_or("development");
        let dir = local_source::expand(path, &root);
        let gem = local_source::read_gemspec(&dir, text("name"), &local).map_err(|message| {
            eprintln!("{message}");
            Stop(ExitCode::FAILURE)
        })?;
        let listed =
            |gemfile: &GemfileJson, name: &str| gemfile.dependencies.iter().any(|d| d.name == name);
        let dependency =
            |name: &str, requirement: &gem_requirement::Requirement, group: &str, source| {
                let mut requirements: Vec<String> = requirement
                    .requirements()
                    .iter()
                    .map(|(op, version)| format!("{} {version}", op.as_str()))
                    .collect();
                if requirements.is_empty() {
                    requirements.push(">= 0".to_owned());
                }
                GemfileJsonDependency {
                    name: name.to_owned(),
                    requirements,
                    groups: vec![group.to_owned()],
                    platforms: Vec::new(),
                    source,
                    install_if: true,
                    env: None,
                    force_ruby_platform: None,
                }
            };
        if !listed(gemfile, &gem.spec.name) {
            let source = serde_json::json!({
                "type": "path",
                "path": path,
                "root": root.display().to_string(),
                "glob": directive.get("glob").cloned().unwrap_or(serde_json::Value::Null),
            });
            let any = gem_requirement::Requirement::default();
            gemfile
                .dependencies
                .push(dependency(&gem.spec.name, &any, "default", Some(source)));
        }
        for (name, requirement) in &gem.development {
            if !listed(gemfile, name) {
                gemfile
                    .dependencies
                    .push(dependency(name, requirement, group, None));
            }
        }
    }
    Ok(())
}

/// `SharedHelpers.default_lockfile`: `gems.locked` beside a `gems.rb`,
/// else the Gemfile's name with `.lock`.
fn lockfile_path(gemfile: &Path) -> PathBuf {
    let name = gemfile
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Gemfile".to_owned());
    if name == "gems.rb" {
        gemfile.with_file_name("gems.locked")
    } else {
        gemfile.with_file_name(format!("{name}.lock"))
    }
}

/// `path` relative to the current directory when it is under it, as
/// Bundler's `relative_lockfile_path` names a lockfile in its messages.
fn relative_to_cwd(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_owned))
        .unwrap_or_else(|| path.to_owned())
        .display()
        .to_string()
}

/// The dependencies `Definition#requested_dependencies` installs here: those
/// on this platform (`relevant_deps?`, `current_platform?`), whose
/// `install_if` held and whose `env` is set, outside groups that are only
/// optional ones.
// ponytail: every group but the optional ones, as Spinel reads no Bundler
// settings. Upgrade: `--without` and `--with` once the CLI takes them.
fn installed_dependencies(
    gemfile: &GemfileJson,
    dependencies: &[GemfileDependency],
    local: &GemPlatform,
) -> Vec<GemfileDependency> {
    let generic = [gem_platform::generic(local)];
    dependencies
        .iter()
        .zip(&gemfile.dependencies)
        .filter(|(dep, json)| {
            !dep.gem_platforms(&generic).is_empty()
                && dep.current_platform()
                && json.install_if
                && env_holds(json.env.as_ref())
                && !(!json.groups.is_empty()
                    && json
                        .groups
                        .iter()
                        .all(|g| gemfile.optional_groups.contains(g)))
        })
        .map(|(dep, _)| dep.clone())
        .collect()
}

/// `Dependency#current_env?`: an `env "NAME"` block holds when the variable
/// is set, an `env "NAME" => value` one when it is set to that value (or
/// matches that Regexp, written as its `inspect`).
fn env_holds(env: Option<&serde_json::Value>) -> bool {
    match env {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(name)) => std::env::var_os(name).is_some(),
        Some(serde_json::Value::Object(pairs)) => pairs.iter().all(|(name, want)| {
            let Ok(value) = std::env::var(name) else {
                return false;
            };
            match want.as_str() {
                // ponytail: a Regexp is matched as the text between its
                // slashes, with no flags. Upgrade with a Ruby regexp engine
                // on this side, or by evaluating `env` in the evaluator.
                Some(pattern) if pattern.starts_with('/') && pattern.len() > 1 => {
                    let inner = pattern[1..].rsplit_once('/').map_or("", |(p, _)| p);
                    value.contains(inner)
                }
                Some(want) => value == want,
                None => true,
            }
        }),
        Some(_) => true,
    }
}

/// The full names of the gems a GEM_HOME has a gemspec for.
fn linked_gems(gem_home: &Path) -> HashSet<String> {
    std::fs::read_dir(gem_home.join("specifications"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".gemspec")
                .map(str::to_owned)
        })
        .collect()
}

/// A spec's version as a lockfile line has it: `1.16.0`, or
/// `1.16.0-x86_64-linux` for a platform build.
fn version_of(spec: &Spec) -> String {
    match &spec.platform {
        GemPlatform::Ruby => spec.version.to_string(),
        platform => format!("{}-{platform}", spec.version),
    }
}

/// A full name split at its version, for a gem no lockfile names: the
/// version starts at the first `-` followed by a digit.
fn split_full_name(full_name: &str) -> (String, String) {
    let bytes = full_name.as_bytes();
    (1..bytes.len())
        .find(|&i| bytes[i - 1] == b'-' && bytes[i].is_ascii_digit())
        .map_or_else(
            || (full_name.to_owned(), String::new()),
            |i| (full_name[..i - 1].to_owned(), full_name[i..].to_owned()),
        )
}

/// Where under the GEM_HOME Bundler looks for gems once `.bundle/config`
/// names it as `BUNDLE_PATH`: `Bundler.ruby_scope`, the engine and
/// `RbConfig::CONFIG["ruby_version"]` (the language version with its teeny
/// set to 0). `ruby` is CRuby's Bundler, the plain `bundle exec` in the same
/// project; `spinel` is Bundler running on Spinel, as `require
/// "bundler/setup"` under `spinel run` runs it.
fn bundler_scopes() -> [String; 2] {
    let mut parts = spinel_vm::LANGUAGE_VERSION.split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    let ruby_version = format!("{major}.{minor}.0");
    ["ruby", spinel_vm::ENGINE].map(|engine| format!("{engine}/{ruby_version}"))
}

/// Point Bundler at the GEM_HOME: `BUNDLE_PATH` in the project's
/// `.bundle/config`, written as `bundle config set --local path` writes it,
/// keeping every other setting there. Nothing is written when it is
/// already so.
fn write_bundle_config(root: &Path) -> std::io::Result<()> {
    let path = root.join(".bundle/config");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let setting = format!("BUNDLE_PATH: \"{GEM_HOME}\"");
    let mut lines: Vec<&str> = existing.lines().collect();
    if lines.is_empty() {
        lines.push("---");
    }
    match lines.iter().position(|l| l.starts_with("BUNDLE_PATH:")) {
        Some(at) => lines[at] = &setting,
        None => lines.push(&setting),
    }
    let text = format!("{}\n", lines.join("\n"));
    if text == existing {
        return Ok(());
    }
    std::fs::create_dir_all(root.join(".bundle"))?;
    write_atomically(&path, text.as_bytes())
}

/// Write `bytes` beside `path` and rename them into place, so a reader
/// never sees half a file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staging = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&staging, bytes)?;
    std::fs::rename(&staging, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&staging);
    })
}

/// Write a file the user owns, as [`write_atomically`] does, but to the file
/// a symlink names and with the permissions it had, so only its bytes
/// change.
fn write_in_place(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path)?;
    let permissions = std::fs::metadata(&target)?.permissions();
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staging = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&staging, bytes)?;
    std::fs::set_permissions(&staging, permissions)
        .and_then(|()| std::fs::rename(&staging, &target))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&staging);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lockfile_sits_beside_its_gemfile() {
        assert_eq!(
            lockfile_path(Path::new("/app/Gemfile")),
            PathBuf::from("/app/Gemfile.lock")
        );
        assert_eq!(
            lockfile_path(Path::new("/app/gems.rb")),
            PathBuf::from("/app/gems.locked")
        );
        assert_eq!(
            lockfile_path(Path::new("/app/Gemfile.next")),
            PathBuf::from("/app/Gemfile.next.lock")
        );
    }

    #[test]
    fn a_full_name_splits_at_its_version() {
        let split = |s: &str| split_full_name(s);
        assert_eq!(split("rack-3.2.8"), ("rack".into(), "3.2.8".into()));
        assert_eq!(
            split("rack-session-2.1.2"),
            ("rack-session".into(), "2.1.2".into())
        );
        assert_eq!(
            split("nokogiri-1.16.0-x86_64-linux"),
            ("nokogiri".into(), "1.16.0-x86_64-linux".into())
        );
    }

    #[test]
    fn bundle_config_keeps_other_settings() {
        let dir = std::env::temp_dir().join(format!("spinel-install-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".bundle")).unwrap();
        write_bundle_config(&dir).unwrap();
        let config = dir.join(".bundle/config");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "---\nBUNDLE_PATH: \".spinel/gem_home\"\n"
        );
        std::fs::write(
            &config,
            "---\nBUNDLE_JOBS: \"4\"\nBUNDLE_PATH: \"vendor/bundle\"\n",
        )
        .unwrap();
        write_bundle_config(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "---\nBUNDLE_JOBS: \"4\"\nBUNDLE_PATH: \".spinel/gem_home\"\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_blocks_hold_as_bundler_checks_them() {
        assert!(env_holds(None));
        assert!(env_holds(Some(&serde_json::Value::Null)));
        assert!(env_holds(Some(&serde_json::json!("PATH"))));
        assert!(!env_holds(Some(&serde_json::json!(
            "SPINEL_INSTALL_TEST_SURELY_UNSET"
        ))));
        assert!(!env_holds(Some(&serde_json::json!({
            "SPINEL_INSTALL_TEST_SURELY_UNSET": "1"
        }))));
    }
}
