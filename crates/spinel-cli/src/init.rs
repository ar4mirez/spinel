//! `spinel init`: scaffold a project in the current directory.
//!
//! Writes a `Gemfile`, a `.spinel-version` naming this Spinel, a `test/`
//! directory, and `.gitignore` entries for `.spinel/` and `.bundle/`
//! (docs/cli.md). It never
//! overwrites: a file that is already there is kept as it is, so running it in
//! an existing project, or twice, is safe. The one file it may change is
//! `.gitignore`, and only by appending the `.spinel/` or `.bundle/` line when
//! no line already decides about that directory.
//!
//! It writes no `.ruby-version`: mise, rbenv, asdf and chruby read that file
//! as a request for a CRuby, and fail on `spinel-<version>` (#902). The
//! Spinel a project was made with goes in `.spinel-version`, which only
//! Spinel reads (`ruby_version.rs`).

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use crate::ruby_version::{SPINEL_VERSION_FILE, spinel_version_file_contents};

/// What `bundle init` writes, so the file reads the same to a Bundler user.
const GEMFILE: &str = "\
# frozen_string_literal: true

source \"https://rubygems.org\"

# gem \"rails\"
";

/// The directories `.gitignore` should ignore, both machine-local and so never
/// committed: `.spinel/`, which every Spinel artifact lives under (the gem
/// home, the bytecode cache, the require map), and `.bundle/`, where
/// `spinel install` writes the `.bundle/config` that points Bundler at it.
const IGNORED: [&str; 2] = [".spinel", ".bundle"];

/// What `init` did with one path, printed one per line.
enum Outcome {
    Created,
    Kept,
    Appended,
}

impl Outcome {
    fn verb(&self) -> &'static str {
        match self {
            Outcome::Created => "create",
            Outcome::Kept => "exists",
            Outcome::Appended => "append",
        }
    }
}

/// Scaffold `dir`, printing what happened to each path. Stops at the first
/// error, having changed nothing it reported as untouched.
pub fn run(dir: &Path) -> io::Result<()> {
    report("Gemfile", create_new(&dir.join("Gemfile"), GEMFILE)?);
    report(
        SPINEL_VERSION_FILE,
        create_new(
            &dir.join(SPINEL_VERSION_FILE),
            &spinel_version_file_contents(),
        )?,
    );
    report("test/", create_dir(&dir.join("test"))?);
    report(".gitignore", ignore_local_dirs(&dir.join(".gitignore"))?);
    Ok(())
}

fn report(name: &str, outcome: Outcome) {
    println!("{:>7}  {name}", outcome.verb());
}

/// Write `contents` to `path` unless something is already there. `create_new`
/// rather than an existence check, so a file that appears in between is not
/// overwritten either.
fn create_new(path: &Path, contents: &str) -> io::Result<Outcome> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(contents.as_bytes())?;
            Ok(Outcome::Created)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(Outcome::Kept),
        Err(err) => Err(err),
    }
}

fn create_dir(path: &Path) -> io::Result<Outcome> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(Outcome::Created),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {
            Ok(Outcome::Kept)
        }
        Err(err) => Err(err),
    }
}

/// Make sure `.gitignore` ignores each of [`IGNORED`]: create it with one line
/// per directory, or append the lines for the directories no existing line
/// decides about. Every other line is left alone; a line that already ignores,
/// or un-ignores, a directory means its entry is not appended.
fn ignore_local_dirs(path: &Path) -> io::Result<Outcome> {
    let existing = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let all: String = IGNORED.iter().map(|dir| format!("{dir}/\n")).collect();
            return create_new(path, &all);
        }
        Err(err) => return Err(err),
    };
    let missing: Vec<&str> = IGNORED
        .into_iter()
        .filter(|dir| {
            !existing
                .split(|&b| b == b'\n')
                .any(|line| decides_about(line.trim_ascii(), dir))
        })
        .collect();
    if missing.is_empty() {
        return Ok(Outcome::Kept);
    }
    let mut file = OpenOptions::new().append(true).open(path)?;
    let mut lines = String::new();
    if existing.last().is_some_and(|&b| b != b'\n') {
        lines.push('\n');
    }
    for dir in missing {
        lines.push_str(dir);
        lines.push_str("/\n");
    }
    file.write_all(lines.as_bytes())?;
    Ok(Outcome::Appended)
}

/// Whether one `.gitignore` line already decides about the top-level directory
/// `dir`: `dir`, with or without a leading or trailing slash. A negation,
/// `!.spinel/`, counts too: the project chose to commit it, and an appended
/// line after it would silently undo that choice.
fn decides_about(line: &[u8], dir: &str) -> bool {
    let line = line.strip_prefix(b"!").unwrap_or(line);
    let line = line.strip_prefix(b"/").unwrap_or(line);
    let line = line.strip_suffix(b"/").unwrap_or(line);
    line == dir.as_bytes()
}

#[cfg(test)]
mod tests {
    use super::decides_about;

    #[test]
    fn spellings_of_the_spinel_directory_count_as_deciding_it() {
        for line in [".spinel", ".spinel/", "/.spinel", "/.spinel/", "!.spinel/"] {
            assert!(decides_about(line.as_bytes(), ".spinel"), "{line}");
        }
        for line in [
            "spinel/",
            ".spinel/gem_home",
            "# .spinel/",
            ".spinelx",
            ".bundle/",
        ] {
            assert!(!decides_about(line.as_bytes(), ".spinel"), "{line}");
        }
        for line in [".bundle", "/.bundle/", "!.bundle/"] {
            assert!(decides_about(line.as_bytes(), ".bundle"), "{line}");
        }
    }
}
