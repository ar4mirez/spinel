//! The pg-spinel gem (#114, docs/package-manager.md, section 6, step 2),
//! end to end: `gems/pg-spinel/build.rb`, run by `spinel`, builds its
//! extension with cargo, linked with the system's libpq, and packages
//! `pg-spinel-1.7.0.1-<platform>.gem`; a source on 127.0.0.1 serves it
//! beside rubygems.org's own index lines for `fixtures/pg/app`, whose
//! `Gemfile.lock` is the one Bundler 4.0.20 wrote, locking upstream `pg
//! 1.7.0` for this platform. `spinel install --frozen`, with `pg` on the
//! list of substitutes (`SPINEL_SUBSTITUTES`, as Spinel's own list holds no
//! name until the project owns it on rubygems.org), installs the gem in
//! pg's place, upstream's never fetched and the lockfile left byte for byte
//! as Bundler wrote it.
//!
//! With a PostgreSQL server at `SPINEL_PG_TEST_URL` (CI's Linux leg runs
//! one as a service container), the app connects and round-trips a query —
//! a table made, rows written with parameters, read back, a unique
//! violation raised — and `fixtures/pg/surface.rb` prints the same as CRuby
//! with upstream pg 1.7.0 printed (`surface.txt`). Without one, that test
//! says so and passes.
//!
//! And stock RubyGems and Bundler against the same source, when
//! `SPINEL_CRUBY_BUNDLER` says CRuby 4.0's are on `PATH` (CI's Linux leg):
//! they read `pg`'s builds and never ask for `pg-spinel`, which the source
//! lists, so a resolution leaves the lockfile as it was and an install goes
//! for upstream's own build.
//!
//! The extension links the system's libpq, which `pg_config` (or
//! `$PG_CONFIG`) or `$PQ_LIB_DIR` finds, as its `build.rs` does; it is not a
//! default workspace member, so a machine without libpq builds Spinel. There
//! each test says it skipped, except under `CI`, where it fails.
//!
//! Nothing touches the network but the PostgreSQL server: cargo builds
//! `--offline` from what the workspace build fetched, and every gem comes
//! from the local source.

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

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pg")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The platform pg-spinel's build for this host is published under, and
/// the build of upstream pg the fixture's lockfile locks for it.
fn platforms() -> (&'static str, &'static str) {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") if !cfg!(target_env = "musl") => ("x86_64-linux-gnu", "x86_64-linux"),
        ("linux", "aarch64") if !cfg!(target_env = "musl") => {
            ("aarch64-linux-gnu", "aarch64-linux")
        }
        ("macos", "aarch64") => ("arm64-darwin", "arm64-darwin"),
        ("macos", "x86_64") => ("x86_64-darwin", "x86_64-darwin"),
        (os, arch) => panic!("fixtures/pg/app/Gemfile.lock locks no pg for {arch}-{os}"),
    }
}

/// The server `SPINEL_PG_TEST_URL` names, if it is set.
fn server_url() -> Option<String> {
    std::env::var("SPINEL_PG_TEST_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

/// Whether libpq is there to build the extension with, found as its
/// `build.rs` finds it. Where it is not, a test skips, saying so, but on CI
/// (`CI` set), whose legs install it, it fails.
fn libpq_found() -> bool {
    let found = std::env::var_os("PQ_LIB_DIR").is_some_and(|dir| !dir.is_empty()) || {
        let pg_config = std::env::var_os("PG_CONFIG").unwrap_or_else(|| "pg_config".into());
        Command::new(pg_config)
            .arg("--libdir")
            .output()
            .is_ok_and(|out| out.status.success() && !out.stdout.trim_ascii().is_empty())
    };
    if !found {
        let why = "libpq (libpq-dev, or Homebrew's libpq), found by pg_config ($PG_CONFIG) \
                   or $PQ_LIB_DIR, is needed to build the pg-spinel gem's extension";
        assert!(
            std::env::var_os("CI").is_none(),
            "{why}, and CI must build it"
        );
        eprintln!("skipped: {why}");
    }
    found
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

/// `build.rb`, run by this `spinel`, into `root`: the `.gem` it writes,
/// whose path it answers. Cargo's target directory is shared by the tests,
/// which cargo's lock on it takes in turn.
fn build_gem(root: &Path) -> PathBuf {
    let out = root.join("gem");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gems/pg-spinel/build.rb");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pg/target");
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
        out.join(format!("pg-spinel-1.7.0.1-{}.gem", platforms().0)),
        "{printed}"
    );
    gem
}

/// rubygems.org as `fixtures/pg/index` has it, plus pg-spinel: `/versions`
/// with its line added, `/info/pg-spinel` the one build `build_gem` made,
/// and `/gems/` that build alone. Records every path asked for.
struct Source {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Source {
    fn start(gem: &Path) -> Source {
        let bytes = fs::read(gem).unwrap();
        let platform = platforms().0;
        let info = format!(
            "---\n1.7.0.1-{platform} |checksum:{},ruby:>= 3.1\n",
            hex(&Sha256::digest(&bytes))
        );
        let versions = format!(
            "{}pg-spinel 1.7.0.1-{platform} {}\n",
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
                        "info/pg-spinel" => Some(info.clone().into_bytes()),
                        _ => match path.strip_prefix("gems/") {
                            Some(name) if name == gem_name => Some(gem.clone()),
                            Some(_) => None,
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
/// `fixtures/pg/app` with a Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pg").join(name);
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

    /// `spinel install --frozen` with pg on the list of substitutes.
    fn install(&self) -> Output {
        self.spinel()
            .args(["install", "--frozen"])
            .env("SPINEL_SUBSTITUTES", "pg")
            .output()
            .expect("spinel runs")
    }
}

/// Build the gem, serve it, and install the app with it in pg's place,
/// checking the install did what substitution says; the sandbox, the
/// source, and the lockfile as Bundler wrote it.
fn installed(name: &str) -> (Sandbox, Source, String) {
    let sandbox = Sandbox::new(name);
    let gem = build_gem(&sandbox.root);
    let source = Source::start(&gem);
    sandbox.copy_app(&source);
    let lock = sandbox.read("Gemfile.lock");
    let (spinel_platform, locked_platform) = platforms();
    // Bundler's lockfile: upstream pg's build for each platform, nothing of
    // Spinel's.
    assert!(
        lock.contains(&format!("    pg (1.7.0-{locked_platform})\n")),
        "{lock}"
    );
    assert!(!lock.contains("spinel"), "{lock}");

    let out = sandbox.install();
    // The output names the locked gem, as for any other.
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        format!("+ pg 1.7.0-{locked_platform}\n")
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
      "name": "pg",
      "platform": "{locked_platform}",
      "substitute": "pg-spinel",
      "substitute_platform": "{spinel_platform}",
      "substitute_version": "1.7.0.1",
      "version": "1.7.0"
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
        .filter(|p| p.starts_with("/gems/pg"))
        .collect();
    assert_eq!(
        gems,
        [format!("/gems/pg-spinel-1.7.0.1-{spinel_platform}.gem")],
        "{:?}",
        source.asked()
    );
    (sandbox, source, lock)
}

/// The install, and the gem loading on any machine that builds it: no
/// server needed.
#[test]
fn pg_spinel_installs_in_pgs_place_and_loads() {
    if !libpq_found() {
        return;
    }
    let (sandbox, _source, lock) = installed("install");
    let locked_platform = platforms().1;

    // Bundler.setup activates the pg the lock names, and `require` loads
    // pg-spinel's files and its extension, linked with this libpq.
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "bundler/setup"
require "pg"
puts Gem.loaded_specs["pg"].full_name
puts $LOADED_FEATURES.grep(/pg_spinel\.spinel\z/).map { |f| File.basename(f) }
puts PG::VERSION, PG.library_version >= 100000, PG::IS_BINARY_GEM
p PG::Connection.escape_string("O'Reilly"), PG::Connection.quote_ident(%w[s t])
p PG::ERROR_CLASSES["23505"], PG::UniqueViolation.ancestors.take(4)
begin
  PG.connect(host: "127.0.0.1", port: 1, connect_timeout: 5)
rescue PG::ConnectionBad => e
  p [e.class, e.message.include?("Connection refused")]
end
conn = PG::Connection.allocate
copies = [conn.dup, conn.clone]
p copies.map { |c| [c.class, c.equal?(conn), c.finished?] }
copies.each(&:finish) rescue p [$!.class, $!.message]
p conn.finished?"#,
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(
        ok(&out, "spinel run -e"),
        format!(
            "pg-1.7.0-{locked_platform}\npg_spinel.spinel\n1.7.0\ntrue\nfalse\n\
             \"O''Reilly\"\n\"\\\"s\\\".\\\"t\\\"\"\n\
             PG::UniqueViolation\n[PG::UniqueViolation, PG::IntegrityConstraintViolation, PG::ServerError, PG::Error]\n\
             [PG::ConnectionBad, true]\n\
             [[PG::Connection, false, true], [PG::Connection, false, true]]\n\
             [PG::ConnectionBad, \"connection is closed\"]\n\
             true\n"
        )
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}

/// The fixture connects to the server and round-trips a query, and the API
/// answers as CRuby's pg 1.7.0 does, when `SPINEL_PG_TEST_URL` names a
/// server.
#[test]
fn a_fixture_connects_to_postgres_and_round_trips_a_query() {
    let Some(url) = server_url() else {
        eprintln!(
            "skipped: set SPINEL_PG_TEST_URL to a PostgreSQL 18 server's URL \
             (postgres://user:password@host:port/database) to connect the pg fixture to it"
        );
        return;
    };
    if !libpq_found() {
        return;
    }
    let (sandbox, _source, lock) = installed("round-trip");

    // The app: a table made, rows written with parameters and in a
    // transaction, read back, a prepared statement, a unique violation.
    let out = sandbox
        .spinel()
        .args(["run", "app.rb"])
        .env("DATABASE_URL", &url)
        .output()
        .unwrap();
    assert_eq!(
        ok(&out, "spinel run app.rb"),
        "INSERT 0 1: id 1\n\
         1: hello from Spinel (pg 1.7.0)\n\
         2: héllo again (a transaction)\n\
         héllo again\n\
         PG::UniqueViolation: 23505 on spinel_pg_notes_body_key\n\
         ERROR:  duplicate key value violates unique constraint \"spinel_pg_notes_body_key\"\n\
         2\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");

    // The API as CRuby's pg 1.7.0 answers it.
    sandbox.write(
        "surface.rb",
        &fs::read_to_string(fixture().join("surface.rb")).unwrap(),
    );
    let out = sandbox
        .spinel()
        .args(["run", "surface.rb"])
        .env("SPINEL_PG_TEST_URL", &url)
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

    // A copy is upstream's: a connection's a fresh, unconnected one, a
    // result's refused, a cancel connection's closed; the original keeps
    // its own libpq handle, freed once.
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "bundler/setup"
require "pg"
conn = PG.connect(ENV.fetch("SPINEL_PG_TEST_URL"))
copy = conn.dup
p [copy.finished?, conn.finished?]
copy.exec("SELECT 1") rescue p [$!.class, $!.message]
res = conn.exec("SELECT 1 AS one")
[:dup, :clone].each { |m| res.public_send(m) rescue p [$!.class, $!.message] }
if defined?(PG::CancelConnection)
  cancel = PG::CancelConnection.new(conn).dup
  cancel.start rescue p [$!.class, $!.message]
else
  p :no_cancel_connection
end
copy = nil
GC.start
p res.values, conn.exec("SELECT 2").getvalue(0, 0)
conn.finish"#,
        ])
        .env("SPINEL_PG_TEST_URL", &url)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let printed = ok(&out, "spinel run -e (copies)");
    // PG::CancelConnection exists only with libpq 17 or later.
    let cancel = if printed.contains(":no_cancel_connection\n") {
        ":no_cancel_connection"
    } else {
        "[PG::ConnectionBad, \"PG::CancelConnection is closed\"]"
    };
    assert_eq!(
        printed,
        format!(
            "[true, false]\n\
             [PG::ConnectionBad, \"connection is closed\"]\n\
             [TypeError, \"allocator undefined for PG::Result\"]\n\
             [TypeError, \"allocator undefined for PG::Result\"]\n\
             {cancel}\n\
             [[\"1\"]]\n\
             \"2\"\n"
        )
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}

/// Stock RubyGems and Bundler against the source that lists pg-spinel:
/// CRuby 4.0's `gem` and `bundle`, which `SPINEL_CRUBY_BUNDLER` says are on
/// `PATH`, resolve `pg` by name and never ask for `pg-spinel`. `bundle lock
/// --update` resolves the project afresh to the same lockfile; `bundle
/// install` and `gem install pg` go for upstream's own build, which the
/// source does not carry, so each stops there.
#[test]
fn stock_rubygems_and_bundler_never_pick_the_spinel_gem() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!(
            "skipped: set SPINEL_CRUBY_BUNDLER, with CRuby 4.0's gem and bundle on PATH, to run \
             stock RubyGems and Bundler against the source with pg-spinel"
        );
        return;
    }
    if !libpq_found() {
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
    assert!(asked.iter().any(|p| p == "/info/pg"), "{asked:?}");
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");

    let (out, asked) = asked_by(&mut bundle(&["install"]));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("pg-1.7.0-"), "{said}");
    assert!(
        asked.iter().any(|p| p.starts_with("/gems/pg-1.7.0-")),
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
            "pg",
            "--version",
            "1.7.0",
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
    assert!(asked.iter().any(|p| p == "/info/pg"), "{asked:?}");
    assert!(
        asked
            .iter()
            .any(|p| p.starts_with("/quick/Marshal.4.8/pg-1.7.0-")),
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");
}
