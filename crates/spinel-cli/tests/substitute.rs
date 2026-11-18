//! Spinel-native substitution at install time (#112, docs/package-manager.md,
//! section 6, step 2): `fixtures/substitute/app` locks `widget 1.0.0` as
//! Bundler 4.0.20 locked it, against a source that also has `widget-spinel`,
//! Spinel's substitute for it. With `widget` on Spinel's list of substitutes
//! (`SPINEL_SUBSTITUTES`, as the built-in list owns no name yet),
//! `spinel install` fetches `widget-spinel 1.0.0.2`, never `widget` itself,
//! links it into the GEM_HOME as `widget-1.0.0`, records it in
//! `.spinel/substitutions.json`, and leaves the lockfile byte for byte as
//! Bundler wrote it, `ruby` platform entry and all. A gem off the list is
//! installed as itself; a substitute that needs a gem upstream does not
//! fails the install; a Gemfile that names `widget-spinel` gets it as an
//! ordinary gem, with a warning. And stock Bundler on the same Gemfile and
//! lockfile (CRuby's, when `SPINEL_CRUBY_BUNDLER` says it is on `PATH`)
//! installs `widget` itself, never asks for `widget-spinel`, and leaves the
//! lockfile as it was.
//!
//! The source is a server on 127.0.0.1 that serves `fixtures/substitute/`'s
//! index and `.gem`s (`build.rb` makes them) and records what it is asked,
//! so nothing touches the network.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const RUBYGEMS: &str = "https://rubygems.org";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/substitute")
}

/// The fixture's source: `/versions` and `/info/<gem>` from `index/`,
/// `/gems/<file>` from `gems/`, every path asked for recorded.
struct Server {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&asked);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        Server { url, asked }
    }

    /// Every path asked for so far.
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

fn serve(stream: TcpStream, asked: &Mutex<Vec<String>>) {
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
    asked.lock().unwrap().push(path.clone());
    let root = fixture();
    let file = path
        .strip_prefix('/')
        .filter(|p| !p.split('/').any(|part| part == ".." || part.is_empty()))
        .and_then(|p| match p.strip_prefix("gems/") {
            Some(gem) => fs::read(root.join("gems").join(gem)).ok(),
            None => fs::read(root.join("index").join(p)).ok(),
        });
    let (status, body) = match file {
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

/// A project and a Spinel home of its own, under the test's scratch
/// directory.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// `fixtures/substitute/<project>` copied in as `name`, rubygems.org
    /// replaced by `server`.
    fn new(project: &str, name: &str, server: &Server) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("substitute")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        let sandbox = Sandbox { root };
        for file in ["Gemfile", "Gemfile.lock"] {
            if let Ok(text) = fs::read_to_string(fixture().join(project).join(file)) {
                fs::write(
                    sandbox.project().join(file),
                    text.replace(RUBYGEMS, &server.url),
                )
                .unwrap();
            }
        }
        sandbox
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn gem_home(&self) -> PathBuf {
        self.project().join(".spinel/gem_home")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    /// A command run in the project, with nothing of the shell's that
    /// would change where gems come from or go.
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.root.join("home"));
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
            "SPINEL_SUBSTITUTES",
        ] {
            command.env_remove(var);
        }
        command
    }

    /// `spinel install`, with `substitutes` as `SPINEL_SUBSTITUTES` when
    /// given.
    fn install(&self, substitutes: Option<&str>) -> Output {
        let mut command = self.command(SPINEL);
        command.arg("install");
        if let Some(names) = substitutes {
            command.env("SPINEL_SUBSTITUTES", names);
        }
        command.output().expect("spinel runs")
    }

    /// `require "bundler/setup"`, then `widget`: which one loaded, and the
    /// spec Bundler activated for it.
    fn load_widget(&self) -> String {
        let out = self
            .command(SPINEL)
            .args([
                "run",
                "-e",
                r#"require "bundler/setup"
require "widget"
puts Widget::IMPL, Gizmo::IMPL
puts Gem.loaded_specs.values.reject { |s| s.name == "bundler" }.map(&:full_name).sort.join(",")"#,
            ])
            .output()
            .unwrap();
        ok(&out, "spinel run -e")
    }

    /// Where `gems/<full name>` in the GEM_HOME leads.
    fn link_target(&self, full_name: &str) -> PathBuf {
        fs::read_link(self.gem_home().join("gems").join(full_name)).unwrap()
    }

    /// The names under one of the GEM_HOME's directories.
    fn listed(&self, dir: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.gem_home().join(dir))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed ({:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

/// The `.gem` SHA-256 `info/<gem>` lists for `version`.
fn checksum(gem: &str, version: &str) -> String {
    let info = fs::read_to_string(fixture().join("index/info").join(gem)).unwrap();
    info.lines()
        .find_map(|line| {
            let rest = line.strip_prefix(version)?.strip_prefix(' ')?;
            Some(rest.split("checksum:").nth(1)?.to_owned())
        })
        .unwrap_or_else(|| panic!("{gem} {version} in the index"))
}

const WIDGET_SUBSTITUTED: &str = r#"{
  "substitutions": [
    {
      "kind": "native",
      "name": "widget",
      "platform": "ruby",
      "substitute": "widget-spinel",
      "substitute_platform": "ruby",
      "substitute_version": "1.0.0.2",
      "version": "1.0.0"
    }
  ]
}
"#;

const NONE_SUBSTITUTED: &str = "{\n  \"substitutions\": []\n}\n";

#[test]
fn a_listed_gem_is_installed_as_its_substitute_and_the_lock_is_untouched() {
    let server = Server::start();
    let sandbox = Sandbox::new("app", "listed", &server);
    let lock = sandbox.read("Gemfile.lock");
    // Bundler's lockfile: widget's `ruby` build, nothing of Spinel's.
    assert!(lock.contains("    widget (1.0.0)\n"), "{lock}");
    assert!(lock.contains("PLATFORMS\n  ruby\n"), "{lock}");
    assert!(!lock.contains("spinel"), "{lock}");

    let out = sandbox.install(Some("widget"));
    assert_eq!(
        ok(&out, "spinel install"),
        "+ gizmo 1.0.0\n+ widget 1.0.0\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock, "the lockfile changed");
    assert_eq!(
        sandbox.read(".spinel/substitutions.json"),
        WIDGET_SUBSTITUTED
    );

    // The substitute, under the locked identity: `widget-1.0.0` leads to
    // widget-spinel 1.0.0.2's files, and nothing is named widget-spinel.
    let sha = checksum("widget-spinel", "1.0.0.2");
    assert!(
        sandbox
            .link_target("widget-1.0.0")
            .ends_with(format!("store/gems/widget-spinel-1.0.0.2-ruby-{sha}/gem")),
        "{}",
        sandbox.link_target("widget-1.0.0").display()
    );
    assert_eq!(sandbox.listed("gems"), ["gizmo-1.0.0", "widget-1.0.0"]);
    assert_eq!(
        sandbox.listed("specifications"),
        ["gizmo-1.0.0.gemspec", "widget-1.0.0.gemspec"]
    );
    let gemspec = sandbox.read(".spinel/gem_home/specifications/widget-1.0.0.gemspec");
    assert!(
        gemspec.contains("# stub: widget 1.0.0 ruby lib\n"),
        "{gemspec}"
    );
    assert!(
        gemspec.contains("s.add_runtime_dependency(%q<gizmo>.freeze, [\"~> 1.0\".freeze])"),
        "{gemspec}"
    );

    // widget itself was never fetched; of widget-spinel, only the build
    // installed.
    let gems: Vec<String> = server
        .asked()
        .into_iter()
        .filter(|p| p.starts_with("/gems/"))
        .collect();
    assert_eq!(gems.len(), 2, "{gems:?}");
    assert!(
        gems.contains(&"/gems/gizmo-1.0.0.gem".to_owned()),
        "{gems:?}"
    );
    assert!(
        gems.contains(&"/gems/widget-spinel-1.0.0.2.gem".to_owned()),
        "{gems:?}"
    );

    // Bundler.setup activates the `widget 1.0.0` the lock names, and its
    // files are the substitute's.
    assert_eq!(
        sandbox.load_widget(),
        "widget-spinel 1.0.0.2\ngizmo\ngizmo-1.0.0,widget-1.0.0\n"
    );

    // Again: nothing changes, and the lockfile is still Bundler's.
    let out = sandbox.install(Some("widget"));
    assert_eq!(ok(&out, "spinel install"), "Nothing to change\n");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

    // Off the list, widget is installed as itself and nothing is
    // substituted: a gem named `-spinel` is never trusted on its name.
    let out = sandbox.install(None);
    assert_eq!(ok(&out, "spinel install"), "Nothing to change\n");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert_eq!(sandbox.read(".spinel/substitutions.json"), NONE_SUBSTITUTED);
    assert!(
        sandbox.link_target("widget-1.0.0").ends_with(format!(
            "store/gems/widget-1.0.0-ruby-{}/gem",
            checksum("widget", "1.0.0")
        )),
        "{}",
        sandbox.link_target("widget-1.0.0").display()
    );
    assert_eq!(
        sandbox.load_widget(),
        "widget 1.0.0\ngizmo\ngizmo-1.0.0,widget-1.0.0\n"
    );
}

#[test]
fn a_fresh_resolution_locks_what_bundler_locks() {
    let server = Server::start();
    let sandbox = Sandbox::new("app", "fresh", &server);
    let bundlers = sandbox.read("Gemfile.lock");
    fs::remove_file(sandbox.project().join("Gemfile.lock")).unwrap();

    let out = sandbox.install(Some("widget"));
    assert_eq!(
        ok(&out, "spinel install"),
        "+ gizmo 1.0.0\n+ widget 1.0.0\n"
    );
    // The resolver never sees the substitute: the lockfile is the one
    // Bundler wrote for the same Gemfile, byte for byte.
    assert_eq!(sandbox.read("Gemfile.lock"), bundlers);
    assert_eq!(
        sandbox.read(".spinel/substitutions.json"),
        WIDGET_SUBSTITUTED
    );
}

#[test]
fn a_substitute_that_needs_a_gem_upstream_does_not_fails_the_install() {
    let server = Server::start();
    let sandbox = Sandbox::new("extra-dep", "extra-dep", &server);
    let out = sandbox.install(Some("gadget"));
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "spinel: install: gadget-spinel-2.0.0.1 cannot stand in for gadget 2.0.0: it depends \
         on gizmo (>= 0), which gadget does not, so Gemfile.lock locks nothing for it\n"
    );
    // Failed before anything was fetched or written.
    assert!(
        !server.asked().iter().any(|p| p.starts_with("/gems/")),
        "{:?}",
        server.asked()
    );
    assert!(!sandbox.project().join("Gemfile.lock").exists());
    assert!(!sandbox.gem_home().join("gems").exists());

    // Off the list, gadget installs as itself.
    let out = sandbox.install(None);
    assert_eq!(ok(&out, "spinel install"), "+ gadget 2.0.0\n");
}

#[test]
fn a_gemfile_naming_a_substitute_gets_it_as_an_ordinary_gem() {
    let server = Server::start();
    let sandbox = Sandbox::new("direct", "direct", &server);
    let out = sandbox.install(Some("widget"));
    assert_eq!(ok(&out, "spinel install"), "+ widget-spinel 1.0.1.1\n");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "spinel: install: warning: the Gemfile names widget-spinel, Spinel's substitute for \
         widget: it is installed as an ordinary gem, not in widget's place; `gem \"widget\"` \
         gets it in widget's place\n"
    );
    let lock = sandbox.read("Gemfile.lock");
    assert!(lock.contains("    widget-spinel (1.0.1.1)\n"), "{lock}");
    assert_eq!(sandbox.listed("gems"), ["widget-spinel-1.0.1.1"]);
    assert_eq!(sandbox.read(".spinel/substitutions.json"), NONE_SUBSTITUTED);

    // A `-spinel` gem whose upstream is not listed is anybody's: no
    // warning.
    let out = sandbox.install(None);
    assert_eq!(ok(&out, "spinel install"), "Nothing to change\n");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// Stock Bundler on the Gemfile and lockfile `spinel install` substituted
/// for: CRuby's `bundle`, which `SPINEL_CRUBY_BUNDLER` says is on `PATH`
/// (CI sets up Ruby 4.0 for it), installs `widget` itself into a path of
/// its own, never asks the source for `widget-spinel`, and leaves the
/// lockfile byte for byte as it was.
#[test]
fn stock_bundler_ignores_the_substitute_and_keeps_the_lock() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!(
            "skipped: set SPINEL_CRUBY_BUNDLER, with CRuby 4.0's bundle on PATH, to run stock \
             Bundler against the substituted project"
        );
        return;
    }
    let server = Server::start();
    let sandbox = Sandbox::new("app", "stock-bundler", &server);
    let lock = sandbox.read("Gemfile.lock");
    ok(&sandbox.install(Some("widget")), "spinel install");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    let spinel_asked = server.asked().len();

    // Bundler's own settings, path and caches, away from Spinel's
    // `.bundle/config`, so it installs where it alone looks.
    let bundle = |args: &[&str]| {
        let out = sandbox
            .command("bundle")
            .args(args)
            .env("BUNDLE_GEMFILE", sandbox.project().join("Gemfile"))
            .env("BUNDLE_APP_CONFIG", sandbox.root.join("bundle-config"))
            .env("BUNDLE_PATH", sandbox.root.join("bundle-path"))
            .env("BUNDLE_USER_HOME", sandbox.root.join("bundle-user-home"))
            .output()
            .expect("CRuby's bundle runs");
        ok(&out, &format!("bundle {}", args.join(" ")))
    };
    bundle(&["install"]);
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        lock,
        "bundle install changed the lockfile"
    );
    let bundler_asked: Vec<String> = server.asked()[spinel_asked..].to_vec();
    assert!(
        bundler_asked.iter().any(|p| p == "/info/widget"),
        "{bundler_asked:?}"
    );
    assert!(
        !bundler_asked.iter().any(|p| p.contains("spinel")),
        "{bundler_asked:?}"
    );
    assert_eq!(
        bundle(&[
            "exec",
            "ruby",
            "-e",
            "require 'widget'; puts Widget::IMPL; puts Gem.loaded_specs.keys.grep(/widget|gizmo/).sort.join(',')",
        ]),
        "widget 1.0.0\ngizmo,widget\n"
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}
