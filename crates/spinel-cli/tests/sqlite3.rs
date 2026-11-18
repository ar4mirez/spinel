//! The sqlite3-spinel gem (#113, docs/package-manager.md, section 6, step
//! 2), end to end: `gems/sqlite3-spinel/build.rb`, run by `spinel`, builds
//! its extension with cargo and packages
//! `sqlite3-spinel-2.10.2.1-<platform>.gem`; a source on 127.0.0.1 serves it
//! beside rubygems.org's own index lines and `.gem`s for
//! `fixtures/sqlite3/app`, a Sinatra app over SQLite whose `Gemfile.lock` is
//! the one Bundler 4.0.20 wrote, locking upstream `sqlite3 2.10.2` for this
//! platform. `spinel install --frozen`, with `sqlite3` on the list of
//! substitutes (`SPINEL_SUBSTITUTES`, as Spinel's own list holds no name
//! until the project owns it on rubygems.org), installs the gem in
//! sqlite3's place, upstream's never fetched and the lockfile left byte for
//! byte as Bundler wrote it; the app then serves a request that writes to
//! the database and reads it back. `fixtures/sqlite3/surface.rb` prints the
//! same as CRuby with upstream sqlite3 2.10.2 printed (`surface.txt`).
//!
//! And stock RubyGems and Bundler against the same source, when
//! `SPINEL_CRUBY_BUNDLER` says CRuby 4.0's are on `PATH` (CI's Linux leg):
//! they read `sqlite3`'s builds and never ask for `sqlite3-spinel`, which
//! the source lists, so a resolution leaves the lockfile as it was and an
//! install goes for upstream's own build.
//!
//! Nothing touches the network: cargo builds `--offline` from what the
//! workspace build fetched, and every gem comes from the local source.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use md5::Md5;
use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const RUBYGEMS: &str = "https://rubygems.org";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture() -> PathBuf {
    fixtures().join("sqlite3")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The platform sqlite3-spinel's build for this host is published under,
/// and the build of upstream sqlite3 the fixture's lockfile locks for it.
fn host_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") if !cfg!(target_env = "musl") => "x86_64-linux-gnu",
        ("linux", "aarch64") if !cfg!(target_env = "musl") => "aarch64-linux-gnu",
        ("macos", "aarch64") => "arm64-darwin",
        ("macos", "x86_64") => "x86_64-darwin",
        (os, arch) => panic!("fixtures/sqlite3/app/Gemfile.lock locks no sqlite3 for {arch}-{os}"),
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

/// `build.rb`, run by this `spinel`, into `root`: cargo's target directory
/// and the `.gem` it writes, whose path it answers.
fn build_gem(root: &Path) -> PathBuf {
    let out = root.join("gem");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gems/sqlite3-spinel/build.rb");
    let mut command = scrubbed(Command::new(SPINEL));
    command
        .current_dir(root)
        .env("SPINEL_HOME", root.join("home"))
        .arg("run")
        .arg(&script)
        .arg("--out")
        .arg(&out)
        .arg("--target-dir")
        .arg(root.join("target"))
        .args(["--profile", "dev", "--offline", "--jobs", "2"]);
    let printed = ok(&command.output().expect("spinel runs"), "build.rb");
    let gem = PathBuf::from(printed.lines().last().expect("build.rb names the gem"));
    assert_eq!(
        gem,
        out.join(format!("sqlite3-spinel-2.10.2.1-{}.gem", host_platform())),
        "{printed}"
    );
    gem
}

/// rubygems.org as `fixtures/sqlite3/index` has it, plus sqlite3-spinel:
/// `/versions` with its line added, `/info/sqlite3-spinel` the one build
/// `build_gem` made, and `/gems/` from `fixtures/install/gems` and that
/// build. Records every path asked for.
struct Source {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Source {
    fn start(gem: &Path) -> Source {
        let bytes = fs::read(gem).unwrap();
        let platform = host_platform();
        let info = format!(
            "---\n2.10.2.1-{platform} |checksum:{},ruby:>= 3.2\n",
            hex(&Sha256::digest(&bytes))
        );
        let versions = format!(
            "{}sqlite3-spinel 2.10.2.1-{platform} {}\n",
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
                        "info/sqlite3-spinel" => Some(info.clone().into_bytes()),
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

/// A scratch directory for one test: the gem's build, and a copy of
/// `fixtures/sqlite3/app` with a Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("sqlite3")
            .join(name);
        let _ = fs::remove_dir_all(root.join("project"));
        let _ = fs::remove_dir_all(root.join("home"));
        fs::create_dir_all(root.join("project")).unwrap();
        Sandbox { root }
    }

    /// The app, rubygems.org replaced by `source`.
    fn copy_app(&self, source: &Source) {
        for entry in fs::read_dir(fixture().join("app")).unwrap().flatten() {
            let text = fs::read_to_string(entry.path()).unwrap();
            self.write(
                &entry.file_name().to_string_lossy(),
                &text.replace(RUBYGEMS, &source.url),
            );
        }
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

    /// `spinel install --frozen` with sqlite3 on the list of substitutes.
    fn install(&self) -> Output {
        self.spinel()
            .args(["install", "--frozen"])
            .env("SPINEL_SUBSTITUTES", "sqlite3")
            .output()
            .expect("spinel runs")
    }
}

/// Kills the app the test started, however the test ends.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One HTTP/1.0 request to the app on `port`, as soon as it listens; its
/// whole response.
fn request(child: &mut Child, port: u16, request: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(Some(status)) = child.0.try_wait() {
            let mut err = String::new();
            let _ = child.0.stderr.as_mut().unwrap().read_to_string(&mut err);
            panic!("the app exited ({status}) before it served:\n{err}");
        }
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "the app did not listen on {port} in time"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn status_line(response: &str) -> &str {
    response.lines().next().unwrap_or("")
}

fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

#[test]
fn a_sinatra_app_on_sqlite_installs_with_the_spinel_gem_and_serves() {
    let sandbox = Sandbox::new("serve");
    let gem = build_gem(&sandbox.root);
    let source = Source::start(&gem);
    sandbox.copy_app(&source);
    let lock = sandbox.read("Gemfile.lock");
    let platform = host_platform();
    // Bundler's lockfile: upstream sqlite3's build for each platform,
    // nothing of Spinel's.
    assert!(
        lock.contains(&format!("    sqlite3 (2.10.2-{platform})\n")),
        "{lock}"
    );
    assert!(!lock.contains("spinel"), "{lock}");

    let out = sandbox.install();
    // The output names the locked gem, as for any other.
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        format!(
            "+ base64 0.3.0\n+ logger 1.7.0\n+ mustermann 3.1.1\n+ rack 3.2.8\n\
             + rack-protection 4.2.1\n+ rack-session 2.1.2\n+ rackup 2.3.1\n\
             + sinatra 4.2.1\n+ sqlite3 2.10.2-{platform}\n+ tilt 2.9.0\n+ webrick 1.9.2\n"
        )
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
      "name": "sqlite3",
      "platform": "{platform}",
      "substitute": "sqlite3-spinel",
      "substitute_platform": "{platform}",
      "substitute_version": "2.10.2.1",
      "version": "2.10.2"
    }}
  ]
}}
"#
        )
    );
    // Upstream's build was never fetched; the substitute was, once.
    let gems: Vec<String> = source
        .asked()
        .into_iter()
        .filter(|p| p.starts_with("/gems/sqlite3"))
        .collect();
    assert_eq!(
        gems,
        [format!("/gems/sqlite3-spinel-2.10.2.1-{platform}.gem")],
        "{:?}",
        source.asked()
    );

    // Bundler.setup activates the sqlite3 the lock names, and `require`
    // loads sqlite3-spinel's files and its extension.
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "bundler/setup"
require "sqlite3"
puts Gem.loaded_specs["sqlite3"].full_name
puts $LOADED_FEATURES.grep(/sqlite3_spinel\.spinel\z/).map { |f| File.basename(f) }
puts SQLite3::VERSION, SQLite3::SQLITE_VERSION"#,
        ])
        .output()
        .unwrap();
    assert_eq!(
        ok(&out, "spinel run -e"),
        format!("sqlite3-2.10.2-{platform}\nsqlite3_spinel.spinel\n2.10.2\n3.54.1\n")
    );

    // The API as CRuby's sqlite3 2.10.2 answers it.
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

    // The app writes a note and reads every note back in one request, then
    // reads one by id in another.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let database = sandbox.root.join("notes.db");
    let _ = fs::remove_file(&database);
    let mut child = Child(
        sandbox
            .spinel()
            .args(["run", "app.rb"])
            .env("PORT", port.to_string())
            .env("DATABASE", &database)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spinel runs"),
    );
    let form = "body=hello+from+Spinel";
    let response = request(
        &mut child,
        port,
        &format!(
            "POST /notes HTTP/1.0\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}",
            form.len()
        ),
    );
    assert!(
        status_line(&response).ends_with(" 201 Created"),
        "{response}"
    );
    assert_eq!(
        body(&response),
        "created 1\n1: hello from Spinel (sqlite3 2.10.2, SQLite 3.54.1)\n",
        "{response}"
    );
    let response = request(
        &mut child,
        port,
        "GET /notes/1 HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n",
    );
    assert!(status_line(&response).ends_with(" 200 OK"), "{response}");
    assert_eq!(body(&response), "hello from Spinel\n", "{response}");
    drop(child);

    // The note is on disk, for any connection to read.
    let out = sandbox
        .spinel()
        .args(["run", "-e"])
        .arg(r#"require "sqlite3"; p SQLite3::Database.new(ARGV[0], readonly: true).execute("SELECT id, body FROM notes")"#)
        .arg(&database)
        .output()
        .unwrap();
    assert_eq!(ok(&out, "spinel run -e"), "[[1, \"hello from Spinel\"]]\n");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}

/// Stock RubyGems and Bundler against the source that lists sqlite3-spinel:
/// CRuby 4.0's `gem` and `bundle`, which `SPINEL_CRUBY_BUNDLER` says are on
/// `PATH`, resolve `sqlite3` by name and never ask for `sqlite3-spinel`.
/// `bundle lock --update` resolves the project afresh to the same lockfile;
/// `bundle install` and `gem install sqlite3` go for upstream's own build,
/// which the source does not carry (it is upstream's 2.7 MB binary, never
/// needed on Spinel), so each stops there.
#[test]
fn stock_rubygems_and_bundler_never_pick_the_spinel_gem() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!(
            "skipped: set SPINEL_CRUBY_BUNDLER, with CRuby 4.0's gem and bundle on PATH, to run \
             stock RubyGems and Bundler against the source with sqlite3-spinel"
        );
        return;
    }
    let sandbox = Sandbox::new("stock");
    let gem = build_gem(&sandbox.root);
    let source = Source::start(&gem);
    sandbox.copy_app(&source);
    let lock = sandbox.read("Gemfile.lock");
    ok(&sandbox.install(), "spinel install --frozen");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

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
    assert!(asked.iter().any(|p| p == "/info/sqlite3"), "{asked:?}");
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");

    let (out, asked) = asked_by(&mut bundle(&["install"]));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("sqlite3-2.10.2-"), "{said}");
    assert!(
        asked.iter().any(|p| p.starts_with("/gems/sqlite3-2.10.2-")),
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
            "sqlite3",
            "--version",
            "2.10.2",
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
    assert!(asked.iter().any(|p| p == "/info/sqlite3"), "{asked:?}");
    assert!(
        asked
            .iter()
            .any(|p| p.starts_with("/quick/Marshal.4.8/sqlite3-2.10.2-")),
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");
}
