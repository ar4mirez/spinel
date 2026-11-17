//! `spinel test --watch` (#97, docs/test-runner.md, "Watch"): `--watch`'s
//! supervisor (`watch.rs`) running `spinel test` as its child, each change
//! mapped to the tests it affects.
//!
//! - A changed `test/**/*_test.rb` or `spec/**/*_spec.rb`, as discovery finds
//!   them, runs itself.
//! - A changed `app/**/NAME.rb` or `lib/**/NAME.rb` runs every
//!   `test/**/NAME_test.rb` and `spec/**/NAME_spec.rb`, by name alone.
//! - Anything else, including a file of those two kinds with no test to run,
//!   runs what failed last; when nothing did, nothing runs, and a line says
//!   so.
//! - Enter, a line on standard input, runs every test, or the paths given.
//!
//! What failed comes from the runner itself: each run is given a file to
//! record, once it has ended, the location that runs each failure, error and
//! problem again (`shims/test_runner.rb`, step 9), the same `rerun` its
//! report lists them by. The supervisor keeps the set across runs: a run
//! replaces what is known about the files it ran and leaves the rest. A run
//! stopped by a change before it ended has its tests run with the next one.
//! `.spinel/`, where every run writes its timings and the bytecode cache, is
//! not the project's change.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};
use std::process::{ExitCode, ExitStatus};

use crate::test_runner::Reporter;
use crate::watch::{self, Next, Target};

/// Watch the current directory and run `spinel test` with `jobs` and
/// `reporter` as each change asks, first with `paths`.
pub(crate) fn run(jobs: Option<u16>, reporter: Reporter, paths: &[String]) -> ExitCode {
    let root = match std::env::current_dir().and_then(|dir| dir.canonicalize()) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("spinel: cannot read the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let record = match Record::create() {
        Ok(record) => record,
        Err(err) => {
            eprintln!("spinel: cannot make a file to keep what failed in: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut given = Files::new();
    for path in paths {
        let (file, part) = location(path);
        add(&mut given, normal(&root, &file), part);
    }
    let tests = Tests {
        jobs,
        reporter,
        paths: paths.to_vec(),
        given,
        failed: Files::new(),
        running: None,
        record,
    };
    watch::run(&root, tests)
}

/// Which tests of a file to run: all of them, or those at a line, as
/// `FILE:LINE` names them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Whole,
    Line(u64),
}

/// Files by path relative to the project, as the runner names them.
type Files = BTreeMap<String, Part>;

/// What a run runs.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Selection {
    /// Every test, or the paths `spinel test --watch` was given.
    Every,
    Files(Files),
}

/// Add `part` of `file`: one file runs once, whole unless every part asked
/// of it is the same line, as the runner takes a file at most once.
fn add(files: &mut Files, file: String, part: Part) {
    files
        .entry(file)
        .and_modify(|have| {
            if *have != part {
                *have = Part::Whole;
            }
        })
        .or_insert(part);
}

/// `path` or `path:LINE`, split as the runner splits it.
fn location(arg: &str) -> (String, Part) {
    if let Some((path, line)) = arg.rsplit_once(':')
        && !path.is_empty()
        && !line.is_empty()
        && line.bytes().all(|b| b.is_ascii_digit())
        && let Ok(line) = line.parse()
    {
        return (path.to_owned(), Part::Line(line));
    }
    (arg.to_owned(), Part::Whole)
}

/// `file` and its part as `spinel test` takes them.
fn argument(file: &str, part: Part) -> String {
    match part {
        Part::Whole => file.to_owned(),
        Part::Line(line) => format!("{file}:{line}"),
    }
}

/// A path given, relative to the project as the runner's `relative` names
/// it (`./test/a_test.rb` is `test/a_test.rb`), or as given when it is
/// outside it.
fn normal(root: &Path, path: &str) -> String {
    let mut full = PathBuf::new();
    for component in root.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                full.pop();
            }
            other => full.push(other),
        }
    }
    match full.strip_prefix(root) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => path.to_owned(),
    }
}

/// The supervisor's view of the tests.
struct Tests {
    jobs: Option<u16>,
    reporter: Reporter,
    /// The paths given, as given, and by file.
    paths: Vec<String>,
    given: Files,
    /// What failed, by file, as the runs so far left it.
    failed: Files,
    /// What the child that has not been seen to end was started to run.
    running: Option<Selection>,
    record: Record,
}

impl Tests {
    /// The command line that runs `selection`, recording what failed afresh.
    fn launch(&mut self, selection: Selection) -> Vec<OsString> {
        self.record.clear();
        let mut args: Vec<OsString> = vec!["test".into()];
        if let Some(jobs) = self.jobs {
            args.push("-j".into());
            args.push(jobs.to_string().into());
        }
        args.push("--reporter".into());
        args.push(self.reporter.name().into());
        args.push("--record-failures".into());
        args.push(self.record.file.clone().into());
        // After `--`, a file named like a flag is still the file.
        args.push("--".into());
        match &selection {
            Selection::Every => args.extend(self.paths.iter().map(Into::into)),
            Selection::Files(files) => args.extend(
                files
                    .iter()
                    .map(|(file, part)| argument(file, *part).into()),
            ),
        }
        self.running = Some(selection);
        args
    }

    /// `selection`, with what the running child was to run when it is to be
    /// stopped before it ends, so no change's tests go unrun.
    fn with_unfinished(&self, selection: Selection, running: bool) -> Selection {
        match (
            selection,
            running.then_some(self.running.as_ref()).flatten(),
        ) {
            (Selection::Files(mut files), Some(Selection::Files(unfinished))) => {
                for (file, part) in unfinished {
                    add(&mut files, file.clone(), *part);
                }
                Selection::Files(files)
            }
            (Selection::Files(_), Some(Selection::Every)) | (Selection::Every, _) => {
                Selection::Every
            }
            (selection, None) => selection,
        }
    }

    /// What a run of `selection` that recorded `failed` leaves known: what
    /// it ran is replaced, the rest kept.
    fn update(&mut self, selection: &Selection, failed: Vec<String>) {
        match selection {
            Selection::Every if self.given.is_empty() => self.failed.clear(),
            Selection::Every => self.failed.retain(|file, _| !self.given.contains_key(file)),
            Selection::Files(files) => self.failed.retain(|file, _| !files.contains_key(file)),
        }
        for rerun in failed {
            let (file, part) = location(&rerun);
            add(&mut self.failed, file, part);
        }
    }

    /// The tests a change to `path`, under `root`, maps to by name, or
    /// `None` when it maps to none.
    fn mapped(root: &Path, path: &Path) -> Option<Vec<String>> {
        let relative = path.strip_prefix(root).ok()?;
        let top = relative.components().next()?.as_os_str().to_str()?;
        let name = relative.file_name()?.to_str()?;
        let is_test = match top {
            "test" => name.ends_with("_test.rb"),
            "spec" => name.ends_with("_spec.rb"),
            _ => false,
        };
        if is_test {
            let file = relative.to_str()?;
            return path.is_file().then(|| vec![file.to_owned()]);
        }
        if !matches!(top, "app" | "lib") || relative.extension().is_none_or(|ext| ext != "rb") {
            return None;
        }
        let stem = relative.file_stem()?.to_str()?;
        let mut found = Vec::new();
        for (dir, suffix) in [("test", "_test.rb"), ("spec", "_spec.rb")] {
            let wanted = format!("{stem}{suffix}");
            named(root, &root.join(dir), &wanted, &mut found);
        }
        if found.is_empty() {
            return None;
        }
        found.sort();
        Some(found)
    }
}

/// Every file named `wanted` under `dir`, relative to `root`, into `found`,
/// as `Dir.glob("dir/**/wanted")` finds them: a symlinked directory is not
/// gone into.
fn named(root: &Path, dir: &Path, wanted: &str, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            named(root, &path, wanted, found);
        } else if entry.file_name() == wanted
            && path.is_file()
            && let Some(relative) = path.strip_prefix(root).ok().and_then(Path::to_str)
        {
            found.push(relative.to_owned());
        }
    }
}

/// The selection's files for a line: the first few named and the rest
/// counted.
fn listed(files: &Files) -> String {
    const NAMED: usize = 3;
    let mut names: Vec<String> = files
        .iter()
        .take(NAMED)
        .map(|(file, part)| argument(file, *part))
        .collect();
    if files.len() > NAMED {
        names.push(format!("and {} more", files.len() - NAMED));
    }
    names.join(", ")
}

impl Target for Tests {
    fn first(&mut self) -> Vec<OsString> {
        self.launch(Selection::Every)
    }

    fn changed(&mut self, root: &Path, changed: &[PathBuf], running: bool) -> Next {
        let mut files = Files::new();
        let mut other = false;
        for path in changed {
            match Tests::mapped(root, path) {
                Some(found) => {
                    for file in found {
                        add(&mut files, file, Part::Whole);
                    }
                }
                None => other = true,
            }
        }
        if other {
            for (file, part) in &self.failed {
                // A file deleted since it failed has nothing left to run.
                if root.join(file).is_file() {
                    add(&mut files, file.clone(), *part);
                }
            }
        }
        let what = watch::describe(root, changed);
        if files.is_empty() {
            return Next {
                line: format!(
                    "spinel: {what}; nothing failed last time, so no test runs (Enter runs every test)"
                ),
                args: None,
            };
        }
        let selection = self.with_unfinished(Selection::Files(files), running);
        let line = match &selection {
            Selection::Every => format!("spinel: {what}; running every test"),
            Selection::Files(files) => format!("spinel: {what}; running {}", listed(files)),
        };
        Next {
            line,
            args: Some(self.launch(selection)),
        }
    }

    fn enter(&mut self, _running: bool) -> Next {
        Next {
            line: "spinel: running every test".to_owned(),
            args: Some(self.launch(Selection::Every)),
        }
    }

    fn reads_enter(&self) -> bool {
        true
    }

    fn ignores(&self, relative: &Path) -> bool {
        relative.starts_with(".spinel")
    }

    fn ended(&mut self, status: ExitStatus) -> String {
        // A run that recorded nothing (a discovery error, a crash of the
        // runner itself) leaves what was known.
        if let Some(selection) = self.running.take()
            && let Some(failed) = self.record.read()
        {
            self.update(&selection, failed);
        }
        format!(
            "spinel: the tests {}; waiting for a change, or Enter to run every test",
            watch::ended(status)
        )
    }
}

/// The file each run records what failed in, in a directory of the
/// supervisor's own under the system's temporary directory, removed with it.
struct Record {
    dir: PathBuf,
    file: PathBuf,
}

impl Record {
    fn create() -> std::io::Result<Record> {
        let temp = std::env::temp_dir();
        let mut last = None;
        // The first free name is claimed by creating it, readable by this
        // user alone, so no one else's file can stand in for it.
        for n in 0..100 {
            let dir = temp.join(format!("spinel-test-watch-{}-{n}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    return Ok(Record {
                        file: dir.join("failed.json"),
                        dir,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => last = Some(err),
                Err(err) => return Err(err),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("no free name")))
    }

    fn clear(&self) {
        let _ = fs::remove_file(&self.file);
    }

    /// What the last run recorded: `{"version": 1, "failed": [location]}`.
    fn read(&self) -> Option<Vec<String>> {
        let text = fs::read_to_string(&self.file).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        if value.get("version")?.as_u64()? != 1 {
            return None;
        }
        value
            .get("failed")?
            .as_array()?
            .iter()
            .map(|rerun| rerun.as_str().map(str::to_owned))
            .collect()
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_splits_as_the_runner_splits_it() {
        assert_eq!(
            location("test/a_test.rb"),
            ("test/a_test.rb".into(), Part::Whole)
        );
        assert_eq!(
            location("test/a_test.rb:12"),
            ("test/a_test.rb".into(), Part::Line(12))
        );
        assert_eq!(
            location("test/a:b_test.rb"),
            ("test/a:b_test.rb".into(), Part::Whole)
        );
        assert_eq!(location(":12"), (":12".into(), Part::Whole));
    }

    #[test]
    fn a_file_asked_for_at_two_lines_runs_whole() {
        let mut files = Files::new();
        add(&mut files, "a".into(), Part::Line(3));
        add(&mut files, "a".into(), Part::Line(3));
        assert_eq!(files["a"], Part::Line(3));
        add(&mut files, "a".into(), Part::Line(8));
        assert_eq!(files["a"], Part::Whole);
        add(&mut files, "a".into(), Part::Line(3));
        assert_eq!(files["a"], Part::Whole);
    }

    #[test]
    fn a_path_given_is_named_as_the_runner_names_it() {
        let root = Path::new("/p");
        assert_eq!(normal(root, "./test/a_test.rb"), "test/a_test.rb");
        assert_eq!(normal(root, "/p/test/x/../a_test.rb"), "test/a_test.rb");
        assert_eq!(normal(root, "/elsewhere/a_test.rb"), "/elsewhere/a_test.rb");
    }
}
