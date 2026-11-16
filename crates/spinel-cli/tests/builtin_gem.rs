//! Built-in gem substitution (#83, docs/package-manager.md, section 6, step
//! 1): `fixtures/builtin-gem` locks `json` and `stringio`, two default gems
//! whose upstream `.gem`s carry an `ext/` and which Spinel ships as built-ins.
//! `spinel install` uses the built-ins: it fetches neither gem, so no `ext/`
//! is built and nothing compiled is written, links the locked identity into
//! the GEM_HOME, records the substitution in `.spinel/substitutions.json`, and
//! leaves the lockfile as Bundler wrote it. `require "bundler/setup"` then
//! activates the locked specs and `require` loads Spinel's built-ins. And the
//! skew between the locked version and the built-in's: a different patch
//! version is silent, a different minor warns, a different major fails before
//! anything is written.
//!
//! The fixture's rubygems.org is a server on 127.0.0.1 that has nothing and
//! counts what it is asked, so nothing touches the network and a gem fetched
//! would fail the install. (`bigdecimal`, which #83 also names, is not a
//! built-in yet, #109.)

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const RUBYGEMS: &str = "https://rubygems.org";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/builtin-gem")
}

/// A rubygems.org with no files: every request is a 404, and counted.
struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                count.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || not_found(stream));
            }
        });
        Server { url, requests }
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn not_found(stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
    }
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
    );
}

/// A project and a Spinel home of its own, under the test's scratch
/// directory.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// The fixture copied in, rubygems.org replaced by `server` and each
    /// `(from, to)` replaced in the lockfile.
    fn new(name: &str, server: &Server, lock_edits: &[(&str, &str)]) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("builtin_gem")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        let sandbox = Sandbox { root };
        for file in ["Gemfile", "Gemfile.lock"] {
            let mut text = fs::read_to_string(fixture().join(file))
                .unwrap()
                .replace(RUBYGEMS, &server.url);
            if file == "Gemfile.lock" {
                for (from, to) in lock_edits {
                    assert!(text.contains(from), "{from}");
                    text = text.replace(from, to);
                }
            }
            fs::write(sandbox.project().join(file), text).unwrap();
        }
        sandbox
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    fn spinel(&self) -> Command {
        let mut command = Command::new(SPINEL);
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.home());
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
        ] {
            command.env_remove(var);
        }
        command
    }

    fn install(&self) -> Output {
        self.spinel()
            .args(["install", "--frozen"])
            .output()
            .expect("spinel runs")
    }

    /// `require "bundler/setup"`, then the built-ins: their versions, the
    /// specs Bundler activated, and where `json.rb` was loaded from.
    fn load(&self) -> String {
        let out = self
            .spinel()
            .args([
                "run",
                "-e",
                r#"require "bundler/setup"
require "json"
require "stringio"
puts JSON::VERSION, StringIO::VERSION
puts Gem.loaded_specs.values.map(&:full_name).sort.join(",")
puts $LOADED_FEATURES.find { |f| f.end_with?("/json.rb") }
puts JSON.generate({"a" => [1, 2.5, nil]}), JSON.parse('{"b":[true]}').inspect
puts StringIO.new("x\ny").readlines.inspect"#,
            ])
            .output()
            .unwrap();
        ok(&out, "spinel run -e")
    }

    /// Every file and directory under `dir`, relative to it.
    fn tree(&self, dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(at) = stack.pop() {
            for entry in fs::read_dir(&at).into_iter().flatten().flatten() {
                let path = entry.path();
                out.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
                if entry.file_type().unwrap().is_dir() {
                    stack.push(path);
                }
            }
        }
        out.sort();
        out
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
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// What `load` prints when `json_spec` is the json the lock pins.
fn loaded(sandbox: &Sandbox, json_spec: &str) {
    let out = sandbox.load();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "2.18.0", "{out}");
    assert_eq!(lines[1], "3.2.0", "{out}");
    assert_eq!(
        lines[2],
        format!("bundler-4.0.16,{json_spec},stringio-3.2.0"),
        "{out}"
    );
    // The built-in's Ruby half: the standard library Spinel extracted into
    // its home, not anything under the GEM_HOME.
    assert!(
        Path::new(lines[3]).starts_with(sandbox.home().join("stdlib")),
        "{out}"
    );
    assert_eq!(lines[4], r#"{"a":[1,2.5,null]}"#, "{out}");
    assert_eq!(lines[5], r#"{"b" => [true]}"#, "{out}");
    assert_eq!(lines[6], r#"["x\n", "y"]"#, "{out}");
}

#[test]
fn the_lock_installs_the_built_ins_and_skips_ext() {
    let server = Server::start();
    let sandbox = Sandbox::new("plain", &server, &[]);
    let lock = sandbox.read("Gemfile.lock");

    let out = sandbox.install();
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        "+ json 2.18.0\n+ stringio 3.2.0\n"
    );
    assert_eq!(stderr(&out), "");
    // Nothing asked of rubygems.org, so neither `.gem` was fetched, and the
    // store has nothing: no `ext/` was unpacked, let alone built.
    assert_eq!(server.requests(), 0);
    assert_eq!(sandbox.tree(&sandbox.home().join("store/gems")), [""; 0]);
    // The lockfile is Bundler's, byte for byte: it still pins the gems.
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

    let gem_home = sandbox.project().join(".spinel/gem_home");
    assert_eq!(
        sandbox.tree(&gem_home.join("gems")),
        ["json-2.18.0", "stringio-3.2.0"]
    );
    // Nothing compiled anywhere, and nothing for RubyGems to build.
    assert_eq!(sandbox.tree(&gem_home.join("extensions")), [""; 0]);
    for gem in ["json-2.18.0", "stringio-3.2.0"] {
        let dir = fs::canonicalize(gem_home.join("gems").join(gem)).unwrap();
        assert!(dir.starts_with(fs::canonicalize(sandbox.home()).unwrap().join("builtin")));
        assert_eq!(sandbox.tree(&dir), ["lib"], "{gem}");
        let spec =
            fs::read_to_string(gem_home.join(format!("specifications/{gem}.gemspec"))).unwrap();
        assert!(!spec.contains("extensions"), "{spec}");
    }
    let spec = fs::read_to_string(gem_home.join("specifications/json-2.18.0.gemspec")).unwrap();
    assert!(spec.contains("# stub: json 2.18.0 ruby lib\n"), "{spec}");
    assert!(spec.contains("  s.version = \"2.18.0\".freeze\n"), "{spec}");

    assert_eq!(
        sandbox.read(".spinel/substitutions.json"),
        r#"{
  "substitutions": [
    {
      "kind": "builtin",
      "name": "json",
      "platform": "ruby",
      "substitute_version": "2.18.0",
      "version": "2.18.0"
    },
    {
      "kind": "builtin",
      "name": "stringio",
      "platform": "ruby",
      "substitute_version": "3.2.0",
      "version": "3.2.0"
    }
  ]
}
"#
    );

    loaded(&sandbox, "json-2.18.0");

    // A second install has nothing to do and says so.
    let out = sandbox.install();
    assert_eq!(ok(&out, "a second spinel install"), "");
    assert_eq!(stderr(&out), "");
    assert_eq!(server.requests(), 0);
}

#[test]
fn a_patch_version_apart_is_silent() {
    let server = Server::start();
    let sandbox = Sandbox::new("patch", &server, &[("json (2.18.0)", "json (2.18.5)")]);
    let out = sandbox.install();
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        "+ json 2.18.5\n+ stringio 3.2.0\n"
    );
    assert_eq!(stderr(&out), "");
    assert_eq!(server.requests(), 0);
    loaded(&sandbox, "json-2.18.5");
}

#[test]
fn a_minor_version_apart_warns() {
    let server = Server::start();
    let sandbox = Sandbox::new("minor", &server, &[("json (2.18.0)", "json (2.19.1)")]);
    let out = sandbox.install();
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        "+ json 2.19.1\n+ stringio 3.2.0\n"
    );
    assert_eq!(
        stderr(&out),
        "spinel: install: warning: Gemfile.lock pins json 2.19.1, and Spinel's built-in json \
         is 2.18.0: using the built-in\n"
    );
    assert_eq!(server.requests(), 0);
    loaded(&sandbox, "json-2.19.1");
}

#[test]
fn a_major_version_apart_fails_before_anything_is_written() {
    let server = Server::start();
    let sandbox = Sandbox::new(
        "major",
        &server,
        &[
            ("json (2.18.0)", "json (3.0.0)"),
            ("stringio (3.2.0)", "stringio (4.1.0)"),
        ],
    );
    let lock = sandbox.read("Gemfile.lock");
    let out = sandbox.install();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    // Each gem a major version apart is named, not only the first.
    assert_eq!(
        stderr(&out),
        "spinel: install: Gemfile.lock pins json 3.0.0, and Spinel's built-in json is 2.18.0: \
         a different major version, whose Ruby half may expect other primitives than Spinel's; \
         `gem \"json\", \"~> 2.18.0\"` in the Gemfile keeps it to the built-in's version\n\
         spinel: install: Gemfile.lock pins stringio 4.1.0, and Spinel's built-in stringio is \
         3.2.0: a different major version, whose Ruby half may expect other primitives than \
         Spinel's; `gem \"stringio\", \"~> 3.2.0\"` in the Gemfile keeps it to the built-in's version\n"
    );
    assert_eq!(server.requests(), 0);
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert!(!sandbox.project().join(".spinel").exists());
    assert!(!sandbox.project().join(".bundle").exists());
    assert!(!sandbox.home().join("builtin").exists());
}
