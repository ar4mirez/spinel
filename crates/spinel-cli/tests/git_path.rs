//! Git and path sources (#88, docs/package-manager.md, "Git and path
//! sources"), through `spinel install`, `update` and `add`.
//!
//! Each test makes a git repository at test time from
//! `tests/fixtures/git_path/widget-<version>/` (widget, which depends on
//! shared), committed with a fixed identity and date as
//! `tests/fixtures/git_path/bundler.sh` commits it, and puts
//! `tests/fixtures/git_path/gadget/` beside the project as a path gem.
//! `shared` comes from the gems `tests/fixtures/update/build.sh` builds,
//! served on 127.0.0.1 with a compact index written from their checksums.
//! Every lockfile and Gemfile is compared, byte for byte, with what Bundler
//! 4.0.20 wrote for the same steps (`bundler.sh` records them in
//! `fixtures/git_path/bundler/`, the source, repository and revisions
//! spelled as placeholders).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use md5::Md5;
use sha2::{Digest as _, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/git_path")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A gem source on 127.0.0.1 serving `shared` 1.0.0 and 2.0.0: `GET` of
/// `/versions`, `/info/shared` and `/gems/<file>`, nothing more.
struct Server {
    url: String,
}

impl Server {
    fn start() -> Server {
        let gems = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update/gems");
        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        let mut info = b"---\n".to_vec();
        let mut versions = Vec::new();
        for version in ["1.0.0", "2.0.0"] {
            let file = format!("shared-{version}.gem");
            let body = fs::read(gems.join(&file)).unwrap();
            info.extend(
                format!("{version} |checksum:{}\n", hex(&Sha256::digest(&body))).into_bytes(),
            );
            versions.push(version);
            files.insert(format!("/gems/{file}"), body);
        }
        let list = format!(
            "created_at: 2026-01-01T00:00:00Z\n---\nshared {} {}\n",
            versions.join(","),
            hex(&Md5::digest(&info))
        );
        files.insert("/info/shared".to_owned(), info);
        files.insert("/versions".to_owned(), list.into_bytes());
        let files = Arc::new(Mutex::new(files));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let files = Arc::clone(&files);
                std::thread::spawn(move || serve(stream, &files));
            }
        });
        Server { url }
    }
}

fn serve(stream: TcpStream, files: &Mutex<HashMap<String, Vec<u8>>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
    }
    let body = files.lock().unwrap().get(&path).cloned();
    let (status, body) = match body {
        Some(body) => ("200 OK", body),
        None => ("404 Not Found", b"not found".to_vec()),
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&body);
}

/// Copy a directory tree, keeping each file's mode.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

/// `git` in `dir`, with the identity, date and environment `bundler.sh`
/// commits with; its standard output.
fn git(dir: &Path, args: &[&str], date: &str) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Spinel")
        .env("GIT_AUTHOR_EMAIL", "spinel@example.invalid")
        .env("GIT_COMMITTER_NAME", "Spinel")
        .env("GIT_COMMITTER_EMAIL", "spinel@example.invalid")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A project with a git repository and a path gem beside it, a Spinel home
/// of its own, and the server.
struct Sandbox {
    root: PathBuf,
    server: Server,
    revisions: Vec<String>,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("git_path")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        fs::create_dir_all(root.join("repo")).unwrap();
        copy_dir(&fixtures().join("gadget"), &root.join("gadget"));
        Sandbox {
            root,
            server: Server::start(),
            revisions: Vec::new(),
        }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn repo_url(&self) -> String {
        format!("file://{}", self.root.join("repo").display())
    }

    /// Commit `fixtures/git_path/<tree>/` over what the repository has, as
    /// `bundler.sh` does, and note its revision.
    fn commit(&mut self, tree: &str) -> String {
        let repo = self.root.join("repo");
        let date = format!("2026-01-0{}T00:00:00Z", self.revisions.len() + 1);
        if !repo.join(".git").exists() {
            git(
                &repo,
                &["-c", "init.defaultBranch=main", "init", "-q"],
                &date,
            );
        }
        git(&repo, &["rm", "-rq", "--ignore-unmatch", "."], &date);
        copy_dir(&fixtures().join(tree), &repo);
        git(&repo, &["add", "-A"], &date);
        git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", tree],
            &date,
        );
        let revision = git(&repo, &["rev-parse", "HEAD"], &date).trim().to_owned();
        self.revisions.push(revision.clone());
        revision
    }

    /// What Bundler wrote, `fixtures/git_path/bundler/<name>`, for this
    /// sandbox.
    fn bundlers(&self, name: &str) -> String {
        let mut text = fs::read_to_string(fixtures().join("bundler").join(name))
            .unwrap()
            .replace("@SOURCE@", &self.server.url)
            .replace("@REPO@", &self.repo_url());
        for (i, revision) in self.revisions.iter().enumerate() {
            text = text.replace(&format!("@REVISION_{}@", i + 1), revision);
        }
        text
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.project().join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    fn try_read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.project().join(name)).ok()
    }

    fn spinel_in(&self, dir: &Path, home: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(SPINEL);
        command.current_dir(dir).env("SPINEL_HOME", home).args(args);
        for var in [
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "all_proxy",
            "ALL_PROXY",
            "GEM_HOME",
            "GEM_PATH",
            "BUNDLE_GEMFILE",
            "BUNDLE_PATH",
            "BUNDLE_APP_CONFIG",
            "BUNDLE_FROZEN",
            "RUBYOPT",
            "RUBYLIB",
            "GIT_DIR",
            "GIT_WORK_TREE",
        ] {
            command.env_remove(var);
        }
        command.output().expect("spinel runs")
    }

    fn spinel(&self, args: &[&str]) -> Output {
        self.spinel_in(&self.project(), &self.home(), args)
    }

    /// The Gemfile `bundler.sh` locks.
    fn gemfile(&self) {
        self.write("Gemfile", &self.bundlers("Gemfile"));
    }

    /// The GEM_HOME's `gems/`, each entry with where its symlink leads.
    fn linked(&self) -> Vec<(String, PathBuf)> {
        let mut names: Vec<(String, PathBuf)> =
            fs::read_dir(self.project().join(".spinel/gem_home/gems"))
                .map(|dir| {
                    dir.flatten()
                        .map(|e| {
                            (
                                e.file_name().to_string_lossy().into_owned(),
                                fs::read_link(e.path()).unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
        names.sort();
        names
    }
}

/// Its standard output, after checking it exited 0 and said nothing on
/// standard error.
fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "{what} ({:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Its standard error, after checking it exited with `code`.
fn failed(out: &Output, code: i32, what: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(code),
        "{what}\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    stderr
}

#[test]
fn a_git_and_a_path_source_install_and_lock() {
    let mut sandbox = Sandbox::new("install");
    let revision = sandbox.commit("widget-1.0.0");
    sandbox.gemfile();
    assert_eq!(
        ok(&sandbox.spinel(&["install"]), "spinel install"),
        "+ gadget 0.1.0\n+ shared 2.0.0\n+ widget 1.0.0\n"
    );
    // The lockfile Bundler wrote: the revision `main` is at, the path as
    // written, the rubygems gem the git gem depends on.
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        sandbox.bundlers("installed.lock")
    );

    // The git gem is the checkout of its revision in the store, its
    // gemspec evaluated and serialized there as Bundler leaves it; the
    // GEM_HOME has the checkout where Bundler looks for it too.
    let checkout = sandbox
        .home()
        .join(format!("store/git/repo-{revision}/checkout"));
    let gadget = sandbox.root.join("gadget");
    let linked = sandbox.linked();
    let names: Vec<&str> = linked.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["gadget-0.1.0", "shared-2.0.0", "widget-1.0.0"]);
    assert_eq!(linked[2].1, checkout);
    let gemspec = fs::read_to_string(checkout.join("widget.gemspec")).unwrap();
    assert!(
        gemspec.starts_with("# -*- encoding: utf-8 -*-\n# stub: widget 1.0.0 ruby lib\n"),
        "{gemspec}"
    );
    assert!(gemspec.contains("s.files = [\"lib/widget.rb\".freeze, \"lib/widget/version.rb\".freeze, \"widget.gemspec\".freeze]"), "{gemspec}");
    assert_eq!(
        fs::read_link(sandbox.project().join(format!(
            ".spinel/gem_home/bundler/gems/repo-{}",
            &revision[..12]
        )))
        .unwrap(),
        checkout
    );
    // The path gem is read where it is: linked to its directory, and
    // nothing of it in the store.
    assert_eq!(linked[0].1, gadget);
    let stored: Vec<String> = fs::read_dir(sandbox.home().join("store/gems"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        stored.iter().all(|s| s.starts_with("shared-")),
        "{stored:?}"
    );
    assert!(
        sandbox
            .project()
            .join(".spinel/gem_home/bin/gadget")
            .is_file()
    );
    assert!(
        sandbox
            .project()
            .join(".spinel/gem_home/specifications/widget-1.0.0.gemspec")
            .is_file()
    );

    // Both load through RubyGems, as any installed gem does.
    let script = "require \"widget\"; require \"gadget\"; puts Widget::VERSION, Gadget::NAME";
    assert_eq!(
        ok(&sandbox.spinel(&["run", "-e", script]), "spinel run"),
        "1.0.0\ngadget\n"
    );
    // A change to the path gem is there without an install.
    fs::write(
        gadget.join("lib/gadget.rb"),
        "module Gadget\n  NAME = \"changed in place\"\nend\n",
    )
    .unwrap();
    assert_eq!(
        ok(
            &sandbox.spinel(&["run", "-e", "require \"gadget\"; puts Gadget::NAME"]),
            "spinel run"
        ),
        "changed in place\n"
    );

    // Again: nothing to do, nothing written.
    let lock = sandbox.read("Gemfile.lock");
    assert_eq!(
        ok(&sandbox.spinel(&["install"]), "spinel install again"),
        ""
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert_eq!(
        ok(
            &sandbox.spinel(&["install", "--frozen"]),
            "spinel install --frozen"
        ),
        ""
    );
}

#[test]
fn a_locked_revision_is_kept_until_the_gem_is_updated() {
    let mut sandbox = Sandbox::new("revision");
    sandbox.commit("widget-1.0.0");
    sandbox.gemfile();
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let installed = sandbox.bundlers("installed.lock");
    assert_eq!(sandbox.read("Gemfile.lock"), installed);

    // `main` moves on; the lockfile keeps its revision.
    let newer = sandbox.commit("widget-1.1.0");
    assert_eq!(ok(&sandbox.spinel(&["install"]), "spinel install"), "");
    assert_eq!(sandbox.read("Gemfile.lock"), installed);
    assert_eq!(sandbox.linked()[2].0, "widget-1.0.0");

    // Bundler's lockfile in another project, with a Spinel home that has
    // never seen the repository: installed at the locked revision, though
    // `main` is past it, and left byte for byte as Bundler wrote it.
    let other = sandbox.root.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("Gemfile"), sandbox.bundlers("Gemfile")).unwrap();
    fs::write(other.join("Gemfile.lock"), &installed).unwrap();
    let other_home = sandbox.root.join("other-home");
    assert_eq!(
        ok(
            &sandbox.spinel_in(&other, &other_home, &["install", "--frozen"]),
            "spinel install --frozen from Bundler's lockfile"
        ),
        "+ gadget 0.1.0\n+ shared 2.0.0\n+ widget 1.0.0\n"
    );
    assert_eq!(
        fs::read_to_string(other.join("Gemfile.lock")).unwrap(),
        installed
    );

    // `spinel update widget` lets the revision go: `main`'s now, with the
    // version it has, and no warning, as the source is not the one it was
    // locked from.
    assert_eq!(
        ok(
            &sandbox.spinel(&["update", "widget"]),
            "spinel update widget"
        ),
        "+ widget 1.1.0\n- widget 1.0.0\n"
    );
    let updated = sandbox.read("Gemfile.lock");
    assert_eq!(updated, sandbox.bundlers("updated.lock"));
    assert!(updated.contains(&newer));
    let script = "require \"widget\"; puts Widget::VERSION";
    assert_eq!(
        ok(&sandbox.spinel(&["run", "-e", script]), "spinel run"),
        "1.1.0\n"
    );
    // The old checkout is no longer where Bundler looks.
    let checkouts: Vec<String> =
        fs::read_dir(sandbox.project().join(".spinel/gem_home/bundler/gems"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
    assert_eq!(checkouts, [format!("repo-{}", &newer[..12])]);
}

#[test]
fn a_changed_path_gemspec_is_read_again() {
    let mut sandbox = Sandbox::new("path");
    sandbox.commit("widget-1.0.0");
    sandbox.gemfile();
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let gemspec = sandbox.root.join("gadget/gadget.gemspec");
    let text = fs::read_to_string(&gemspec).unwrap();
    fs::write(
        &gemspec,
        text.replace("s.version = \"0.1.0\"", "s.version = \"0.2.0\""),
    )
    .unwrap();

    // Frozen, the lockfile no longer holds the Gemfile: Bundler's words.
    let lock = sandbox.read("Gemfile.lock");
    let err = failed(
        &sandbox.spinel(&["install", "--frozen"]),
        16,
        "spinel install --frozen",
    );
    assert_eq!(
        err,
        "The gemspecs for path gems changed, but the lockfile can't be updated because frozen mode is set\n\n\
         Run `spinel install` elsewhere and add the updated Gemfile.lock to version control.\n"
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

    assert_eq!(
        ok(&sandbox.spinel(&["install"]), "spinel install"),
        "+ gadget 0.2.0\n- gadget 0.1.0\n"
    );
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        sandbox.bundlers("bumped.lock")
    );
}

#[test]
fn spinel_add_takes_a_git_source() {
    let mut sandbox = Sandbox::new("add");
    sandbox.commit("widget-1.0.0");
    sandbox.commit("widget-1.1.0");
    sandbox.write("Gemfile", &format!("source \"{}\"\n", sandbox.server.url));
    assert_eq!(
        ok(
            &sandbox.spinel(&["add", "widget", "--git", &sandbox.repo_url()]),
            "spinel add --git"
        ),
        "+ shared 2.0.0\n+ widget 1.1.0\n"
    );
    assert_eq!(sandbox.read("Gemfile"), sandbox.bundlers("added.Gemfile"));
    assert_eq!(sandbox.read("Gemfile.lock"), sandbox.bundlers("added.lock"));
}

#[test]
fn a_source_that_cannot_be_read_writes_nothing() {
    let mut sandbox = Sandbox::new("refused");
    sandbox.commit("widget-1.0.0");
    let gemfile = format!("source \"{}\"\n", sandbox.server.url);
    sandbox.write("Gemfile", &gemfile);

    // A repository that is not there: git's error, and nothing written.
    let missing = format!("file://{}", sandbox.root.join("no-such-repo").display());
    let err = failed(
        &sandbox.spinel(&["add", "widget", "--git", &missing]),
        1,
        "spinel add of a missing repository",
    );
    assert!(
        err.starts_with("Git error: command `git clone --bare"),
        "{err}"
    );
    assert_eq!(sandbox.read("Gemfile"), gemfile);
    assert_eq!(sandbox.try_read("Gemfile.lock"), None);

    // A branch git would read as an option is refused before git runs.
    let marker = sandbox.root.join("pwned");
    sandbox.write(
        "Gemfile",
        &format!(
            "source \"{}\"\ngem \"widget\", git: \"{}\", branch: \"--upload-pack=touch {}\"\n",
            sandbox.server.url,
            sandbox.repo_url(),
            marker.display()
        ),
    );
    let err = failed(&sandbox.spinel(&["install"]), 2, "an option for a branch");
    assert!(err.contains("starts with `-`"), "{err}");
    assert!(!marker.exists());
    assert_eq!(sandbox.try_read("Gemfile.lock"), None);

    // A gem the repository does not have.
    sandbox.write(
        "Gemfile",
        &format!(
            "source \"{}\"\ngem \"gizmo\", git: \"{}\"\n",
            sandbox.server.url,
            sandbox.repo_url()
        ),
    );
    let err = failed(
        &sandbox.spinel(&["install"]),
        1,
        "a gem the repository lacks",
    );
    assert_eq!(
        err,
        format!(
            "Could not find gem 'gizmo' in {} (at {}).\n\n\
             The source does not contain any versions of 'gizmo'\n",
            sandbox.repo_url(),
            &sandbox.revisions[0][..7]
        )
    );
    assert_eq!(sandbox.try_read("Gemfile.lock"), None);

    // A path that is not there.
    sandbox.write(
        "Gemfile",
        &format!(
            "source \"{}\"\ngem \"gadget\", path: \"../nowhere\"\n",
            sandbox.server.url
        ),
    );
    let err = failed(&sandbox.spinel(&["install"]), 1, "a missing path");
    assert_eq!(
        err,
        format!(
            "The path `{}` does not exist.\n",
            sandbox.root.join("nowhere").display()
        )
    );
}

#[test]
fn a_gemspec_gemfile_installs_its_gem_from_a_path_source() {
    let sandbox = Sandbox::new("gemspec");
    copy_dir(&fixtures().join("app"), &sandbox.project());
    sandbox.write(
        "Gemfile",
        &sandbox
            .read("Gemfile")
            .replace("@SOURCE@", &sandbox.server.url),
    );
    // The gem itself from `.`, and its development dependency with the
    // requirement its gemspec gives it.
    assert_eq!(
        ok(&sandbox.spinel(&["install"]), "spinel install"),
        "+ app 0.3.0\n+ shared 1.0.0\n"
    );
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        sandbox.bundlers("gemspec.lock")
    );
    let linked = sandbox.linked();
    assert_eq!(linked[0], ("app-0.3.0".to_owned(), sandbox.project()));
    assert_eq!(
        ok(
            &sandbox.spinel(&["run", "-e", "require \"app\"; require \"shared\"; p App"]),
            "spinel run"
        ),
        "App\n"
    );
    assert_eq!(
        ok(
            &sandbox.spinel(&["install", "--frozen"]),
            "spinel install --frozen"
        ),
        ""
    );
}
