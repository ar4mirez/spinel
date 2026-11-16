//! Git and path sources (#88, docs/package-manager.md, "Git and path
//! sources"): `gem "x", git: ...` and `gem "x", path: ...`, read as Bundler
//! 4's `Source::Git` and `Source::Path` read them.
//!
//! A source the Gemfile names is a [`Declared`]. Its gems come from its
//! gemspecs, evaluated on the VM (`shims/gemspec_eval.rb`): a path source's
//! where they are, every time, as nothing of it is copied; a git source's in
//! its checkout of one revision (`crate::git_source`), once, the evaluation
//! kept beside the checkout. Each gem it has is a [`LocalGem`], whose
//! [`Spec`] the resolver takes in place of the index's builds of that name
//! (`SourceMap#all_requirements`).

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest as _, Sha256};

use crate::gem_platform::{GemPlatform, Platform};
use crate::gem_requirement::Requirement;
use crate::gem_version::Version;
use crate::git_source;
use crate::lockfile::SourceKind;
use crate::resolver::Spec;

/// `Source::Path::DEFAULT_GLOB`, `Source::Git`'s too.
pub const DEFAULT_GLOB: &str = "{,*,*/*}.gemspec";

/// Spinel's gemspec evaluator, run on the VM by a child `spinel`.
const GEMSPEC_EVAL: &str = include_str!("../../../shims/gemspec_eval.rb");

/// A git or path source as the Gemfile declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declared {
    Git {
        /// As written, less a trailing `/` (`normalize_suffix`).
        uri: String,
        branch: Option<String>,
        git_ref: Option<String>,
        tag: Option<String>,
        /// `true` when given and not false, as the lockfile writes it.
        submodules: Option<String>,
        glob: String,
    },
    Path {
        /// The directory, absolute and cleaned (`expand_path`).
        expanded: PathBuf,
        /// `@path`: relative to the directory of the Gemfile that names it,
        /// or absolute, as `to_s` shows it.
        shown: String,
        /// `lockfile_path`: as the lockfile's `remote:` has it.
        remote: String,
        glob: String,
    },
}

/// A source and the Gemfile gems pinned to it.
#[derive(Debug, Clone)]
pub struct DeclaredSource {
    pub kind: Declared,
    pub pinned: Vec<String>,
}

/// `Pathname#cleanpath` of an absolute path: `.` and `..` taken out by
/// their spelling, as `File.expand_path` does, no symlink followed.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("/");
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// `File.expand_path(path, base)`.
pub fn expand(path: &str, base: &Path) -> PathBuf {
    clean(&base.join(path))
}

/// `Pathname#relative_path_from` of two absolute, cleaned paths.
pub fn relative_path_from(path: &Path, base: &Path) -> String {
    let p: Vec<_> = path.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = p.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); b.len() - common];
    parts.extend(
        p[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

fn option_string(source: &serde_json::Value, key: &str) -> Option<String> {
    match source.get(key)? {
        serde_json::Value::Null | serde_json::Value::Bool(false) => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

impl Declared {
    /// A dependency's `source` from the evaluator's JSON (docs/package-
    /// manager.md, "1. Gemfile evaluation"); `project` is `Bundler.root`,
    /// the Gemfile's directory. `None` for a rubygems or plugin source.
    pub fn from_json(
        source: &serde_json::Value,
        project: &Path,
    ) -> Result<Option<Declared>, String> {
        let glob = option_string(source, "glob").unwrap_or_else(|| DEFAULT_GLOB.to_owned());
        match source.get("type").and_then(serde_json::Value::as_str) {
            Some("git") => {
                let uri = option_string(source, "uri").unwrap_or_default();
                let uri = uri.strip_suffix('/').map_or(uri.clone(), str::to_owned);
                let branch = option_string(source, "branch");
                let git_ref = option_string(source, "ref");
                let tag = option_string(source, "tag");
                if tag.is_some() && (branch.is_some() || git_ref.is_some()) {
                    return Err(format!(
                        "Specification of branch or ref with tag is ambiguous. You specified {}",
                        source
                    ));
                }
                git_source::check_argument("source", &uri).map_err(|e| e.0)?;
                for (what, value) in [("branch", &branch), ("ref", &git_ref), ("tag", &tag)] {
                    if let Some(value) = value {
                        git_source::check_argument(what, value).map_err(|e| e.0)?;
                    }
                }
                Ok(Some(Declared::Git {
                    uri,
                    branch,
                    git_ref,
                    tag,
                    submodules: option_string(source, "submodules"),
                    glob,
                }))
            }
            Some("path") => {
                let path = option_string(source, "path").unwrap_or_default();
                let root =
                    option_string(source, "root").map_or_else(|| project.to_owned(), PathBuf::from);
                let expanded = expand(&path, &root);
                let shown = if Path::new(&path).is_absolute() {
                    expanded.display().to_string()
                } else {
                    relative_path_from(&expanded, &clean(&root))
                };
                // `lockfile_path`: an absolute path as `relative_path` has
                // it (relative to the root when under it, by its spelling),
                // a relative one relative to the project.
                let remote = if Path::new(&path).is_absolute() {
                    let root_text = clean(&root).display().to_string();
                    if expanded.display().to_string().starts_with(&root_text) {
                        relative_path_from(&expanded, &clean(&root))
                    } else {
                        expanded.display().to_string()
                    }
                } else {
                    relative_path_from(&expanded, &clean(project))
                };
                Ok(Some(Declared::Path {
                    expanded,
                    shown,
                    remote,
                    glob,
                }))
            }
            _ => Ok(None),
        }
    }

    /// `ref || branch || tag`: what the revision is found from.
    pub fn reference(&self) -> Option<&str> {
        match self {
            Declared::Git {
                git_ref,
                branch,
                tag,
                ..
            } => git_ref.as_deref().or(branch.as_deref()).or(tag.as_deref()),
            Declared::Path { .. } => None,
        }
    }

    /// `Source::Git#==` and `Source::Path#==` against a source the lockfile
    /// has: a git source by its URI, ref, branch, glob and submodules, a
    /// path source by where it leads from `project`.
    pub fn same_as_locked(&self, locked: &SourceKind, project: &Path) -> bool {
        match (self, locked) {
            (
                Declared::Git {
                    uri,
                    branch,
                    submodules,
                    glob,
                    ..
                },
                SourceKind::Git {
                    remote,
                    git_ref: l_ref,
                    branch: l_branch,
                    tag: l_tag,
                    submodules: l_submodules,
                    glob: l_glob,
                    ..
                },
            ) => {
                let locked_reference = l_ref
                    .as_deref()
                    .or(l_branch.as_deref())
                    .or(l_tag.as_deref());
                let l_glob = l_glob.as_deref().unwrap_or(DEFAULT_GLOB);
                remote.strip_suffix('/').unwrap_or(remote) == uri
                    && locked_reference == self.reference()
                    && l_branch == branch
                    && l_glob == glob
                    && l_submodules == submodules
            }
            (Declared::Path { expanded, .. }, SourceKind::Path { remote, .. }) => {
                expand(remote, project) == *expanded
            }
            _ => false,
        }
    }

    /// `Source::Git#include?`: the same repository, glob and submodules,
    /// whatever the ref; a path source is `==`.
    pub fn includes_locked(&self, locked: &SourceKind, project: &Path) -> bool {
        match (self, locked) {
            (
                Declared::Git {
                    uri,
                    submodules,
                    glob,
                    ..
                },
                SourceKind::Git {
                    remote,
                    submodules: l_submodules,
                    glob: l_glob,
                    ..
                },
            ) => {
                remote.strip_suffix('/').unwrap_or(remote) == uri
                    && l_glob.as_deref().unwrap_or(DEFAULT_GLOB) == glob
                    && l_submodules == submodules
            }
            _ => self.same_as_locked(locked, project),
        }
    }

    /// `to_gemfile`: a git source's URI and the options given, a path
    /// source's path.
    pub fn to_gemfile(&self) -> String {
        match self {
            Declared::Path { shown, .. } => shown.clone(),
            Declared::Git { .. } => self.to_lock(None).to_gemfile(),
        }
    }

    /// `to_lock`'s options, at `revision` for a git source.
    pub fn to_lock(&self, revision: Option<&str>) -> SourceKind {
        let glob = |g: &str| (g != DEFAULT_GLOB).then(|| g.to_owned());
        match self {
            Declared::Git {
                uri,
                branch,
                git_ref,
                tag,
                submodules,
                glob: g,
            } => SourceKind::Git {
                remote: uri.clone(),
                revision: revision.unwrap_or_default().to_owned(),
                git_ref: git_ref.clone(),
                branch: branch.clone(),
                tag: tag.clone(),
                submodules: submodules.clone(),
                glob: glob(g),
            },
            Declared::Path {
                remote, glob: g, ..
            } => SourceKind::Path {
                remote: remote.clone(),
                glob: glob(g),
            },
        }
    }

    /// `identifier`, which `SourceList#lock_other_sources` sorts by: a git
    /// source's URI and, in parentheses, its ref as `humanized_ref` shows
    /// it, the locked revision when it came from the lockfile and is not
    /// unlocked, and a glob that is not the default; a path source's
    /// `source at \`path\``.
    pub fn identifier(&self, locked_revision: Option<&str>) -> String {
        match self {
            Declared::Git { uri, glob, .. } => {
                let mut specifiers: Vec<String> = Vec::new();
                if let Some(at) = self.humanized_ref() {
                    specifiers.push(at);
                }
                if let Some(revision) = locked_revision {
                    specifiers.push(revision.to_owned());
                }
                if glob != DEFAULT_GLOB {
                    specifiers.push(format!("glob: {glob}"));
                }
                with_specifiers(uri, &specifiers)
            }
            Declared::Path { shown, .. } => format!("source at `{shown}`"),
        }
    }

    /// `to_s`: the source as messages name it.
    pub fn describe(&self, revision: Option<&str>) -> String {
        match self {
            Declared::Git { uri, glob, .. } => {
                let mut specifiers: Vec<String> = Vec::new();
                if let Some(revision) = revision {
                    let short = &revision[..revision.len().min(7)];
                    specifiers.push(match self.humanized_ref() {
                        Some(at) => format!("at {at}@{short}"),
                        None => format!("at {short}"),
                    });
                }
                if glob != DEFAULT_GLOB {
                    specifiers.push(format!("glob: {glob}"));
                }
                with_specifiers(uri, &specifiers)
            }
            Declared::Path { .. } => self.identifier(None),
        }
    }

    /// `humanized_ref`.
    fn humanized_ref(&self) -> Option<String> {
        let Declared::Git { git_ref, .. } = self else {
            return None;
        };
        let reference = self.reference()?;
        match git_ref {
            Some(user_ref) => {
                let short =
                    reference.len() >= 4 && reference.bytes().all(|b| b.is_ascii_alphanumeric());
                Some(if short {
                    user_ref.chars().take(7).collect()
                } else {
                    user_ref.clone()
                })
            }
            None => Some(reference.to_owned()),
        }
    }
}

/// `uri_with_specifiers`.
fn with_specifiers(uri: &str, specifiers: &[String]) -> String {
    if specifiers.is_empty() {
        uri.to_owned()
    } else {
        format!("{uri} ({})", specifiers.join(", "))
    }
}

/// The git and path sources of a Gemfile, in `SourceList#all_sources`'
/// order (path sources, then git, each as first declared; equal ones are
/// one), and for each dependency the one it is pinned to, if any.
pub fn declared(
    sources: &[Option<&serde_json::Value>],
    names: &[&str],
    project: &Path,
) -> Result<(Vec<DeclaredSource>, Vec<Option<usize>>), String> {
    let mut paths: Vec<DeclaredSource> = Vec::new();
    let mut gits: Vec<DeclaredSource> = Vec::new();
    let mut pins: Vec<Option<(bool, usize)>> = Vec::new();
    for (source, name) in sources.iter().zip(names) {
        let Some(kind) = source
            .map(|s| Declared::from_json(s, project))
            .transpose()?
            .flatten()
        else {
            pins.push(None);
            continue;
        };
        let is_path = matches!(kind, Declared::Path { .. });
        let list = if is_path { &mut paths } else { &mut gits };
        let at = match list.iter().position(|s| s.kind == kind) {
            Some(at) => at,
            None => {
                list.push(DeclaredSource {
                    kind,
                    pinned: Vec::new(),
                });
                list.len() - 1
            }
        };
        if !list[at].pinned.iter().any(|n| n == name) {
            list[at].pinned.push((*name).to_owned());
        }
        pins.push(Some((is_path, at)));
    }
    let offset = paths.len();
    let pins = pins
        .into_iter()
        .map(|p| p.map(|(is_path, at)| if is_path { at } else { offset + at }))
        .collect();
    paths.extend(gits);
    Ok((paths, pins))
}

/// One gem a git or path source has: what its evaluated gemspec says.
#[derive(Debug, Clone)]
pub struct LocalGem {
    /// As the resolver reads it.
    pub spec: Spec,
    /// `full_gem_path`: the directory of its gemspec.
    pub gem_dir: PathBuf,
    /// `to_ruby_for_cache`, as `Gem::Installer#write_spec` writes it.
    pub gemspec: String,
    pub require_paths: Vec<String>,
    pub bindir: Option<String>,
    pub executables: Vec<String>,
    pub extensions: Vec<String>,
    pub development: Vec<(String, Requirement)>,
}

impl LocalGem {
    pub fn full_name(&self) -> String {
        self.spec.full_name()
    }
}

/// A gemspec as `shims/gemspec_eval.rb` prints it; `file` and `dir` are
/// relative to the checkout in a git source's kept evaluation.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct Evaluated {
    file: String,
    dir: String,
    name: String,
    version: String,
    platform: String,
    dependencies: Vec<(String, Vec<String>)>,
    development_dependencies: Vec<(String, Vec<String>)>,
    required_ruby_version: Vec<String>,
    required_rubygems_version: Vec<String>,
    require_paths: Vec<String>,
    bindir: Option<String>,
    executables: Vec<String>,
    extensions: Vec<String>,
    gemspec: String,
}

/// What a gemspec may not hold for its name, files and stubs to be put in
/// a path and a gemspec safely, as `Gem::Installer#verify_spec` refuses.
fn verify(e: &Evaluated) -> Result<(), String> {
    let refuse = |what: &str| Err(format!("{}: its gemspec has {what}", e.file));
    if !crate::compact_index::valid_name(&e.name) {
        return refuse(&format!("an invalid name {:?}", e.name));
    }
    let breaks = |s: &str| s.contains('\n') || s.contains('\r') || s.contains('\0');
    if e.require_paths.iter().any(|p| breaks(p)) || e.extensions.iter().any(|p| breaks(p)) {
        return refuse("a line break in a require path or extension");
    }
    if breaks(&e.platform) || breaks(&e.version) {
        return refuse("a line break in its version or platform");
    }
    if let Some(bindir) = &e.bindir {
        let p = Path::new(bindir);
        if breaks(bindir)
            || p.is_absolute()
            || p.components().any(|c| matches!(c, Component::ParentDir))
        {
            return refuse(&format!("a bindir outside the gem: {bindir:?}"));
        }
    }
    for exe in &e.executables {
        if exe.is_empty() || exe == "." || exe == ".." || exe.contains('/') || breaks(exe) {
            return refuse(&format!("an executable that is not a file name: {exe:?}"));
        }
    }
    for (name, _) in e.dependencies.iter().chain(&e.development_dependencies) {
        if name.contains(['<', '>']) || breaks(name) {
            return refuse(&format!("a dependency named {name:?}"));
        }
    }
    Ok(())
}

fn requirement(list: &[String]) -> Result<Requirement, String> {
    Requirement::new(list.iter().map(String::as_str)).map_err(|e| e.to_string())
}

impl Evaluated {
    fn local_gem(&self, base: &Path, local: &Platform) -> Result<LocalGem, String> {
        verify(self)?;
        let version = Version::new(&self.version).map_err(|e| format!("{}: {e}", self.file))?;
        let dependencies = self
            .dependencies
            .iter()
            .map(|(name, reqs)| Ok((name.clone(), requirement(reqs)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let development = self
            .development_dependencies
            .iter()
            .map(|(name, reqs)| Ok((name.clone(), requirement(reqs)?)))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(LocalGem {
            spec: Spec {
                name: self.name.clone(),
                version,
                platform: GemPlatform::new(Some(&self.platform), local),
                dependencies,
                required_ruby_version: requirement(&self.required_ruby_version)?,
                required_rubygems_version: requirement(&self.required_rubygems_version)?,
                checksum: None,
            },
            gem_dir: if self.dir.is_empty() {
                base.to_owned()
            } else {
                base.join(&self.dir)
            },
            gemspec: self.gemspec.clone(),
            require_paths: self.require_paths.clone(),
            bindir: self.bindir.clone(),
            executables: self.executables.clone(),
            extensions: self.extensions.clone(),
            development,
        })
    }
}

/// Evaluate the gemspecs each `(dir, glob, serialize)` finds, in a child
/// `spinel` running `shims/gemspec_eval.rb`. An error is the evaluator's
/// message, as Bundler words it.
fn evaluate(sources: &[(&Path, &str, bool)]) -> Result<Vec<Vec<Evaluated>>, String> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let exe = std::env::current_exe()
        .map_err(|err| format!("cannot find the spinel executable: {err}"))?;
    let input: Vec<serde_json::Value> = sources
        .iter()
        .map(|(dir, glob, serialize)| {
            serde_json::json!({
                "dir": dir.display().to_string(),
                "glob": glob,
                "serialize": serialize,
            })
        })
        .collect();
    let input = serde_json::to_string(&input).expect("JSON of strings");
    let script = format!(
        "require \"json\"\n$spinel_gemspec_sources = JSON.parse(String.new({}, encoding: \"UTF-8\"))\n{GEMSPEC_EVAL}",
        crate::gemfile_edit::dump_bytes(input.as_bytes())
    );
    let mut child = Command::new(exe)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot evaluate the gemspecs: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A child that exits early closes its end; its status says why.
        let _ = stdin.write_all(script.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|err| format!("cannot evaluate the gemspecs: {err}"))?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(err.trim_end().to_owned());
    }
    if !err.is_empty() {
        eprint!("{err}");
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|err| format!("the gemspec evaluator printed no JSON: {err}"))
}

/// What a git or path source has, read.
#[derive(Debug, Clone, Default)]
pub struct Materialized {
    /// The revision a git source is at.
    pub revision: Option<String>,
    /// A git source's checkout in the store.
    pub checkout: Option<PathBuf>,
    pub gems: Vec<LocalGem>,
}

/// Read each path source's gemspecs where they are, in one child `spinel`.
// ponytail: a path with no gemspec has no gems, where Bundler makes a
// stand-in gemspec from the source's `name` and `version`. Upgrade by passing
// those to the evaluator.
pub fn read_paths(sources: &[&Declared], local: &Platform) -> Result<Vec<Materialized>, String> {
    let mut wanted: Vec<(&Path, &str, bool)> = Vec::new();
    for source in sources {
        let Declared::Path { expanded, glob, .. } = source else {
            continue;
        };
        if !expanded.is_dir() {
            let what = if expanded.exists() {
                "is not a directory."
            } else {
                "does not exist."
            };
            return Err(format!("The path `{}` {what}", expanded.display()));
        }
        wanted.push((expanded, glob, false));
    }
    evaluate(&wanted)?
        .into_iter()
        .map(|gems| {
            Ok(Materialized {
                revision: None,
                checkout: None,
                gems: gems
                    .iter()
                    .map(|e| e.local_gem(Path::new("/"), local))
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect()
}

/// `Dsl#gemspec`: the gem of the gemspec in `dir` (`{,*}.gemspec`), of the
/// one named `name` when given, its build for the local platform when it
/// has several; Bundler's error when there is none, or more than one gem.
pub fn read_gemspec(dir: &Path, name: Option<&str>, local: &Platform) -> Result<LocalGem, String> {
    let evaluated = evaluate(&[(dir, "{,*}.gemspec", false)])?
        .pop()
        .unwrap_or_default();
    let gems: Vec<LocalGem> = evaluated
        .iter()
        .filter(|e| name.is_none_or(|n| e.name == n))
        .map(|e| e.local_gem(Path::new("/"), local))
        .collect::<Result<_, _>>()?;
    let mut versions: Vec<(String, String)> = gems
        .iter()
        .map(|g| (g.spec.name.clone(), g.spec.version.to_string()))
        .collect();
    versions.sort();
    versions.dedup();
    match versions.len() {
        0 => Err(format!("There are no gemspecs at {}", dir.display())),
        1 => {
            let here = GemPlatform::Platform(local.clone());
            Ok(gems
                .iter()
                .find(|g| g.spec.platform == here)
                .unwrap_or(&gems[0])
                .clone())
        }
        _ => Err(format!(
            "There are multiple gemspecs at {}. Please use the :name option to specify which one should be used",
            dir.display()
        )),
    }
}

/// The file a git checkout's evaluation for `glob` is kept in.
fn kept_evaluation(entry: &Path, glob: &str) -> PathBuf {
    let digest = Sha256::digest(glob.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    entry.join(format!("gemspecs-{hex}.json"))
}

/// Evaluate a checkout's gemspecs for `glob`, and keep the result beside it
/// with its paths relative to the checkout.
fn evaluate_checkout(work: &Path, glob: &str, serialize: bool) -> Result<Vec<Evaluated>, String> {
    let mut evaluated = evaluate(&[(work, glob, serialize)])?
        .pop()
        .unwrap_or_default();
    let relative = |p: &str| {
        Path::new(p)
            .strip_prefix(work)
            .map_or_else(|_| p.to_owned(), |r| r.display().to_string())
    };
    for e in &mut evaluated {
        e.file = relative(&e.file);
        e.dir = relative(&e.dir);
    }
    let parent = work.parent().expect("a checkout is in its entry");
    let text = serde_json::to_vec(&evaluated).expect("JSON of strings");
    crate::install::write_atomically(&kept_evaluation(parent, glob), &text)
        .map_err(|err| format!("{}: {err}", parent.display()))?;
    Ok(evaluated)
}

/// Read a git source at `locked` (its locked revision, still held), else at
/// the revision its ref, branch or tag (or the remote's `HEAD`) is at now:
/// fetched into the mirror as needed, checked out into the store, and its
/// gemspecs evaluated there once.
pub fn read_git(
    home: &Path,
    source: &Declared,
    locked: Option<&str>,
    local: &Platform,
) -> Result<Materialized, String> {
    let Declared::Git {
        uri,
        submodules,
        glob,
        ..
    } = source
    else {
        return Err("not a git source".to_owned());
    };
    let (mirror, fresh) = git_source::mirror(home, uri).map_err(|e| e.0)?;
    let reference = source.reference();
    let extra = reference.filter(|r| r.starts_with("refs/"));
    let full_sha = |r: &str| git_source::check_revision(r).is_ok();
    let revision = match locked {
        Some(revision) => {
            git_source::check_revision(revision).map_err(|e| e.0)?;
            if !git_source::has_revision(&mirror, revision) {
                git_source::fetch(&mirror, uri, extra).map_err(|e| e.0)?;
            }
            if !git_source::has_revision(&mirror, revision) {
                let _ = git_source::fetch_revision(&mirror, uri, revision);
            }
            if !git_source::has_revision(&mirror, revision) {
                return Err(git_source::missing_revision(revision, uri).0);
            }
            revision.to_owned()
        }
        None => match reference {
            Some(sha) if full_sha(sha) => {
                if !git_source::has_revision(&mirror, sha) {
                    let _ = git_source::fetch_revision(&mirror, uri, sha);
                }
                if !git_source::has_revision(&mirror, sha) {
                    return Err(git_source::missing_revision(sha, uri).0);
                }
                sha.to_owned()
            }
            _ => {
                if !fresh || extra.is_some() {
                    git_source::fetch(&mirror, uri, extra).map_err(|e| e.0)?;
                }
                git_source::rev_parse(&mirror, uri, reference).map_err(|e| e.0)?
            }
        },
    };
    let submodules = submodules.is_some();
    let entry = git_source::checkout(home, &mirror, uri, &revision, submodules, |work| {
        evaluate_checkout(work, glob, true).map(|_| ())
    })?;
    let work = entry.join("checkout");
    let kept = std::fs::read(kept_evaluation(&entry, glob))
        .ok()
        .and_then(|text| serde_json::from_slice::<Vec<Evaluated>>(&text).ok());
    let evaluated = match kept {
        Some(evaluated) => evaluated,
        None => evaluate_checkout(&work, glob, false)?,
    };
    Ok(Materialized {
        revision: Some(revision),
        gems: evaluated
            .iter()
            .map(|e| e.local_gem(&work, local))
            .collect::<Result<_, _>>()?,
        checkout: Some(work),
    })
}

/// `Source::Git#install_path`'s last part: the base name and the first 12
/// digits of the revision, where Bundler checks a git source out
/// (`<bundle path>/bundler/gems/`).
pub fn bundler_checkout_name(uri: &str, revision: &str) -> String {
    format!(
        "{}-{}",
        git_source::base_name(uri),
        &revision[..revision.len().min(12)]
    )
}

/// The names `sources` provide, each the first source to have it
/// (`SourceMap#all_requirements`): the gems pinned to a source come from it,
/// and every other gem a source has from that source too, unless two
/// sources have it, which Bundler refuses.
pub fn providers(
    sources: &[DeclaredSource],
    read: &[Materialized],
    revisions: &[Option<String>],
) -> Result<Vec<(String, usize)>, String> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        for name in &source.pinned {
            if !out.iter().any(|(n, _)| n == name) {
                out.push((name.clone(), i));
            }
        }
    }
    let pinned: HashSet<String> = out.iter().map(|(n, _)| n.clone()).collect();
    for (i, materialized) in read.iter().enumerate() {
        let mut names: Vec<&str> = materialized
            .gems
            .iter()
            .map(|g| g.spec.name.as_str())
            .filter(|n| !pinned.contains(*n))
            .collect();
        names.dedup();
        for name in names {
            match out.iter().find(|(n, _)| n == name) {
                None => out.push((name.to_owned(), i)),
                Some((_, j)) if *j == i => {}
                Some((_, j)) => {
                    let mut shown = [
                        format!(
                            "  * {}",
                            sources[*j].kind.describe(revisions[*j].as_deref())
                        ),
                        format!("  * {}", sources[i].kind.describe(revisions[i].as_deref())),
                    ];
                    shown.sort();
                    return Err(format!(
                        "The gem '{name}' was found in multiple relevant sources.\n{}\n\
                         You must add this gem to the source block for the source you wish it to be installed from.",
                        shown.join("\n")
                    ));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_as_pathname_makes_them() {
        let rel = |p: &str, b: &str| relative_path_from(Path::new(p), Path::new(b));
        assert_eq!(rel("/a/b/c", "/a/b"), "c");
        assert_eq!(rel("/a/x", "/a/b"), "../x");
        assert_eq!(rel("/a/b", "/a/b"), ".");
        assert_eq!(rel("/", "/a/b"), "../..");
        assert_eq!(
            expand("../foo/./bar", Path::new("/app/sub")),
            PathBuf::from("/app/foo/bar")
        );
    }

    #[test]
    fn path_sources_lock_relative_to_the_project() {
        let json = serde_json::json!({"type": "path", "path": "../localgem", "root": "/w/app", "glob": null});
        let declared = Declared::from_json(&json, Path::new("/w/app"))
            .unwrap()
            .unwrap();
        let Declared::Path {
            remote,
            shown,
            expanded,
            ..
        } = &declared
        else {
            panic!("a path source");
        };
        assert_eq!(remote, "../localgem");
        assert_eq!(shown, "../localgem");
        assert_eq!(expanded, Path::new("/w/localgem"));
        assert_eq!(declared.identifier(None), "source at `../localgem`");
        // From a Gemfile `eval_gemfile` read in a subdirectory: relative to
        // that file as written, to the project in the lockfile.
        let json = serde_json::json!({"type": "path", "path": "gems/x", "root": "/w/app/sub", "glob": null});
        let declared = Declared::from_json(&json, Path::new("/w/app"))
            .unwrap()
            .unwrap();
        assert_eq!(
            declared.to_lock(None),
            SourceKind::Path {
                remote: "sub/gems/x".into(),
                glob: None
            }
        );
    }

    #[test]
    fn git_sources_compare_as_bundler_compares_them() {
        let json = serde_json::json!({"type": "git", "uri": "https://example.com/foo.git/", "branch": "main",
                                      "ref": null, "tag": null, "submodules": null, "glob": null});
        let declared = Declared::from_json(&json, Path::new("/app"))
            .unwrap()
            .unwrap();
        let locked = declared.to_lock(Some("d7ea43fc3548bd4c1d198faff07de727daa1ac40"));
        assert!(declared.same_as_locked(&locked, Path::new("/app")));
        let SourceKind::Git { remote, branch, .. } = &locked else {
            panic!()
        };
        assert_eq!(remote, "https://example.com/foo.git");
        assert_eq!(branch.as_deref(), Some("main"));
        let other = serde_json::json!({"type": "git", "uri": "https://example.com/foo.git", "branch": "dev",
                                       "ref": null, "tag": null, "submodules": null, "glob": null});
        let other = Declared::from_json(&other, Path::new("/app"))
            .unwrap()
            .unwrap();
        assert!(!other.same_as_locked(&locked, Path::new("/app")));
        assert_eq!(
            declared.identifier(None),
            "https://example.com/foo.git (main)"
        );
        assert_eq!(
            declared.describe(Some("d7ea43fc3548bd4c1d198faff07de727daa1ac40")),
            "https://example.com/foo.git (at main@d7ea43f)"
        );
    }

    #[test]
    fn a_tag_with_a_branch_is_ambiguous() {
        let json = serde_json::json!({"type": "git", "uri": "u", "branch": "main", "ref": null, "tag": "v1",
                                      "submodules": null, "glob": null});
        let err = Declared::from_json(&json, Path::new("/app")).unwrap_err();
        assert!(
            err.starts_with("Specification of branch or ref with tag is ambiguous."),
            "{err}"
        );
        let json = serde_json::json!({"type": "git", "uri": "u", "branch": "--upload-pack=touch x", "ref": null,
                                      "tag": null, "submodules": null, "glob": null});
        assert!(Declared::from_json(&json, Path::new("/app")).is_err());
    }
}
