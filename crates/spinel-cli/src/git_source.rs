//! The git half of git sources (#88, docs/package-manager.md, "Git and path
//! sources"): the system `git`, run as Bundler 4's `GitProxy` runs it.
//!
//! - **Mirror.** `<home>/git/<base name>-<16 hex>` is a bare clone of the
//!   repository, shared by every project, where Bundler keeps one per bundle
//!   (`cache/bundler/git/<base name>-<sha1>`). It is cloned once, beside
//!   itself and renamed into place, and fetched again only when a revision
//!   must be found anew or a locked one is not in it.
//! - **Revision.** A locked revision is used as it is. Otherwise `git
//!   rev-parse --verify` of the `ref`, `branch` or `tag` in the mirror, or of
//!   `HEAD` (the remote's default branch) when none is given, as
//!   `find_local_revision` finds it: an annotated tag's revision is the tag's
//!   own object, as Bundler locks it.
//! - **Checkout.** `<home>/store/git/<base name>-<revision>[-submodules]/
//!   checkout` is `copy_to`'s clone of the mirror, reset to the revision,
//!   with its submodules when asked for. It is built in `store/tmp/` and
//!   renamed into place, so one that exists is complete, and is shared by
//!   every project as the gem store is.
//!
//! Nothing here goes through a shell: every argument is its own `argv`
//! entry, a URI follows `--` where `git` takes one, and a URI, ref, branch
//! or tag that starts with `-` is refused before `git` sees it, as is a
//! revision that is not a hex object name.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest as _, Sha256};

/// Why a git source could not be read, worded as Bundler's `GitError`s are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError(pub String);

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

/// Refuse what `git` would read as an option.
pub fn check_argument(what: &str, value: &str) -> Result<(), GitError> {
    if value.starts_with('-') {
        return Err(GitError(format!(
            "The git {what} `{value}` starts with `-`, which git would read as an option"
        )));
    }
    if value.contains('\0') || value.contains('\n') {
        return Err(GitError(format!(
            "The git {what} {value:?} has a line break or a NUL in it"
        )));
    }
    Ok(())
}

/// A revision as `git rev-parse` prints one: 40 (SHA-1) or 64 (SHA-256) hex
/// digits. A lockfile's `revision:` is refused otherwise.
pub fn check_revision(revision: &str) -> Result<(), GitError> {
    let hex = revision.bytes().all(|b| b.is_ascii_hexdigit());
    if hex && matches!(revision.len(), 40 | 64) {
        Ok(())
    } else {
        Err(GitError(format!(
            "The git revision `{revision}` is not a full object name"
        )))
    }
}

/// `Source::Git#base_name`: the last part of the URI, without `.git`.
pub fn base_name(uri: &str) -> String {
    // `uri.sub(%r{^(\w+://)?([^/:]+:)?(//\w*/)?(\w*/)*}, "")`.
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = uri.as_bytes();
    let mut i = 0;
    let scheme_end = bytes.iter().take_while(|&&b| word(b)).count();
    if uri[scheme_end..].starts_with("://") {
        i = scheme_end + 3;
    }
    let host_end = i + bytes[i..]
        .iter()
        .take_while(|&&b| b != b'/' && b != b':')
        .count();
    if host_end > i && bytes.get(host_end) == Some(&b':') {
        i = host_end + 1;
    }
    if uri[i..].starts_with("//") {
        let w = bytes[i + 2..].iter().take_while(|&&b| word(b)).count();
        if bytes.get(i + 2 + w) == Some(&b'/') {
            i += 3 + w;
        }
    }
    loop {
        let w = bytes[i..].iter().take_while(|&&b| word(b)).count();
        if bytes.get(i + w) == Some(&b'/') {
            i += w + 1;
        } else {
            break;
        }
    }
    let rest = &uri[i..];
    // `File.basename(rest, ".git")`.
    let last = rest.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    match last.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() => stem.to_owned(),
        _ => last.to_owned(),
    }
}

/// A name safe as one path component: the base name, with anything but
/// letters, digits, `.`, `_` and `-` made `_`, and never `.` or `..`.
fn component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => format!("_{cleaned}"),
        _ => cleaned,
    }
}

/// Run `git` with `args` (in `dir` when given), its standard output on
/// success, else Bundler's `GitCommandError`.
fn git<I, S>(args: I, dir: Option<&Path>) -> Result<String, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    // `capture3_args_for`, and `with_clean_git_env`.
    command
        .args(["-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .args(&args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null());
    let shown = || {
        let words: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        format!("git {}", words.join(" "))
    };
    let out = command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            GitError(
                "You need to install git to be able to use gems from git repositories. \
                 For help installing git, please refer to GitHub's tutorial at \
                 https://help.github.com/articles/set-up-git"
                    .to_owned(),
            )
        } else {
            GitError(format!(
                "Git error: command `{}` could not run: {err}",
                shown()
            ))
        }
    })?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let mut message = format!("Git error: command `{}`", shown());
    if let Some(dir) = dir {
        message.push_str(&format!(" in directory {}", dir.display()));
    }
    message.push_str(" has failed.");
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        message.push('\n');
        message.push_str(err.trim_end());
    }
    Err(GitError(message))
}

/// The mirror of `uri` under `home`.
pub fn mirror_path(home: &Path, uri: &str) -> PathBuf {
    let digest = Sha256::digest(uri.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    home.join("git")
        .join(format!("{}-{hex}", component(&base_name(uri))))
}

/// A new, empty directory in `beside` to build something in, named so the
/// store's sweep removes it once this process is gone: `<pid>-git-<n>`.
fn staging(beside: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(beside).map_err(|err| format!("{}: {err}", beside.display()))?;
    let pid = std::process::id();
    for n in 0u32.. {
        let path = beside.join(format!("{pid}-git-{n}"));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("{}: {err}", path.display())),
        }
    }
    unreachable!("some staging name is free")
}

/// Remove a staging directory, whatever is in it.
fn discard(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

/// A repository's mirror: cloned when there is none yet. Returns its path,
/// and whether it was cloned just now (so it is as fresh as a fetch).
pub fn mirror(home: &Path, uri: &str) -> Result<(PathBuf, bool), GitError> {
    check_argument("source", uri)?;
    let path = mirror_path(home, uri);
    if path.join("HEAD").is_file() {
        return Ok((path, false));
    }
    let parent = path.parent().expect("a mirror has a parent");
    fs::create_dir_all(parent).map_err(|err| GitError(format!("{}: {err}", parent.display())))?;
    let tmp = staging(&home.join("store").join("tmp")).map_err(GitError)?;
    // `git clone` wants a directory that is not there, or empty.
    // ponytail: a full clone, where Bundler clones `--depth 1
    // --single-branch` when it can. Upgrade with `GitProxy#depth` and its
    // `--unshallow` when a locked revision is deeper.
    let cloned = git(
        [
            OsStr::new("clone"),
            OsStr::new("--bare"),
            OsStr::new("--no-hardlinks"),
            OsStr::new("--quiet"),
            OsStr::new("--"),
            OsStr::new(uri),
            tmp.as_os_str(),
        ],
        None,
    );
    if let Err(err) = cloned {
        discard(&tmp);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp, &path) {
        discard(&tmp);
        // Another `spinel` cloned it first; theirs is as good.
        if !path.join("HEAD").is_file() {
            return Err(GitError(format!("{}: {err}", path.display())));
        }
    }
    Ok((path, true))
}

/// Bring the mirror's branches and tags up to date with `uri`, and `extra`
/// (a ref under `refs/` that is neither) with them.
pub fn fetch(mirror: &Path, uri: &str, extra: Option<&str>) -> Result<(), GitError> {
    check_argument("source", uri)?;
    let mut args = vec![
        "fetch".to_owned(),
        "--force".to_owned(),
        "--quiet".to_owned(),
        "--no-tags".to_owned(),
        "--".to_owned(),
        uri.to_owned(),
        "refs/heads/*:refs/heads/*".to_owned(),
        "refs/tags/*:refs/tags/*".to_owned(),
    ];
    if let Some(reference) = extra {
        check_argument("ref", reference)?;
        args.push(format!("{reference}:{reference}"));
    }
    git(&args, Some(mirror)).map(|_| ())
}

/// Fetch one object by name, for a locked revision no branch or tag leads
/// to any more.
pub fn fetch_revision(mirror: &Path, uri: &str, revision: &str) -> Result<(), GitError> {
    check_argument("source", uri)?;
    check_revision(revision)?;
    git(
        [
            "fetch",
            "--force",
            "--quiet",
            "--no-tags",
            "--",
            uri,
            &format!("{revision}:refs/spinel/{revision}"),
        ],
        Some(mirror),
    )
    .map(|_| ())
}

/// Whether the mirror has the object `revision` (`has_revision_cached?`).
pub fn has_revision(mirror: &Path, revision: &str) -> bool {
    check_revision(revision).is_ok() && git(["cat-file", "-e", revision], Some(mirror)).is_ok()
}

/// `find_local_revision`: the object `reference` names in the mirror, or
/// `HEAD` when it is `None`; a reference that names nothing is Bundler's
/// `MissingGitRevisionError`.
pub fn rev_parse(mirror: &Path, uri: &str, reference: Option<&str>) -> Result<String, GitError> {
    let name = reference.unwrap_or("HEAD");
    check_argument("ref", name)?;
    match git(["rev-parse", "--verify", "--quiet", name], Some(mirror)) {
        Ok(out) => {
            let revision = out.trim().to_owned();
            check_revision(&revision)?;
            Ok(revision)
        }
        Err(_) => Err(missing_revision(name, uri)),
    }
}

/// `MissingGitRevisionError`'s words.
pub fn missing_revision(reference: &str, uri: &str) -> GitError {
    GitError(format!(
        "Revision {reference} does not exist in the repository {uri}. Maybe you misspelled it?"
    ))
}

/// Where the checkout of `revision` lives in the store under `home`.
pub fn checkout_entry(home: &Path, uri: &str, revision: &str, submodules: bool) -> PathBuf {
    let mut name = format!("{}-{revision}", component(&base_name(uri)));
    if submodules {
        name.push_str("-submodules");
    }
    home.join("store").join("git").join(name)
}

/// `GitProxy#copy_to` into a staging directory under `home`'s store: a
/// clone of the mirror whose `origin` is `uri` again, reset to `revision`,
/// with its submodules when asked for. `finish` runs on the staged checkout
/// (the gemspecs are evaluated and serialized there) before the entry is
/// renamed into place. An entry already there is kept, and `finish` is not
/// run. Returns the entry.
pub fn checkout(
    home: &Path,
    mirror: &Path,
    uri: &str,
    revision: &str,
    submodules: bool,
    finish: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    check_argument("source", uri).map_err(|e| e.0)?;
    check_revision(revision).map_err(|e| e.0)?;
    let entry = checkout_entry(home, uri, revision, submodules);
    if entry.join("checkout").is_dir() {
        return Ok(entry);
    }
    let tmp = staging(&home.join("store").join("tmp"))?;
    let work = tmp.join("checkout");
    let made = (|| {
        git(
            [
                OsStr::new("clone"),
                OsStr::new("--no-checkout"),
                OsStr::new("--quiet"),
                OsStr::new("--"),
                mirror.as_os_str(),
                work.as_os_str(),
            ],
            None,
        )
        .map_err(|e| e.0)?;
        git(["remote", "set-url", "origin", uri], Some(&work)).map_err(|e| e.0)?;
        git(["reset", "--hard", "--quiet", revision], Some(&work)).map_err(|e| e.0)?;
        if submodules {
            git(
                ["submodule", "update", "--init", "--recursive"],
                Some(&work),
            )
            .map_err(|e| e.0)?;
        }
        finish(&work)
    })();
    if let Err(err) = made {
        discard(&tmp);
        return Err(err);
    }
    let parent = entry.parent().expect("an entry has a parent");
    fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    if let Err(err) = fs::rename(&tmp, &entry) {
        discard(&tmp);
        if !entry.join("checkout").is_dir() {
            return Err(format!("{}: {err}", entry.display()));
        }
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_names_are_bundlers() {
        for (uri, base) in [
            ("https://github.com/rack/rack.git", "rack"),
            ("https://github.com/rack/rack", "rack"),
            ("git@github.com:rails/rails.git", "rails"),
            ("file:///tmp/repos/foo", "foo"),
            ("/abs/path/to/bar.git", "bar"),
            ("../relative/baz", "baz"),
            ("https://example.com/a/b/c.git/", "c"),
        ] {
            assert_eq!(base_name(uri), base, "{uri}");
        }
    }

    #[test]
    fn options_and_odd_revisions_are_refused() {
        assert!(check_argument("ref", "--upload-pack=x").is_err());
        assert!(check_argument("ref", "main").is_ok());
        assert!(check_argument("source", "-oProxyCommand=x").is_err());
        assert!(check_revision("d7ea43fc3548bd4c1d198faff07de727daa1ac40").is_ok());
        assert!(check_revision("d7ea43f").is_err());
        assert!(check_revision("--hard").is_err());
    }

    #[test]
    fn components_stay_one_component() {
        assert_eq!(component(".."), "_..");
        assert_eq!(component("a/b"), "a_b");
        assert_eq!(component("rack"), "rack");
    }
}
