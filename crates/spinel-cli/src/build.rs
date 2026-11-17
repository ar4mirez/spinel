//! `spinel build --compile` (#100, docs/build.md): assemble a project's
//! payload and write it, with its trailer, into a copy of this `spinel`.
//!
//! The payload is one `tar` stream compressed with zstd, written straight to
//! a file beside the output and hashed as it is written, so a payload of any
//! size is never held in memory. It holds, in this order:
//!
//! ```text
//! manifest.json   entry point, Spinel version, spinel-ext ABI, argv, env
//! app/            the project's files: what .gitignore keeps, as git decides
//!                 it, plus --include, minus --exclude
//! gem_home/       the gems the app needs from .spinel/gem_home, every symlink
//!                 resolved into a real copy
//! bytecode/       the bytecode cache for every .rb file above, warmed
//! ```
//!
//! The core image and the standard library are inside the `spinel` binary the
//! payload is packed into, so neither is copied. `ext/`, the `.spinel`
//! extensions for the target, is phase 5 (#103-#105); a payload has none.
//!
//! The archive is the same bytes for the same project: entries in a fixed
//! order (each directory's names sorted), every mtime 0, every owner root
//! with no names, and every mode 0755 or 0644 by whether the file is
//! executable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use sha2::{Digest, Sha256};

use crate::install::{GemfileJson, Stop};
use crate::lockfile::Lockfile;
use crate::trailer::{self, Payload, Trailer};
use crate::watch::Ignored;

/// The GEM_HOME's subdirectories as `gem install` lays one out, each made in
/// the payload's `gem_home/` (`gem_home.rs`, `SUBDIRECTORIES`). `ruby/<ver>/`
/// and `spinel/<ver>/`, the views Bundler reads through `BUNDLE_PATH`, are
/// symlinks to these and are not copied.
const GEM_HOME_DIRS: [&str; 8] = [
    "bin",
    "build_info",
    "cache",
    "doc",
    "extensions",
    "gems",
    "plugins",
    "specifications",
];

/// What `spinel build` was asked for.
pub struct Request<'a> {
    /// The program the app runs, as given: relative to the current directory.
    pub entry: &'a Path,
    pub output: &'a Path,
    /// Gitignore-style patterns, relative to the project, of files to take
    /// though `.gitignore` leaves them out.
    pub include: &'a [String],
    /// Gitignore-style patterns of files to leave out whatever else says.
    pub exclude: &'a [String],
    /// Gemfile groups whose gems go in besides `default`'s.
    pub groups: &'a [String],
    /// `KEY=VALUE` pairs for the manifest's `env`.
    pub env: &'a [String],
    /// The arguments the app's `ARGV` starts with.
    pub argv: &'a [String],
}

/// `manifest.json`, the payload's first entry.
#[derive(serde::Serialize)]
struct Manifest<'a> {
    /// The program to run, relative to `app/`, `/`-separated.
    entry: String,
    /// The Spinel version that wrote the payload, whose binary it is packed
    /// into.
    spinel_version: &'static str,
    /// The `spinel-ext` ABI that binary loads extensions for.
    spinel_ext_abi: u32,
    /// The arguments `ARGV` starts with, before those the app is run with.
    argv: &'a [String],
    /// The variables set in the app's environment before it runs.
    env: BTreeMap<&'a str, &'a str>,
}

pub fn run(request: &Request<'_>) -> ExitCode {
    match build(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop(code)) => code,
    }
}

fn fail(message: impl std::fmt::Display) -> Stop {
    Stop::failure("build", message)
}

fn build(request: &Request<'_>) -> Result<(), Stop> {
    let mut env = BTreeMap::new();
    for pair in request.env {
        match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() => {
                env.insert(key, value);
            }
            _ => return Err(fail(format!("--env {pair}: expected KEY=VALUE"))),
        }
    }

    let cwd = std::env::current_dir()
        .and_then(fs::canonicalize)
        .map_err(|err| fail(format!("cannot read the current directory: {err}")))?;
    // The project is the Gemfile's directory, found as `spinel install`
    // finds it; a program with no Gemfile is a project of the current
    // directory with no gems.
    let gemfile = if std::env::var_os("BUNDLE_GEMFILE").is_some() || find_gemfile(&cwd) {
        Some(crate::install::evaluate_gemfile("build", None, None)?)
    } else {
        None
    };
    let gemfile_path = gemfile
        .as_ref()
        .and_then(|g| g.gemfiles.first())
        .map(PathBuf::from);
    let root = match &gemfile_path {
        Some(path) => {
            let dir = path.parent().unwrap_or(Path::new("."));
            fs::canonicalize(cwd.join(dir))
                .map_err(|err| fail(format!("{}: {err}", dir.display())))?
        }
        None => cwd.clone(),
    };

    // The output and the directory the payload is assembled in, beside it,
    // so a project that writes its app inside itself does not pack either.
    let output = normalize(&cwd.join(request.output));
    let Some(name) = output.file_name() else {
        return Err(fail(format!(
            "-o {}: not a file name",
            request.output.display()
        )));
    };
    let parent = output.parent().unwrap_or(Path::new("/"));
    fs::create_dir_all(parent).map_err(|err| fail(format!("{}: {err}", parent.display())))?;
    let parent =
        fs::canonicalize(parent).map_err(|err| fail(format!("{}: {err}", parent.display())))?;
    let output = parent.join(name);
    let mut staging_name = name.to_os_string();
    staging_name.push(format!(".spinel-build-{}", std::process::id()));
    let staging = Staging::new(parent.join(staging_name))
        .map_err(|err| fail(format!("{}: {err}", parent.display())))?;

    let mut filter = AppFilter::new(&root, request.include, request.exclude).map_err(fail)?;
    let entry = entry_point(&cwd, &root, request.entry, &mut filter).map_err(fail)?;

    // The gems the app needs, read before anything is written, so a project
    // that is not installed stops with nothing made.
    let gems = match (&gemfile, &gemfile_path) {
        (Some(gemfile), Some(path)) => {
            let lock_path = crate::install::lockfile_path(path);
            let text = match fs::read_to_string(&lock_path) {
                Ok(text) => Some(text),
                Err(err) if err.kind() == io::ErrorKind::NotFound => None,
                Err(err) => return Err(fail(format!("{}: {err}", lock_path.display()))),
            };
            match text {
                Some(text) => {
                    let local = crate::gem_platform::Platform::local(spinel_vm::PLATFORM);
                    let lock_name = lock_path.display().to_string();
                    let lock = Lockfile::parse(&text, &lock_name, &local).map_err(|err| {
                        eprintln!("{err}");
                        Stop(ExitCode::FAILURE)
                    })?;
                    Some(Gems::needed(&root, gemfile, &lock, request.groups))
                }
                None if gemfile.dependencies.is_empty() => None,
                None => {
                    return Err(fail(format!(
                        "{} is missing; run `spinel install` first",
                        lock_path.display()
                    )));
                }
            }
        }
        _ => None,
    };

    let manifest = Manifest {
        entry,
        spinel_version: env!("CARGO_PKG_VERSION"),
        spinel_ext_abi: spinel_ext::ABI,
        argv: request.argv,
        env,
    };
    let payload_path = staging.0.join("payload.tar.zst");
    let bytecode = staging.0.join("bytecode");
    let trailer = assemble(
        &payload_path,
        &bytecode,
        &manifest,
        &mut filter,
        gems.as_ref(),
        vec![output.clone(), staging.0.clone()],
    )
    .map_err(fail)?;

    let binary = std::env::current_exe()
        .map_err(|err| fail(format!("cannot find this executable: {err}")))?;
    let mut payload = Payload {
        bytes: File::open(&payload_path)
            .map_err(|err| fail(format!("{}: {err}", payload_path.display())))?,
        trailer,
    };
    trailer::write(&binary, &mut payload, &output)
        .map_err(|err| fail(format!("{}: {err}", output.display())))?;
    println!(
        "{}: {} byte payload, sha256 {}",
        request.output.display(),
        trailer.payload_len,
        trailer::hex(&trailer.sha256)
    );
    Ok(())
}

/// Whether a Gemfile or `gems.rb` is in `dir` or above it, where the
/// Gemfile evaluator looks.
fn find_gemfile(dir: &Path) -> bool {
    dir.ancestors()
        .any(|dir| dir.join("Gemfile").is_file() || dir.join("gems.rb").is_file())
}

/// `path` with `.` and `..` taken out by spelling alone, so a symlink on the
/// way is still the project's own name for it.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The entry point relative to `app/`, `/`-separated: a file in the project
/// that the app's files include.
fn entry_point(
    cwd: &Path,
    root: &Path,
    entry: &Path,
    filter: &mut AppFilter,
) -> Result<String, String> {
    let path = normalize(&cwd.join(entry));
    let shown = entry.display();
    if !fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
        return Err(format!("--compile {shown}: no such file"));
    }
    let Ok(relative) = path.strip_prefix(root) else {
        return Err(format!(
            "--compile {shown}: not in the project at {}",
            root.display()
        ));
    };
    if !filter.keeps(&path, false) {
        return Err(format!(
            "--compile {shown}: .gitignore or --exclude leaves it out of the app; --include it"
        ));
    }
    Ok(relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// Write the payload to `path` and return its trailer.
fn assemble(
    path: &Path,
    bytecode: &Path,
    manifest: &Manifest<'_>,
    filter: &mut AppFilter,
    gems: Option<&Gems>,
    skip: Vec<PathBuf>,
) -> Result<Trailer, String> {
    let file = File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let hashed = Hashed {
        inner: io::BufWriter::new(file),
        hasher: Sha256::new(),
        len: 0,
    };
    let encoder = zstd::Encoder::new(hashed, 0).map_err(|err| err.to_string())?;
    let mut walk = Walk {
        tar: tar::Builder::new(encoder),
        bytecode: bytecode.to_path_buf(),
        stack: Vec::new(),
        skip,
    };

    let mut json = serde_json::to_vec_pretty(manifest).map_err(|err| err.to_string())?;
    json.push(b'\n');
    walk.bytes(Path::new("manifest.json"), 0o644, &json)?;

    walk.dir(Path::new("app"))?;
    let root = filter.root.clone();
    walk.tree(
        &root,
        Path::new("app"),
        Seen::Kept,
        &mut |path, is_dir, seen| filter.decide(path, is_dir, seen),
    )?;

    walk.dir(Path::new("gem_home"))?;
    if let Some(gems) = gems {
        gems.copy(&mut walk, filter)?;
    }

    // The cache, once every `.rb` file is compiled into it.
    walk.dir(Path::new("bytecode"))?;
    let mut cached: Vec<_> = match fs::read_dir(bytecode) {
        Ok(dir) => dir
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| !name.to_string_lossy().starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    };
    cached.sort();
    for name in cached {
        let source = bytecode.join(&name);
        let meta = fs::metadata(&source).map_err(|err| format!("{}: {err}", source.display()))?;
        walk.stream(&source, &Path::new("bytecode").join(&name), &meta)?;
    }

    let encoder = walk.tar.into_inner().map_err(|err| err.to_string())?;
    let hashed = encoder.finish().map_err(|err| err.to_string())?;
    let Hashed { inner, hasher, len } = hashed;
    let file = inner.into_inner().map_err(|err| err.error().to_string())?;
    file.sync_all().map_err(|err| err.to_string())?;
    Ok(Trailer {
        sha256: hasher.finalize().into(),
        payload_len: len,
    })
}

/// A directory's entries as the walk takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// Taken.
    Kept,
    /// Under a directory `.gitignore` leaves out, walked only for the files
    /// an `--include` takes back.
    Hidden,
}

/// Which of the project's files go in `app/`.
struct AppFilter {
    root: PathBuf,
    ignored: Ignored,
    include: Gitignore,
    exclude: Gitignore,
}

impl AppFilter {
    fn new(root: &Path, include: &[String], exclude: &[String]) -> Result<AppFilter, String> {
        let patterns = |flag: &str, lines: &[String]| {
            let mut builder = GitignoreBuilder::new(root);
            for line in lines {
                builder
                    .add_line(None, line)
                    .map_err(|err| format!("--{flag} {line}: {err}"))?;
            }
            builder.build().map_err(|err| format!("--{flag}: {err}"))
        };
        Ok(AppFilter {
            root: root.to_path_buf(),
            ignored: Ignored::new(root.to_path_buf()),
            include: patterns("include", include)?,
            exclude: patterns("exclude", exclude)?,
        })
    }

    /// Whether `path`, in the project, goes in `app/`: each directory on the
    /// way, then `path` itself, as the walk decides them.
    fn keeps(&mut self, path: &Path, is_dir: bool) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let mut at = self.root.clone();
        let parts: Vec<_> = relative.components().collect();
        for (i, part) in parts.iter().enumerate() {
            at.push(part);
            let part_is_dir = i + 1 < parts.len() || is_dir;
            match self.decide(&at, part_is_dir, Seen::Kept) {
                Some(Seen::Kept) => {}
                _ => return false,
            }
        }
        true
    }

    /// How the walk takes `path`, an entry of a directory it took as
    /// `parent`: `None` to leave it out. `.git/` and `.spinel/` are never the
    /// app's. `--exclude` wins; an `--include` takes a file back from
    /// `.gitignore`.
    fn decide(&mut self, path: &Path, is_dir: bool, parent: Seen) -> Option<Seen> {
        let relative = path.strip_prefix(&self.root).ok()?;
        if relative
            .components()
            .next()
            .is_some_and(|first| first.as_os_str() == ".spinel")
            || relative.components().any(|part| part.as_os_str() == ".git")
        {
            return None;
        }
        if self
            .exclude
            .matched_path_or_any_parents(path, is_dir)
            .is_ignore()
        {
            return None;
        }
        let includes = !self.include.is_empty();
        if includes
            && self
                .include
                .matched_path_or_any_parents(path, is_dir)
                .is_ignore()
        {
            return Some(Seen::Kept);
        }
        if parent == Seen::Hidden || self.ignored.is_ignored(path, is_dir) {
            // An ignored directory is walked only for what an `--include`
            // may take back from it.
            return (is_dir && includes).then_some(Seen::Hidden);
        }
        Some(Seen::Kept)
    }
}

/// The locked gems the app needs, by name, and where they are installed.
struct Gems {
    home: PathBuf,
    /// Every locked build of each needed gem, by full name, with the gem's
    /// name; the ones installed are copied.
    builds: BTreeMap<String, String>,
    names: BTreeSet<String>,
    /// The names that must be installed: every needed gem but those reached
    /// only through a dependency limited to some platforms.
    required: BTreeSet<String>,
}

impl Gems {
    /// The gems in the Gemfile's `default` group and in `groups`, and every
    /// locked gem they reach, as `Bundler.setup(:default, *groups)` loads
    /// them.
    fn needed(root: &Path, gemfile: &GemfileJson, lock: &Lockfile, groups: &[String]) -> Gems {
        let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
        for spec in lock.sources.iter().flat_map(|source| &source.specs) {
            deps.entry(spec.name.as_str())
                .or_default()
                .extend(spec.dependencies.iter().map(|(name, _)| name.as_str()));
        }
        let reach = |roots: Vec<&str>| {
            let mut found = BTreeSet::new();
            let mut todo = roots;
            while let Some(name) = todo.pop() {
                if found.insert(name.to_owned()) {
                    todo.extend(deps.get(name).into_iter().flatten().copied());
                }
            }
            found
        };
        let wanted: Vec<_> = gemfile
            .dependencies
            .iter()
            .filter(|dep| {
                dep.install_if
                    && dep
                        .groups
                        .iter()
                        .any(|group| group == "default" || groups.contains(group))
            })
            .collect();
        let names = reach(wanted.iter().map(|dep| dep.name.as_str()).collect());
        let required = reach(
            wanted
                .iter()
                .filter(|dep| dep.platforms.is_empty())
                .map(|dep| dep.name.as_str())
                .collect(),
        );
        let builds = lock
            .sources
            .iter()
            .flat_map(|source| &source.specs)
            .filter(|spec| names.contains(&spec.name))
            .map(|spec| (spec.full_name(), spec.name.clone()))
            .collect();
        Gems {
            home: root.join(".spinel/gem_home"),
            builds,
            names,
            required,
        }
    }

    /// Copy what the GEM_HOME has of these gems into `gem_home/`, following
    /// every symlink: each gem's directory, its gemspec, the `bin/` stubs of
    /// its executables, and the git checkouts Bundler finds it in. A path gem
    /// whose directory `app/` takes, as a `gemspec` project's own is, takes
    /// the files `app/` does.
    fn copy(&self, walk: &mut Walk, filter: &mut AppFilter) -> Result<(), String> {
        let dest = Path::new("gem_home");
        for dir in GEM_HOME_DIRS {
            walk.dir(&dest.join(dir))?;
        }
        let mut installed = BTreeSet::new();
        let mut gem_dirs = Vec::new();
        for (full_name, name) in &self.builds {
            let dir = self.home.join("gems").join(full_name);
            if !fs::metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
                continue;
            }
            installed.insert(name.as_str());
            let resolved =
                fs::canonicalize(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            let target = dest.join("gems").join(full_name);
            // ponytail: a `gemspec` project's files go in twice, under `app/`
            // and here, and each `.rb` is compiled twice. Upgrade, if payloads
            // show it: copy only the gemspec's `files` for a path gem, or
            // point the path gem at `app/`.
            if filter.keeps(&resolved, true) {
                walk.tree(&resolved, &target, Seen::Kept, &mut |path, is_dir, seen| {
                    filter.decide(path, is_dir, seen)
                })?;
            } else {
                walk.tree(&dir, &target, Seen::Kept, &mut gem_file)?;
            }
            gem_dirs.push(resolved);
            let spec = self
                .home
                .join("specifications")
                .join(format!("{full_name}.gemspec"));
            if let Ok(meta) = fs::metadata(&spec) {
                walk.file(
                    &spec,
                    &dest
                        .join("specifications")
                        .join(format!("{full_name}.gemspec")),
                    &meta,
                )?;
            }
        }
        let missing: Vec<&str> = self
            .required
            .iter()
            .map(String::as_str)
            .filter(|name| *name != "bundler" && !installed.contains(name))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "{} not installed in {}; run `spinel install` first",
                missing.join(", "),
                self.home.display()
            ));
        }

        // `bin/`: the stubs `spinel install` wrote for these gems.
        for (name, path) in sorted_entries(&self.home.join("bin")) {
            let Some(exe) = name.to_str() else { continue };
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if crate::gem_home::stub_gem(&text, spinel_vm::ENGINE, exe)
                .is_some_and(|gem| self.names.contains(&gem))
            {
                let meta =
                    fs::metadata(&path).map_err(|err| format!("{}: {err}", path.display()))?;
                walk.file(&path, &dest.join("bin").join(exe), &meta)?;
            }
        }

        // `bundler/gems/`: a git gem's checkout, where Bundler looks for it.
        // ponytail: its files are copied twice, here and under `gems/`, as
        // both are symlinks to the one checkout. Upgrade, if payloads with
        // git gems show it: one copy, with the other entry a hard link in
        // the tar.
        let mut made = false;
        for (name, path) in sorted_entries(&self.home.join("bundler/gems")) {
            let Ok(checkout) = fs::canonicalize(&path) else {
                continue;
            };
            if !gem_dirs.iter().any(|dir| dir.starts_with(&checkout)) {
                continue;
            }
            if !made {
                walk.dir(&dest.join("bundler"))?;
                walk.dir(&dest.join("bundler/gems"))?;
                made = true;
            }
            walk.tree(
                &path,
                &dest.join("bundler/gems").join(&name),
                Seen::Kept,
                &mut gem_file,
            )?;
        }
        Ok(())
    }
}

/// A gem's files are all its own but a repository's `.git` and a project's
/// `.spinel`, which a path gem's directory may have.
fn gem_file(path: &Path, _is_dir: bool, _parent: Seen) -> Option<Seen> {
    let name = path.file_name()?;
    (name != ".git" && name != ".spinel").then_some(Seen::Kept)
}

/// A directory's entries, by name; none when it cannot be read.
fn sorted_entries(dir: &Path) -> Vec<(std::ffi::OsString, PathBuf)> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map(|dir| {
            dir.flatten()
                .map(|entry| (entry.file_name(), entry.path()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

/// The payload's tar stream, and where its `.rb` files are compiled to.
struct Walk {
    tar: tar::Builder<zstd::Encoder<'static, Hashed<io::BufWriter<File>>>>,
    bytecode: PathBuf,
    /// The directories being copied, resolved, to stop at a symlink that
    /// leads back into one of them.
    stack: Vec<PathBuf>,
    /// The app being written and its staging directory, never taken.
    skip: Vec<PathBuf>,
}

impl Walk {
    /// Copy the directory `src` to `dest`, taking each entry `decide` keeps
    /// and following symlinks, so each is copied as what it leads to.
    fn tree(
        &mut self,
        src: &Path,
        dest: &Path,
        seen: Seen,
        decide: &mut dyn FnMut(&Path, bool, Seen) -> Option<Seen>,
    ) -> Result<(), String> {
        let resolved = fs::canonicalize(src).map_err(|err| format!("{}: {err}", src.display()))?;
        if self.stack.contains(&resolved) {
            return Err(format!(
                "{}: a symlink leads back into a directory it is in",
                src.display()
            ));
        }
        self.stack.push(resolved.clone());
        let entries = fs::read_dir(src)
            .and_then(Iterator::collect::<io::Result<Vec<_>>>)
            .map_err(|err| format!("{}: {err}", src.display()))?;
        let mut entries: Vec<_> = entries
            .into_iter()
            .map(|entry| (entry.file_name(), entry.path()))
            .collect();
        entries.sort();
        for (name, path) in entries {
            if self.skip.contains(&resolved.join(&name)) {
                continue;
            }
            let meta = fs::metadata(&path);
            let is_dir = meta.as_ref().is_ok_and(Metadata::is_dir);
            let Some(next) = decide(&path, is_dir, seen) else {
                continue;
            };
            let meta = meta.map_err(|err| match fs::symlink_metadata(&path) {
                Ok(link) if link.file_type().is_symlink() => {
                    format!("{}: a symlink that leads nowhere", path.display())
                }
                _ => format!("{}: {err}", path.display()),
            })?;
            let target = dest.join(&name);
            if is_dir {
                self.tree(&path, &target, next, decide)?;
            } else if next == Seen::Kept && meta.is_file() {
                self.file(&path, &target, &meta)?;
            }
        }
        self.stack.pop();
        Ok(())
    }

    /// The file `src` at `dest`; a `.rb` file is compiled into the cache too.
    fn file(&mut self, src: &Path, dest: &Path, meta: &Metadata) -> Result<(), String> {
        if dest.extension().is_some_and(|ext| ext == "rb") {
            let source = fs::read(src).map_err(|err| format!("{}: {err}", src.display()))?;
            self.bytes(dest, mode(meta), &source)?;
            warm(&self.bytecode, &dest.to_string_lossy(), &source);
            Ok(())
        } else {
            self.stream(src, dest, meta)
        }
    }

    /// The file `src` at `dest`, read as it is written.
    fn stream(&mut self, src: &Path, dest: &Path, meta: &Metadata) -> Result<(), String> {
        let file = File::open(src).map_err(|err| format!("{}: {err}", src.display()))?;
        let len = file
            .metadata()
            .map_err(|err| format!("{}: {err}", src.display()))?
            .len();
        let mut header = header(tar::EntryType::Regular, mode(meta), len);
        self.tar
            .append_data(
                &mut header,
                dest,
                Exact {
                    inner: file,
                    left: len,
                },
            )
            .map_err(|err| format!("{}: {err}", src.display()))
    }

    fn bytes(&mut self, dest: &Path, mode: u32, bytes: &[u8]) -> Result<(), String> {
        let mut header = header(tar::EntryType::Regular, mode, bytes.len() as u64);
        self.tar
            .append_data(&mut header, dest, bytes)
            .map_err(|err| format!("{}: {err}", dest.display()))
    }

    fn dir(&mut self, dest: &Path) -> Result<(), String> {
        let mut header = header(tar::EntryType::Directory, 0o755, 0);
        self.tar
            .append_data(&mut header, dest, io::empty())
            .map_err(|err| format!("{}: {err}", dest.display()))
    }
}

/// A header with nothing of the machine it was written on in it.
fn header(kind: tar::EntryType, mode: u32, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_size(size);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header
}

/// 0755 for a file anyone may execute, 0644 for any other.
fn mode(meta: &Metadata) -> u32 {
    if meta.permissions().mode() & 0o111 == 0 {
        0o644
    } else {
        0o755
    }
}

/// Compile `source`, as `require` would compile it under `path`, into the
/// cache in `dir`. A file that does not parse or compile is left to be
/// compiled, and to fail, when it is required.
// ponytail: compiled under its path in the payload (`app/lib/x.rb`), since
// where the app is extracted (`$XDG_CACHE_HOME/spinel/apps/<hash>/`) depends
// on the payload's hash, which this cache is part of. Running the app (#101)
// reads the entries rebased onto that directory.
fn warm(dir: &Path, path: &str, source: &[u8]) {
    let Ok(parsed) = spinel_core::parse_required(path, source) else {
        return;
    };
    let Ok(iseq) = spinel_vm::compile::program_with(&parsed.program, "<top (required)>", None)
    else {
        return;
    };
    spinel_vm::sbc::write(dir, path, source, None, &iseq, &parsed.warnings);
}

/// Exactly `left` bytes of `inner`: a file that shrinks while it is read is
/// an error rather than an entry shorter than its header says.
struct Exact<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Exact<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let want = usize::try_from(self.left).map_or(buf.len(), |left| left.min(buf.len()));
        let got = self.inner.read(&mut buf[..want])?;
        if got == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the file changed while it was read",
            ));
        }
        self.left -= got as u64;
        Ok(got)
    }
}

/// Counts and hashes what is written through it: the payload's trailer.
struct Hashed<W> {
    inner: W,
    hasher: Sha256,
    len: u64,
}

impl<W: Write> Write for Hashed<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.len += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The directory a payload is assembled in, removed with all it holds when
/// the build ends, whether it wrote the app or not.
struct Staging(PathBuf);

impl Staging {
    fn new(path: PathBuf) -> io::Result<Staging> {
        // A directory this process left at the same name is its own.
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path)?;
        Ok(Staging(path))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
