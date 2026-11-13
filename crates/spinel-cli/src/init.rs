//! `spinel init`: scaffold a project in the current directory.
//!
//! Writes a `Gemfile`, a `.ruby-version` naming this Spinel, a `test/`
//! directory, and a `.gitignore` entry for `.spinel/` (docs/cli.md). It never
//! overwrites: a file that is already there is kept as it is, so running it in
//! an existing project, or twice, is safe. The one file it may change is
//! `.gitignore`, and only by appending the `.spinel/` line when no line already
//! ignores that directory.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

/// What `bundle init` writes, so the file reads the same to a Bundler user.
const GEMFILE: &str = "\
# frozen_string_literal: true

source \"https://rubygems.org\"

# gem \"rails\"
";

/// The directory every Spinel artifact lives under: the gem home, the bytecode
/// cache, the require map. Machine-local, so never committed.
const IGNORED: &str = ".spinel/";

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
    let version = format!("spinel-{}\n", spinel_vm::ENGINE_VERSION);

    report("Gemfile", create_new(&dir.join("Gemfile"), GEMFILE)?);
    report(
        ".ruby-version",
        create_new(&dir.join(".ruby-version"), &version)?,
    );
    report("test/", create_dir(&dir.join("test"))?);
    report(".gitignore", ignore_spinel(&dir.join(".gitignore"))?);
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

/// Make sure `.gitignore` ignores `.spinel/`: create it with that one line, or
/// append the line to an existing file that has no line ignoring the
/// directory. Every other line is left alone.
/// A line that already ignores, or un-ignores, `.spinel/` leaves the file as it is.
fn ignore_spinel(path: &Path) -> io::Result<Outcome> {
    let existing = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return create_new(path, &format!("{IGNORED}\n"));
        }
        Err(err) => return Err(err),
    };
    if existing
        .split(|&b| b == b'\n')
        .any(|line| ignores_spinel(line.trim_ascii()))
    {
        return Ok(Outcome::Kept);
    }
    let mut file = OpenOptions::new().append(true).open(path)?;
    let mut line = String::new();
    if existing.last().is_some_and(|&b| b != b'\n') {
        line.push('\n');
    }
    line.push_str(IGNORED);
    line.push('\n');
    file.write_all(line.as_bytes())?;
    Ok(Outcome::Appended)
}

/// Whether one `.gitignore` line already decides about the top-level
/// `.spinel` directory: `.spinel`, with or without a leading or trailing slash.
/// A negation, `!.spinel/`, counts too: the project chose to commit it, and an
/// appended line after it would silently undo that choice.
fn ignores_spinel(line: &[u8]) -> bool {
    let line = line.strip_prefix(b"!").unwrap_or(line);
    let line = line.strip_prefix(b"/").unwrap_or(line);
    let line = line.strip_suffix(b"/").unwrap_or(line);
    line == b".spinel"
}

#[cfg(test)]
mod tests {
    use super::ignores_spinel;

    #[test]
    fn spellings_of_the_spinel_directory_count_as_deciding_it() {
        for line in [".spinel", ".spinel/", "/.spinel", "/.spinel/", "!.spinel/"] {
            assert!(ignores_spinel(line.as_bytes()), "{line}");
        }
        for line in ["spinel/", ".spinel/gem_home", "# .spinel/", ".spinelx"] {
            assert!(!ignores_spinel(line.as_bytes()), "{line}");
        }
    }
}
