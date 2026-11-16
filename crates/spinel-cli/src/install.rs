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
use crate::lockfile::{Lockfile, SourceKind};
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
    /// The global rubygems source, as written.
    pub remote: String,
    pub request: resolver::Request,
    /// `Platform.local`.
    pub local: gem_platform::Platform,
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
        }) if kind == "rubygems" => remote.clone(),
        _ => return Err("the Gemfile names no rubygems source".to_owned()),
    };
    // ponytail: one global rubygems source. Upgrade: git and path sources
    // (#88), and a source scoped to some gems, each resolved with its own
    // index.
    if !gemfile.gemspecs.is_empty() {
        return Err("`gemspec` is not supported yet (#88)".to_owned());
    }
    let local = gem_platform::Platform::local(spinel_vm::PLATFORM);
    let mut dependencies = Vec::new();
    for dep in &gemfile.dependencies {
        if dep.source.is_some() {
            return Err(format!(
                "{}: a source of its own is not supported yet (#88)",
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
    let shown_remote = if remote.ends_with('/') {
        remote.clone()
    } else {
        format!("{remote}/")
    };
    Ok(Prepared {
        request: resolver::Request {
            gemfile: gemfile_name,
            source: format!("rubygems repository {shown_remote}"),
            dependencies,
            platforms,
            ruby_version,
            rubygems_version,
        },
        remote,
        local,
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
    client.versions().await?;
    let mut index = resolver::Index::new();
    let mut wanted: Vec<String> = roots.into_iter().filter(|r| r != "bundler").collect();
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

/// Why `spinel install` (or `spinel add`) stopped: the exit status, after
/// what to say was printed.
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

/// A new Gemfile `spinel add` writes once everything it installs is in
/// place, just before the lockfile, and puts back if the lockfile cannot be
/// written.
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

/// One evaluated Gemfile on its way to the GEM_HOME: where its project and
/// lockfile are, the resolver's request, and the index client.
pub(crate) struct Session {
    /// The command whose messages these are: `install` or `add`.
    command: &'static str,
    frozen: bool,
    gemfile_path: PathBuf,
    root: PathBuf,
    lock_path: PathBuf,
    lock_name: String,
    prepared: Prepared,
    ruby: Option<Vec<String>>,
    /// The lockfile as it is on disk, if there is one.
    current: Option<String>,
    home: PathBuf,
    client: compact_index::Client,
    runtime: tokio::runtime::Runtime,
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
        let client = compact_index::Client::new(&prepared.remote, &home)
            .map_err(|err| fail(err.to_string()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| fail(format!("cannot start the async runtime: {err}")))?;
        Ok(Session {
            command,
            frozen,
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
        })
    }

    fn fail(&self, message: impl std::fmt::Display) -> Stop {
        Stop::failure(self.command, message)
    }

    /// The Gemfile this session read, as the evaluator found it.
    pub(crate) fn gemfile_path(&self) -> &Path {
        &self.gemfile_path
    }

    fn read_index(&self) -> Result<resolver::Index, Stop> {
        let roots: Vec<String> = self
            .prepared
            .request
            .dependencies
            .iter()
            .map(|d| d.name.clone())
            .collect();
        self.runtime
            .block_on(read_index(&self.client, roots, &self.prepared.local))
            .map_err(|err| self.fail(err))
    }

    /// Resolve, keeping to the lockfile when there is one (#79), into the
    /// lockfile Bundler would write. Nothing is written.
    pub(crate) fn resolve(&mut self) -> Result<Resolved, Stop> {
        let local = GemPlatform::Platform(self.prepared.local.clone());
        let Some(text) = self.current.clone() else {
            let index = self.read_index()?;
            let resolved =
                resolver::resolve(&self.prepared.request, &index).map_err(|failure| {
                    eprintln!("{}", failure.to_string().trim_end());
                    Stop(ExitCode::FAILURE)
                })?;
            let lock = Lockfile::from_resolution(
                &self.prepared.remote,
                &self.prepared.request,
                &index,
                &resolved,
                self.ruby.is_some(),
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
        let mut definition = locked::Definition::new(
            &lock,
            &self.prepared.remote,
            &self.prepared.request.dependencies,
            self.ruby.as_deref(),
            &local,
            &[],
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
                &self.prepared.remote,
                &self.prepared.request,
                Some(&index),
                &specs,
            );
            Ok(Resolved {
                written,
                index: Some(index),
            })
        } else {
            let specs = definition.unchanged(&self.prepared.request.dependencies);
            let written = definition.write_lock(
                &text,
                &self.prepared.remote,
                &self.prepared.request,
                None,
                &specs,
            );
            Ok(Resolved {
                written,
                index: None,
            })
        }
    }

    /// The specs a resolution locks from the rubygems source, every
    /// platform's build.
    pub(crate) fn locked_specs(&self, resolved: &Resolved) -> Result<Vec<Spec>, Stop> {
        self.read_written(&resolved.written).map(|(_, specs)| specs)
    }

    /// The lockfile a resolution writes, read back, and its rubygems
    /// source's specs.
    fn read_written(&self, written: &str) -> Result<(Lockfile, Vec<Spec>), Stop> {
        let lock = Lockfile::parse(written, &self.lock_name, &self.prepared.local)
            .map_err(|err| self.fail(format!("the lockfile written does not read back: {err}")))?;
        let specs = lock
            .sources
            .iter()
            .filter(|s| matches!(s.kind, SourceKind::Gem { .. }))
            .flat_map(|s| s.specs.iter().map(locked::locked_spec))
            .collect();
        Ok((lock, specs))
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
        let (lock, specs) = self.read_written(&written)?;
        let wanted = installed_dependencies(gemfile, &prepared.request.dependencies, &local);
        let mut to_install = lockfile::spec_set_for(&specs, &wanted, std::slice::from_ref(&local));
        to_install.sort_by_cached_key(Spec::full_name);

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

        let client = &self.client;
        let store = gem_store::Store::new(&self.home);
        let mut wants = Vec::new();
        let mut ok = true;
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
                            prepared.remote
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
        let mut stored = Vec::new();
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
        let report = gem_home::link(
            &gem_home_dir,
            &absolute(&store.root().join("gems")),
            &stored,
        )
        .map_err(|err| self.fail(err))?;
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
        for spec in &to_install {
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
    serde_json::from_slice(&out.stdout).map_err(|err| {
        Stop::failure(
            command,
            format!("the Gemfile evaluator printed no JSON: {err}"),
        )
    })
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
