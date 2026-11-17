//! Running a compiled app (#101, docs/build.md "Runtime behavior of a
//! compiled app").
//!
//! A `spinel` whose trailer says it carries a payload is that app and nothing
//! else: its command line is the app's, so `mytool --help` is the app's help.
//! The payload is extracted once, to `$XDG_CACHE_HOME/spinel/apps/<sha256>/`
//! (`~/.cache` when `XDG_CACHE_HOME` is unset or relative, as the XDG base
//! directory spec says), and every later run finds it there, without reading
//! or hashing the payload again: the directory is named by the hash the
//! payload was checked against when it was written.
//!
//! First runs may race, so each extracts into a directory of its own beside
//! the target and renames it into place: `rename(2)` makes it appear whole or
//! not at all, and a run that loses the race uses the winner's copy and
//! removes its own. The payload is hashed as it is extracted, so a first run
//! reads it once; a payload that does not match its trailer is refused and
//! nothing of it is kept.
//!
//! Nothing in the archive is trusted to say where it goes: an absolute path,
//! a `..`, or any entry but a file or a directory (a link, a device) stops
//! the extraction. `spinel build` writes none of those, since it copies what
//! every symlink leads to.
//!
//! The payload's bytecode cache was compiled under each file's path in the
//! payload (`app/lib/greet.rb`), since the directory it lands in is not known
//! when it is built; each entry is rebased onto the extraction directory as it
//! is written (`spinel_vm::sbc::rebase`), so the first run is already warm.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use sha2::{Digest, Sha256};

use crate::trailer::{self, App};

/// `manifest.json`, as `spinel build` writes it (`build.rs`). The version
/// and extension ABI are not read: the payload is packed into the `spinel`
/// that wrote it, so they are this binary's.
#[derive(serde::Deserialize)]
struct Manifest {
    /// The program to run, relative to `app/`, `/`-separated.
    entry: String,
    /// What `ARGV` starts with, before the app's own arguments.
    #[serde(default)]
    argv: Vec<String>,
    /// Set in the environment before the app runs.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// Run the compiled app `file` holds: extract its payload unless it is
/// already, then run its entry point as `spinel run` would.
pub fn run(file: &mut File, app: App, exe: &Path) -> ExitCode {
    let root = match extracted(file, app) {
        Ok(root) => root,
        Err(Failure::Payload(err)) => {
            eprintln!("spinel: `{}`: {err}", exe.display());
            return ExitCode::FAILURE;
        }
        Err(Failure::Extract { dir, err }) => {
            eprintln!(
                "spinel: `{}` is a compiled app ({} byte payload, sha256 {}), \
                 but its payload cannot be extracted to `{}`: {err}",
                exe.display(),
                app.trailer.payload_len,
                trailer::hex(&app.trailer.sha256),
                dir.display(),
            );
            return ExitCode::FAILURE;
        }
    };
    let manifest = match read_manifest(&root) {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!(
                "spinel: `{}`: {}: {err}",
                exe.display(),
                root.join("manifest.json").display()
            );
            return ExitCode::FAILURE;
        }
    };
    let entry = root.join("app").join(&manifest.entry);
    let source = match fs::read(&entry) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("spinel: cannot read `{}`: {err}", entry.display());
            return ExitCode::FAILURE;
        }
    };
    // The manifest's arguments, then the app's own, whatever they are:
    // nothing on this command line is Spinel's.
    let mut args = manifest.argv.clone();
    args.extend(
        std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned()),
    );
    let setup = setup(&root, &manifest);
    let around = crate::Around {
        prelude: Some(&setup),
        ..crate::Around::default()
    };
    crate::run_program(&entry, &source, &args, false, around)
}

/// What `shims/setup.rb` does for `spinel run`, for the extracted app rather
/// than for a project found from the current directory: its gems, its
/// directory on the load path, its environment, and its bytecode cache. Run
/// before the gem prelude, so RubyGems starts with these paths.
fn setup(root: &Path, manifest: &Manifest) -> String {
    let path = |dir: &str| crate::ruby_literal(&root.join(dir).to_string_lossy());
    let gem_home = path("gem_home");
    let mut code = format!(
        "ENV[\"GEM_HOME\"] = {gem_home}\n\
         ENV[\"GEM_PATH\"] = {gem_home}\n"
    );
    for (key, value) in &manifest.env {
        code.push_str(&format!(
            "ENV[{}] = {}\n",
            crate::ruby_literal(key),
            crate::ruby_literal(value)
        ));
    }
    code.push_str(&format!(
        "$LOAD_PATH.unshift({})\n__bytecode_cache__({})\n",
        path("app"),
        path("bytecode")
    ));
    code
}

fn read_manifest(root: &Path) -> io::Result<Manifest> {
    let bytes = fs::read(root.join("manifest.json"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if relative(Path::new(&manifest.entry)).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the entry point `{}` is not inside `app/`", manifest.entry),
        ));
    }
    Ok(manifest)
}

/// Why the payload could not be made ready to run.
enum Failure {
    /// The payload does not match its trailer.
    Payload(trailer::Error),
    /// The payload is intact but could not be written out.
    Extract { dir: PathBuf, err: io::Error },
}

/// The directory the payload is extracted to, extracting it first if no run
/// has: `<cache>/spinel/apps/<sha256>/`, as its real path, the one `require`
/// resolves files under and the bytecode cache's entries are rebased onto.
fn extracted(file: &mut File, app: App) -> Result<PathBuf, Failure> {
    let hex = trailer::hex(&app.trailer.sha256);
    let Some(apps) = cache_home().map(|cache| cache.join("spinel").join("apps")) else {
        return Err(Failure::Extract {
            dir: PathBuf::from("$XDG_CACHE_HOME/spinel/apps"),
            err: io::Error::new(
                io::ErrorKind::NotFound,
                "neither XDG_CACHE_HOME nor HOME is set",
            ),
        });
    };
    let fail = |err| Failure::Extract {
        dir: apps.join(&hex),
        err,
    };
    // The cache hit: one `stat`, and the real path.
    if let Ok(root) = fs::canonicalize(apps.join(&hex))
        && root.is_dir()
    {
        return Ok(root);
    }
    fs::create_dir_all(&apps).map_err(fail)?;
    let apps = fs::canonicalize(&apps).map_err(fail)?;
    let target = apps.join(&hex);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let staging = apps.join(format!(".{hex}.{}.{nanos}.tmp", std::process::id()));
    let unpacked = unpack(file, app, &staging, &target);
    let placed = unpacked.and_then(|()| fs::rename(&staging, &target).map_err(Failure::from));
    match placed {
        Ok(()) => Ok(target),
        Err(err) => {
            let _ = fs::remove_dir_all(&staging);
            // Another run placed its copy first: that one is as good, since
            // it was checked against the same hash.
            if matches!(err, Failure::Extract { .. }) && target.is_dir() {
                return Ok(target);
            }
            Err(match err {
                Failure::Extract { err, .. } => fail(err),
                payload => payload,
            })
        }
    }
}

impl From<io::Error> for Failure {
    fn from(err: io::Error) -> Failure {
        Failure::Extract {
            dir: PathBuf::new(),
            err,
        }
    }
}

/// `$XDG_CACHE_HOME`, or `~/.cache` when it is unset, empty or relative.
fn cache_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        return Some(dir);
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(".cache"))
}

/// Extract the payload into `staging`, which it makes, hashing every byte of
/// it as it goes. Bytecode entries are rebased onto `root`, where `staging`
/// will be renamed to. A payload that does not match its trailer is
/// `Failure::Payload` whatever else went wrong reading it, since a corrupt
/// payload is what any other failure would then be down to.
fn unpack(file: &mut File, app: App, staging: &Path, root: &Path) -> Result<(), Failure> {
    file.seek(SeekFrom::Start(app.payload_offset))?;
    let mut payload = Hashing {
        inner: io::Read::take(&mut *file, app.trailer.payload_len),
        hasher: Sha256::new(),
        read: 0,
    };
    let unpacked = unpack_archive(&mut payload, staging, root);
    // The rest of the payload, past the end of the archive, still counts.
    let drained = io::copy(&mut payload, &mut io::sink());
    if payload.read != app.trailer.payload_len {
        return Err(Failure::Payload(trailer::Error::Truncated {
            declared: app.trailer.payload_len,
            available: payload.read,
        }));
    }
    let actual: [u8; 32] = payload.hasher.finalize().into();
    if actual != app.trailer.sha256 {
        return Err(Failure::Payload(trailer::Error::Corrupt {
            expected: app.trailer.sha256,
            actual,
        }));
    }
    unpacked?;
    drained?;
    Ok(())
}

/// The tar inside the zstd stream `payload`, written under `staging`.
fn unpack_archive(payload: &mut impl Read, staging: &Path, root: &Path) -> io::Result<()> {
    let root = root.to_string_lossy();
    fs::create_dir(staging)?;
    let mut archive = tar::Archive::new(zstd::Decoder::new(payload)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let named = entry.path()?.into_owned();
        let Some(path) = relative(&named) else {
            return Err(unsafe_entry(&named, "a path outside the payload"));
        };
        let dest = staging.join(&path);
        match entry.header().entry_type() {
            tar::EntryType::Directory => fs::create_dir_all(&dest)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                let executable = entry.header().mode()? & 0o111 != 0;
                if path.starts_with("bytecode") {
                    // Read whole to rewrite: one compiled `.rb` file.
                    let mut bytes = Vec::new();
                    entry.read_to_end(&mut bytes)?;
                    // An entry that cannot be rebased is left out; the file
                    // is compiled when it is required, as on any miss.
                    if let Some(rebased) = spinel_vm::sbc::rebase(&bytes, &root) {
                        create(&dest, false)?.write_all(&rebased)?;
                    }
                } else {
                    let mut out = io::BufWriter::new(create(&dest, executable)?);
                    io::copy(&mut entry, &mut out)?;
                    out.flush()?;
                }
            }
            // A pax global header describes the archive, not a file.
            tar::EntryType::XGlobalHeader => {}
            _ => return Err(unsafe_entry(&named, "neither a file nor a directory")),
        }
    }
    Ok(())
}

/// A new file at `path`, never one already there: an archive naming the
/// same file twice is not one `spinel build` wrote.
fn create(path: &Path, executable: bool) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if executable { 0o755 } else { 0o644 })
        .open(path)
}

/// `path` as plain names under the directory it is relative to, or `None`
/// when it is absolute, climbs out with `..`, or names nothing.
fn relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn unsafe_entry(path: &Path, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the payload's entry `{}` is {what}", path.display()),
    )
}

/// The payload as it is read, every byte fed into a sha256.
struct Hashing<R> {
    inner: R,
    hasher: Sha256,
    read: u64,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let got = self.inner.read(buf)?;
        self.hasher.update(&buf[..got]);
        self.read += got as u64;
        Ok(got)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_are_relative() {
        assert_eq!(
            relative(Path::new("app/lib/greet.rb")),
            Some(PathBuf::from("app/lib/greet.rb"))
        );
        assert_eq!(relative(Path::new("./app")), Some(PathBuf::from("app")));
        assert_eq!(relative(Path::new("/etc/passwd")), None);
        assert_eq!(relative(Path::new("app/../../x")), None);
        assert_eq!(relative(Path::new("..")), None);
        assert_eq!(relative(Path::new(".")), None);
        assert_eq!(relative(Path::new("")), None);
    }
}
