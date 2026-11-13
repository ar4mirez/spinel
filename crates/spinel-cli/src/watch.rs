//! `--watch`: run a program again whenever the project changes (#69,
//! docs/cli.md).
//!
//! The process that was asked to watch never runs Ruby itself. It is a
//! supervisor: it starts `spinel` again, without `--watch`, as a child in a
//! process group of its own, watches the project directory with `notify`, and
//! on a change that `.gitignore` does not exclude it ends that group and
//! starts a fresh child. Nothing is reloaded in place; Zeitwerk and its kind
//! already do that inside an app. `spinel run --watch` builds the child's
//! command line today, and `spinel test --watch` is meant to hand its own to
//! [`run`] the same way.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{CreateKind, RemoveKind};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

/// How long the project must stay quiet after a change before the child is
/// restarted, so an editor's burst of writes for one save restarts it once.
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// How long a child has to end after SIGTERM before its group gets SIGKILL.
const GRACE: Duration = Duration::from_secs(2);

/// How often the supervisor looks at its child while nothing else happens,
/// to report that it ended on its own.
const POLL: Duration = Duration::from_millis(50);

/// What the supervisor's one loop waits on.
enum Message {
    Changed(notify::Result<Event>),
    Signal(i32),
}

/// Watch `root` and keep `spinel <child_args>` running against it until a
/// signal ends the supervisor. Returns the exit code for that signal, as a
/// shell reports a process it ended: 128 plus its number.
pub fn run(root: &Path, child_args: &[OsString]) -> ExitCode {
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(err) => {
            eprintln!("spinel: cannot watch `{}`: {err}", root.display());
            return ExitCode::FAILURE;
        }
    };
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            eprintln!("spinel: cannot find the spinel binary to restart: {err}");
            return ExitCode::FAILURE;
        }
    };

    let (sender, messages) = mpsc::channel();
    let mut signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) {
        Ok(signals) => signals,
        Err(err) => {
            eprintln!("spinel: cannot handle signals for --watch: {err}");
            return ExitCode::FAILURE;
        }
    };
    let on_signal = sender.clone();
    thread::spawn(move || {
        for signal in signals.forever() {
            if on_signal.send(Message::Signal(signal)).is_err() {
                break;
            }
        }
    });

    let mut watcher = match notify::recommended_watcher(move |event| {
        let _ = sender.send(Message::Changed(event));
    }) {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!("spinel: cannot watch `{}`: {err}", root.display());
            return ExitCode::FAILURE;
        }
    };
    // ponytail: one recursive watch covers ignored directories too (`target/`,
    // `node_modules/`), whose events are then dropped by `Ignored`. A tree large
    // enough to hit the inotify watch limit needs a watch per directory that
    // `.gitignore` keeps, added as directories appear.
    if let Err(err) = watcher.watch(&root, RecursiveMode::Recursive) {
        eprintln!("spinel: cannot watch `{}`: {err}", root.display());
        return ExitCode::FAILURE;
    }
    eprintln!("spinel: watching {} for changes", root.display());

    let mut ignored = Ignored::new(root.clone());
    let mut child = Running::start(&program, child_args, &root);
    let mut changed: Vec<PathBuf> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let wait = deadline.map_or(POLL, |at| {
            at.saturating_duration_since(Instant::now()).min(POLL)
        });
        match messages.recv_timeout(wait) {
            Ok(Message::Changed(Ok(event))) => {
                let before = changed.len();
                collect(&event, &mut ignored, &mut changed);
                // Events were lost, so any file may have changed.
                if event.need_rescan() && !changed.contains(&root) {
                    changed.push(root.clone());
                }
                if changed.len() > before {
                    deadline = Some(Instant::now() + DEBOUNCE);
                }
            }
            Ok(Message::Changed(Err(err))) => eprintln!("spinel: warning: watching: {err}"),
            Ok(Message::Signal(signal)) => {
                child.stop();
                return ExitCode::from(128 + u8::try_from(signal).unwrap_or(0));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("spinel: the file watcher stopped");
                child.stop();
                return ExitCode::FAILURE;
            }
        }
        if deadline.is_some_and(|at| Instant::now() >= at) {
            eprintln!("spinel: restarting: {}", describe(&root, &changed));
            changed.clear();
            deadline = None;
            child.stop();
            child = Running::start(&program, child_args, &root);
        }
        child.reap();
    }
}

/// Add each path an event names that `.gitignore` keeps, once, to `changed`.
/// Reading a file is not a change: the child reads the program every time it
/// starts, and counting that would restart it forever.
fn collect(event: &Event, ignored: &mut Ignored, changed: &mut Vec<PathBuf>) {
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }
    let dir_kind = matches!(
        event.kind,
        EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
    );
    for path in &event.paths {
        if path.file_name().is_some_and(|name| name == ".gitignore") {
            ignored.forget(path.parent().unwrap_or(path));
        }
        let is_dir = dir_kind || path.is_dir();
        if !ignored.is_ignored(path, is_dir) && !changed.contains(path) {
            changed.push(path.clone());
        }
    }
}

/// The changed paths for the restart line: relative to the project, the first
/// few named and the rest counted.
fn describe(root: &Path, changed: &[PathBuf]) -> String {
    const NAMED: usize = 3;
    let mut names: Vec<String> = changed
        .iter()
        .take(NAMED)
        .map(|path| {
            let relative = path.strip_prefix(root).unwrap_or(path);
            if relative.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                relative.display().to_string()
            }
        })
        .collect();
    if changed.len() > NAMED {
        names.push(format!("and {} more", changed.len() - NAMED));
    }
    format!("{} changed", names.join(", "))
}

/// The child and the process group it leads.
struct Running {
    child: Option<Child>,
}

impl Running {
    fn start(program: &Path, args: &[OsString], dir: &Path) -> Running {
        // ponytail: the child's group is not the terminal's foreground group,
        // so a program that reads the terminal is stopped by SIGTTIN, and
        // Ctrl-C reaches it only through the supervisor. Handing the terminal
        // to the child's group with `tcsetpgrp` while it runs lifts both.
        let spawned = Command::new(program)
            .args(args)
            .current_dir(dir)
            .process_group(0)
            .spawn();
        match spawned {
            Ok(child) => Running { child: Some(child) },
            Err(err) => {
                eprintln!("spinel: cannot start `{}`: {err}", program.display());
                Running { child: None }
            }
        }
    }

    /// Report the child once it has ended on its own; the supervisor goes on
    /// watching, and the next change starts it again.
    fn reap(&mut self) {
        let Some(child) = &mut self.child else { return };
        match child.try_wait() {
            Ok(Some(status)) => {
                eprintln!(
                    "spinel: the program {}; waiting for a change",
                    ended(status)
                );
                self.child = None;
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("spinel: warning: cannot wait for the program: {err}");
                self.child = None;
            }
        }
    }

    /// End the child's whole process group: SIGTERM, then SIGKILL for whatever
    /// is still there after [`GRACE`]. Only while the child is unreaped, since
    /// until then its pid, which is the group's id, cannot name anyone else.
    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let Ok(group) = libc::pid_t::try_from(child.id()) else {
            let _ = child.kill();
            let _ = child.wait();
            return;
        };
        signal_group(group, libc::SIGTERM);
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < GRACE => thread::sleep(Duration::from_millis(10)),
                _ => {
                    signal_group(group, libc::SIGKILL);
                    let _ = child.wait();
                    break;
                }
            }
        }
        // The child is gone; anything it left in its group goes with it. While
        // the group has a member its id cannot be reused, and pids are handed
        // out in sequence, so the moment since the reap is far too short for
        // an empty group's id to come round to someone else.
        signal_group(group, libc::SIGKILL);
    }
}

fn signal_group(group: libc::pid_t, signal: libc::c_int) {
    // SAFETY: `kill` takes plain integers and touches no memory of ours. A
    // negative pid addresses the process group that `group`, a child of this
    // process, was started to lead (see `Running::stop` for why the id still
    // names it).
    unsafe {
        libc::kill(-group, signal);
    }
}

fn ended(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with status {code}"),
        (None, Some(signal)) => format!("was ended by signal {signal}"),
        (None, None) => "ended".to_owned(),
    }
}

/// Which paths under the project `.gitignore` excludes, as git decides it: a
/// path is ignored when it, or a directory on the way to it, is matched by the
/// nearest rule — `.gitignore` files from deepest to the project's own, then
/// `.git/info/exclude`, then the user's global excludes file. `.git/` itself
/// is always ignored, as git never tracks it.
struct Ignored {
    root: PathBuf,
    /// One matcher per directory that has been asked about, for its own
    /// `.gitignore`; dropped when that file changes.
    per_dir: HashMap<PathBuf, Gitignore>,
    exclude: Gitignore,
    global: Gitignore,
}

impl Ignored {
    fn new(root: PathBuf) -> Ignored {
        let mut exclude = GitignoreBuilder::new(&root);
        exclude.add(root.join(".git/info/exclude"));
        let exclude = exclude.build().unwrap_or_else(|_| Gitignore::empty());
        let (global, _) = GitignoreBuilder::new(&root).build_global();
        Ignored {
            root,
            per_dir: HashMap::new(),
            exclude,
            global,
        }
    }

    fn forget(&mut self, dir: &Path) {
        self.per_dir.remove(dir);
    }

    fn matcher(&mut self, dir: &Path) -> &Gitignore {
        self.per_dir.entry(dir.to_path_buf()).or_insert_with(|| {
            let mut builder = GitignoreBuilder::new(dir);
            // A missing or unreadable `.gitignore` excludes nothing.
            builder.add(dir.join(".gitignore"));
            builder.build().unwrap_or_else(|_| Gitignore::empty())
        })
    }

    fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        // Anything outside the project is not the project's change.
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return true;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts
            .first()
            .is_some_and(|first| first.as_os_str() == ".git")
        {
            return true;
        }
        let mut dir = self.root.clone();
        for (i, part) in parts.iter().enumerate() {
            let candidate = dir.join(part);
            let candidate_is_dir = i + 1 < parts.len() || is_dir;
            if self.excludes(&dir, &candidate, candidate_is_dir) {
                return true;
            }
            dir = candidate;
        }
        false
    }

    /// Whether the rules that apply in `dir` exclude `candidate`, an entry of
    /// `dir`: the first rule that matches decides, deepest file first.
    fn excludes(&mut self, dir: &Path, candidate: &Path, is_dir: bool) -> bool {
        let root = self.root.clone();
        let mut at = Some(dir);
        while let Some(current) = at {
            match self.matcher(current).matched(candidate, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
            if current == root {
                break;
            }
            at = current.parent();
        }
        for rules in [&self.exclude, &self.global] {
            match rules.matched(candidate, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spinel-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir.canonicalize().expect("canonical")
    }

    #[test]
    fn gitignore_rules_apply_as_git_applies_them() {
        let root = scratch("rules");
        fs::write(root.join(".gitignore"), "*.log\nbuild/\n!keep.log\n").unwrap();
        fs::create_dir_all(root.join("lib/build")).unwrap();
        fs::write(root.join("lib/.gitignore"), "gen/\n!build/\n").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/.gitignore"), "!out.rb\n").unwrap();
        fs::create_dir_all(root.join(".git/info")).unwrap();
        fs::write(root.join(".git/info/exclude"), "secret.rb\n").unwrap();
        let mut ignored = Ignored::new(root.clone());

        assert!(!ignored.is_ignored(&root.join("app.rb"), false));
        assert!(ignored.is_ignored(&root.join("debug.log"), false));
        assert!(ignored.is_ignored(&root.join("lib/debug.log"), false));
        assert!(!ignored.is_ignored(&root.join("keep.log"), false));
        assert!(ignored.is_ignored(&root.join("build"), true));
        // A pattern ending in `/` names only directories.
        assert!(!ignored.is_ignored(&root.join("build"), false));
        // A nested `.gitignore` adds rules for its own directory, and a deeper
        // rule wins over a shallower one.
        assert!(ignored.is_ignored(&root.join("lib/gen/x.rb"), false));
        assert!(!ignored.is_ignored(&root.join("lib/x.rb"), false));
        assert!(!ignored.is_ignored(&root.join("lib/build/x.rb"), false));
        // git never reads inside an excluded directory, so nothing in it can
        // be included again.
        assert!(ignored.is_ignored(&root.join("build/out.rb"), false));
        assert!(ignored.is_ignored(&root.join("secret.rb"), false));
        assert!(ignored.is_ignored(&root.join(".git/index"), false));
        assert!(ignored.is_ignored(Path::new("/elsewhere/app.rb"), false));

        // A changed `.gitignore` is read again.
        fs::write(root.join(".gitignore"), "app.rb\n").unwrap();
        ignored.forget(&root);
        assert!(ignored.is_ignored(&root.join("app.rb"), false));
        assert!(!ignored.is_ignored(&root.join("debug.log"), false));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restart_line_names_a_few_paths_and_counts_the_rest() {
        let root = Path::new("/p");
        let paths: Vec<PathBuf> = ["a.rb", "lib/b.rb", "c.rb", "d.rb", "e.rb"]
            .iter()
            .map(|p| root.join(p))
            .collect();
        assert_eq!(describe(root, &paths[..1]), "a.rb changed");
        assert_eq!(
            describe(root, &paths),
            "a.rb, lib/b.rb, c.rb, and 2 more changed"
        );
        assert_eq!(describe(root, &[root.to_path_buf()]), ". changed");
    }
}
