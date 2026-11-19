//! The nio4r-spinel gem (#115, docs/package-manager.md, section 6, step 2),
//! end to end: `gems/nio4r-spinel/build.rb`, run by `spinel`, builds its
//! extension with cargo and packages `nio4r-spinel-2.7.5.1-<platform>.gem`;
//! a source on 127.0.0.1 serves it beside rubygems.org's own index lines and
//! `.gem`s for `fixtures/nio4r/app`, whose `Gemfile.lock` is the one Bundler
//! 4.0.20 wrote, locking upstream `nio4r 2.7.5` (a source gem, `ruby`) and
//! the RSpec that runs its suite. `spinel install --frozen`, with `nio4r` on
//! the list of substitutes (`SPINEL_SUBSTITUTES`, as Spinel's own list holds
//! no name until the project owns it on rubygems.org), installs the gem in
//! nio4r's place, upstream's never fetched and the lockfile left byte for
//! byte as Bundler wrote it.
//!
//! Then nio4r's own test suite, upstream's `spec/` at 2.7.5 vendored in
//! `fixtures/nio4r/app/spec` (`gems/nio4r-spinel/vendor.sh`), runs on it
//! under rspec-core 3.13.7, installed from the same source, and passes as it
//! does on CRuby with upstream nio4r; and `fixtures/nio4r/surface.rb`
//! prints the same as CRuby with upstream nio4r 2.7.5 printed
//! (`surface.txt`).
//!
//! And stock RubyGems and Bundler against the same source, when
//! `SPINEL_CRUBY_BUNDLER` says CRuby 4.0's are on `PATH` (CI's Linux leg):
//! they read `nio4r`'s builds and never ask for `nio4r-spinel`, which the
//! source lists, so a resolution leaves the lockfile as it was and an
//! install goes for upstream's own `.gem`.
//!
//! Nothing touches the network: cargo builds `--offline` from what the
//! workspace build fetched, every gem comes from the local source, and the
//! suite's sockets are on 127.0.0.1.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use md5::Md5;
use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const RUBYGEMS: &str = "https://rubygems.org";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture() -> PathBuf {
    fixtures().join("nio4r")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The platform nio4r-spinel's build for this host is published under.
fn host_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") if !cfg!(target_env = "musl") => "x86_64-linux-gnu",
        ("linux", "aarch64") if !cfg!(target_env = "musl") => "aarch64-linux-gnu",
        ("macos", "aarch64") => "arm64-darwin",
        ("macos", "x86_64") => "x86_64-darwin",
        (os, arch) => panic!("nio4r-spinel has no platform named for {arch}-{os}"),
    }
}

/// Scrub what would change where gems come from or go, or reach a proxy.
fn scrubbed(mut command: Command) -> Command {
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
        "SPEC_OPTS",
        "NIO4R_PURE",
        "SPINEL_SUBSTITUTES",
    ] {
        command.env_remove(var);
    }
    command
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

/// `build.rb`, run by this `spinel`, into `root`: the `.gem` it writes,
/// whose path it answers. Cargo's target directory is shared by the tests,
/// which cargo's lock on it takes in turn.
fn build_gem(root: &Path) -> PathBuf {
    let out = root.join("gem");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gems/nio4r-spinel/build.rb");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nio4r/target");
    let mut command = scrubbed(Command::new(SPINEL));
    command
        .current_dir(root)
        .env("SPINEL_HOME", root.join("home"))
        .arg("run")
        .arg(&script)
        .arg("--out")
        .arg(&out)
        .arg("--target-dir")
        .arg(target)
        .args(["--profile", "dev", "--offline", "--jobs", "2"]);
    let printed = ok(&command.output().expect("spinel runs"), "build.rb");
    let gem = PathBuf::from(printed.lines().last().expect("build.rb names the gem"));
    assert_eq!(
        gem,
        out.join(format!("nio4r-spinel-2.7.5.1-{}.gem", host_platform())),
        "{printed}"
    );
    gem
}

/// rubygems.org as `fixtures/nio4r/index` has it, plus nio4r-spinel:
/// `/versions` with its line added, `/info/nio4r-spinel` the one build
/// `build_gem` made, and `/gems/` that build and the RSpec `.gem`s in
/// `fixtures/install/gems`. Records every path asked for.
struct Source {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Source {
    fn start(gem: &Path) -> Source {
        let bytes = fs::read(gem).unwrap();
        let platform = host_platform();
        let info = format!(
            "---\n2.7.5.1-{platform} |checksum:{},ruby:>= 2.6\n",
            hex(&Sha256::digest(&bytes))
        );
        let versions = format!(
            "{}nio4r-spinel 2.7.5.1-{platform} {}\n",
            fs::read_to_string(fixture().join("index/versions")).unwrap(),
            hex(&Md5::digest(info.as_bytes()))
        );
        let gem_name = gem.file_name().unwrap().to_string_lossy().into_owned();
        let files = Arc::new((versions, info, gem_name, bytes));

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&asked);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, files) = (Arc::clone(&log), Arc::clone(&files));
                std::thread::spawn(move || {
                    let (versions, info, gem_name, gem) = &*files;
                    serve(stream, &log, |path| match path {
                        "versions" => Some(versions.clone().into_bytes()),
                        "info/nio4r-spinel" => Some(info.clone().into_bytes()),
                        _ => match path.strip_prefix("gems/") {
                            Some(name) if name == gem_name => Some(gem.clone()),
                            Some(name) => fs::read(fixtures().join("install/gems").join(name)).ok(),
                            None => fs::read(fixture().join("index").join(path)).ok(),
                        },
                    })
                });
            }
        });
        Source { url, asked }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

fn serve(stream: TcpStream, asked: &Mutex<Vec<String>>, file: impl Fn(&str) -> Option<Vec<u8>>) {
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
    let body = path
        .strip_prefix('/')
        .filter(|p| !p.split('/').any(|part| part == ".." || part.is_empty()))
        .and_then(file);
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

/// `from` copied into `to`, directories and all.
fn copy_tree(from: &Path, to: &Path, edit: &dyn Fn(String) -> String) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target, edit);
        } else {
            fs::write(&target, edit(fs::read_to_string(entry.path()).unwrap())).unwrap();
        }
    }
}

/// A scratch directory for one test: the gem's build, and a copy of
/// `fixtures/nio4r/app` with a Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("nio4r")
            .join(name);
        let _ = fs::remove_dir_all(root.join("project"));
        let _ = fs::remove_dir_all(root.join("home"));
        fs::create_dir_all(root.join("project")).unwrap();
        Sandbox { root }
    }

    /// The app, its `spec/` included, rubygems.org replaced by `source`.
    fn copy_app(&self, source: &Source) {
        copy_tree(&fixture().join("app"), &self.project(), &|text| {
            text.replace(RUBYGEMS, &source.url)
        });
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.project().join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    fn command(&self, program: &str) -> Command {
        let mut command = scrubbed(Command::new(program));
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.root.join("home"));
        command
    }

    fn spinel(&self) -> Command {
        self.command(SPINEL)
    }

    /// `spinel install --frozen` with nio4r on the list of substitutes.
    fn install(&self) -> Output {
        self.spinel()
            .args(["install", "--frozen"])
            .env("SPINEL_SUBSTITUTES", "nio4r")
            .output()
            .expect("spinel runs")
    }
}

/// Build the gem, serve it, and install the app with it in nio4r's place,
/// checking the install did what substitution says; the sandbox, the
/// source, and the lockfile as Bundler wrote it.
fn installed(name: &str) -> (Sandbox, Source, String) {
    let sandbox = Sandbox::new(name);
    let gem = build_gem(&sandbox.root);
    let source = Source::start(&gem);
    sandbox.copy_app(&source);
    let lock = sandbox.read("Gemfile.lock");
    let platform = host_platform();
    // Bundler's lockfile: upstream nio4r, a source gem, nothing of Spinel's.
    assert!(lock.contains("    nio4r (2.7.5)\n"), "{lock}");
    assert!(!lock.contains("spinel"), "{lock}");

    let out = sandbox.install();
    // The output names the locked gem, as for any other.
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        "+ diff-lcs 1.6.2\n+ nio4r 2.7.5\n+ rspec-core 3.13.7\n+ rspec-expectations 3.13.6\n\
         + rspec-mocks 3.13.9\n+ rspec-support 3.13.8\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock, "the lockfile changed");
    assert_eq!(
        sandbox.read(".spinel/substitutions.json"),
        format!(
            r#"{{
  "substitutions": [
    {{
      "kind": "native",
      "name": "nio4r",
      "platform": "ruby",
      "substitute": "nio4r-spinel",
      "substitute_platform": "{platform}",
      "substitute_version": "2.7.5.1",
      "version": "2.7.5"
    }}
  ]
}}
"#
        )
    );
    // Upstream's gem was never fetched; the substitute was, once.
    let gems: Vec<String> = source
        .asked()
        .into_iter()
        .filter(|p| p.starts_with("/gems/nio4r"))
        .collect();
    assert_eq!(
        gems,
        [format!("/gems/nio4r-spinel-2.7.5.1-{platform}.gem")],
        "{:?}",
        source.asked()
    );
    (sandbox, source, lock)
}

/// The install, then nio4r's own suite on the substitute, all of it: every
/// example passes, as on CRuby with upstream nio4r 2.7.5, but those the
/// suite itself marks pending, and the API answers as CRuby's does.
#[test]
fn nio4r_spinel_installs_in_nio4rs_place_and_passes_nio4rs_own_suite() {
    let (sandbox, _source, lock) = installed("suite");

    // Bundler.setup activates the nio4r the lock names, and `require`
    // loads nio4r-spinel's files and its extension.
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "bundler/setup"
require "nio"
puts Gem.loaded_specs["nio4r"].full_name
puts $LOADED_FEATURES.grep(/nio4r_spinel\.spinel\z/).map { |f| File.basename(f) }
p NIO::VERSION, NIO.engine, NIO::Selector.new.backend.class"#,
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(
        ok(&out, "spinel run -e"),
        "nio4r-2.7.5\nnio4r_spinel.spinel\n\"2.7.5\"\n\"libev\"\nSymbol\n"
    );

    // nio4r's suite, as its Rakefile's `rspec` runs it, every file under
    // `spec/`, under the RSpec the lock names.
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "bundler/setup"
load Gem.bin_path("rspec-core", "rspec")"#,
            "--",
            "--no-color",
            "--format",
            "documentation",
            "spec",
        ])
        .output()
        .unwrap();
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    let report = format!(
        "{printed}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "nio4r's suite failed\n{report}");
    let summary = printed
        .lines()
        .find(|line| line.contains(" examples, "))
        .unwrap_or_else(|| panic!("no summary\n{report}"));
    let (counted, pending) = match summary.split_once(", 0 failures") {
        Some((counted, rest)) => (counted, rest.trim_start_matches(", ")),
        None => panic!("{summary}\n{report}"),
    };
    assert_eq!(counted, "112 examples", "{report}");
    // Pending are only the examples the suite itself marks so, which CRuby
    // with upstream nio4r 2.7.5 reports pending too: UDP has no socket
    // blocked on writing, and the TCP one is pending when the kernel drained
    // the socket again before the check, which depends on how fast it was
    // filled (CRuby's loop slowed by 0.1 ms a write fills it for good).
    let marked = [
        "UDPSocket behaves like an NIO selectable does not select unwritable objects",
        "TCPSocket behaves like an NIO selectable does not select unwritable objects",
    ];
    let listed: Vec<&str> = printed
        .split_once("Pending: (Failures listed here are expected")
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .lines()
        // Each entry's first line: `  1) <full description>`.
        .filter_map(|line| line.trim_start().split_once(") "))
        .filter(|(number, _)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .map(|(_, name)| name)
        .collect();
    let expected_count = if pending.is_empty() {
        0
    } else {
        pending
            .trim_end_matches(" pending")
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("{summary}"))
    };
    assert_eq!(listed.len(), expected_count, "{report}");
    for name in &listed {
        assert!(marked.contains(name), "{name} is pending\n{report}");
    }
    assert!(
        listed.contains(&marked[0]),
        "the UDP example is not pending\n{report}"
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

    // The API as CRuby's nio4r 2.7.5 answers it.
    sandbox.write(
        "surface.rb",
        &fs::read_to_string(fixture().join("surface.rb")).unwrap(),
    );
    let out = sandbox
        .spinel()
        .args(["run", "surface.rb"])
        .output()
        .unwrap();
    let printed = ok(&out, "spinel run surface.rb");
    let expected = fs::read_to_string(fixture().join("surface.txt")).unwrap();
    for (line, (got, want)) in printed.lines().zip(expected.lines()).enumerate() {
        assert_eq!(
            got,
            want,
            "surface.rb line {} differs from CRuby's",
            line + 1
        );
    }
    assert_eq!(
        printed.lines().count(),
        expected.lines().count(),
        "{printed}"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// A selector's descriptors are closed with it, or by its finalizer when it
/// is collected unclosed, and a wait in `select` lets the other threads of
/// its Ractor run. A regular file, which epoll refuses, is ready on every
/// run, as libev's `epoll_eperms` make it, with the answers CRuby 4.0.9 and
/// upstream nio4r 2.7.5 give on Linux. A registration left in epoll by a
/// descriptor closed before it was deregistered, its description still open
/// in a `dup`, does not keep `select` from waiting.
#[test]
fn selectors_free_their_descriptors_and_wait_outside_the_lock() {
    let (sandbox, _source, _lock) = installed("descriptors");
    sandbox.write("plain.txt", "a regular file\n");
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "nio"
open_fds = -> { Dir.children("/dev/fd").size }
before = open_fds.()
10.times { NIO::Selector.new.close }
p open_fds.() - before
10.times { NIO::Selector.new }
GC.start
p open_fds.() - before <= 3
s = NIO::Selector.new
ticks = 0
t = Thread.new { 20.times { ticks += 1; sleep 0.01 } }
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
p s.select(0.4)
p (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round(1), ticks
t.join
s = NIO::Selector.new
f = File.open("plain.txt")
m = s.register(f, :r)
p s.select(0).map(&:readiness), s.select(1).size, s.select { |x| }
m.interests = :rw
p s.select(0).map(&:readiness)
m.interests = nil
p s.select(0)
s.deregister(f)
p s.select(0), s.backend
s = NIO::Selector.new
r, w = IO.pipe
kept = r.dup
s.register(r, :r)
w.write("x")
r.close
s.deregister(r)
cpu = -> { Process.times.then { |t| t.utime + t.stime } }
spent = cpu.()
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
p s.select(0.3)
p (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round(1), cpu.() - spent < 0.15
r2, w2 = IO.pipe
s.register(r2, :r)
w2.write("y")
p s.select(1).map { |ready| ready.io.equal?(r2) }
kept.close"#,
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let backend = if cfg!(any(target_os = "linux", target_os = "android")) {
        ":epoll"
    } else {
        ":select"
    };
    assert_eq!(
        ok(&out, "spinel run -e"),
        format!(
            "0\ntrue\nnil\n0.4\n20\n[:r]\n1\n1\n[:rw]\nnil\nnil\n{backend}\nnil\n0.3\ntrue\n[true]\n"
        )
    );
}

/// Stock RubyGems and Bundler against the source that lists nio4r-spinel:
/// CRuby 4.0's `gem` and `bundle`, which `SPINEL_CRUBY_BUNDLER` says are on
/// `PATH`, resolve `nio4r` by name and never ask for `nio4r-spinel`. `bundle
/// lock --update` resolves the project afresh to the same lockfile; `bundle
/// install` and `gem install nio4r` go for upstream's own gem, which the
/// source does not carry, so each stops there.
#[test]
fn stock_rubygems_and_bundler_never_pick_the_spinel_gem() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!(
            "skipped: set SPINEL_CRUBY_BUNDLER, with CRuby 4.0's gem and bundle on PATH, to run \
             stock RubyGems and Bundler against the source with nio4r-spinel"
        );
        return;
    }
    let (sandbox, source, lock) = installed("stock");

    // What each stock command asks the source, from when it starts.
    let asked_by = |command: &mut Command| {
        let from = source.asked().len();
        let out = command.output().expect("CRuby's tools run");
        (out, source.asked()[from..].to_vec())
    };
    let bundle = |args: &[&str]| {
        let mut command = sandbox.command("bundle");
        command
            .args(args)
            .env("BUNDLE_GEMFILE", sandbox.project().join("Gemfile"))
            .env("BUNDLE_APP_CONFIG", sandbox.root.join("bundle-config"))
            .env("BUNDLE_PATH", sandbox.root.join("bundle-path"))
            .env("BUNDLE_USER_HOME", sandbox.root.join("bundle-user-home"));
        command
    };

    let (out, asked) = asked_by(&mut bundle(&["lock", "--update"]));
    ok(&out, "bundle lock --update");
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        lock,
        "bundle lock --update changed the lockfile"
    );
    assert!(asked.iter().any(|p| p == "/info/nio4r"), "{asked:?}");
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");

    let (out, asked) = asked_by(&mut bundle(&["install"]));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("nio4r-2.7.5"), "{said}");
    assert!(
        asked.iter().any(|p| p == "/gems/nio4r-2.7.5.gem"),
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        lock,
        "bundle install changed the lockfile"
    );

    let mut gem_install = sandbox.command("gem");
    gem_install
        .args([
            "install",
            "nio4r",
            "--version",
            "2.7.5",
            "--clear-sources",
            "--no-document",
        ])
        .arg("--source")
        .arg(format!("{}/", source.url))
        .arg("--install-dir")
        .arg(sandbox.root.join("gem-install"))
        .env("HOME", sandbox.root.join("gem-home"));
    let (out, asked) = asked_by(&mut gem_install);
    assert!(!out.status.success(), "{out:?}");
    assert!(asked.iter().any(|p| p == "/info/nio4r"), "{asked:?}");
    assert!(
        asked
            .iter()
            .any(|p| p.starts_with("/quick/Marshal.4.8/nio4r-2.7.5")),
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");
}
