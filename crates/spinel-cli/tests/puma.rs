//! The puma-spinel gem (#116, docs/package-manager.md, section 6, step 2),
//! end to end: `gems/puma-spinel/build.rb` and `gems/nio4r-spinel/build.rb`,
//! run by `spinel`, build their extensions with cargo and package
//! `puma-spinel-8.1.2.1-<platform>.gem` and
//! `nio4r-spinel-2.7.5.1-<platform>.gem`; a source on 127.0.0.1 serves them
//! beside rubygems.org's own index lines and `.gem`s for two apps whose
//! `Gemfile.lock`s are the ones Bundler 4.0.20 wrote, locking upstream
//! `puma 8.1.2` and `nio4r 2.7.5` (source gems, `ruby`). `spinel install
//! --frozen`, with `puma` and `nio4r` on the list of substitutes
//! (`SPINEL_SUBSTITUTES`, as Spinel's own list holds no name until the
//! project owns it on rubygems.org), installs the gems in their place,
//! upstream's never fetched and the lockfile left byte for byte as Bundler
//! wrote it.
//!
//! `fixtures/puma/sinatra`, `../sinatra-lock`'s Sinatra app on Puma in
//! WEBrick's place, serves a request on Puma, answering what CRuby 4.0.9
//! with upstream Puma answered (`sinatra.txt`). `fixtures/puma/surface.rb`
//! prints what CRuby with upstream Puma printed of the C extension's API
//! (`surface.txt`). And Puma's own test suite, upstream's `test/` at 8.1.2
//! vendored in `fixtures/puma/app/test` (`gems/puma-spinel/vendor.sh`), runs
//! on the installed gem as Puma's `test/runner` runs it, offline, and every
//! test ends as it ended on CRuby 4.0.9 with upstream Puma on the machine
//! that recorded `suite.txt`, but those [`EXCLUDED`] names, which neither run
//! runs.
//!
//! And stock RubyGems and Bundler against the same source, when
//! `SPINEL_CRUBY_BUNDLER` says CRuby 4.0's are on `PATH` (CI's Linux leg):
//! they read `puma`'s builds and never ask for `puma-spinel`, which the
//! source lists, so a resolution leaves the lockfile as it was and an
//! install goes for upstream's own `.gem`.
//!
//! Nothing touches the network: cargo builds `--offline` from what the
//! workspace build fetched, every gem comes from the local source, and the
//! servers the suite starts listen on 127.0.0.1 or UNIX sockets.

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
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

/// What neither run of Puma's suite runs, the CRuby one that recorded
/// `suite.txt` (`fixtures/puma/build.rb` passes the same pattern) nor this
/// one, each for a reason the run cannot change:
///
/// - `TestWorkerGemIndependence` (6 tests) bundle-installs json 2.7 and
///   nio4r 2.7 releases from rubygems.org, which a run offline cannot reach,
///   on CRuby either.
/// - `TestPluginSystemd#test_systemd_notify` and
///   `#test_systemd_cluster_notify` expect the 5 threads Puma defaults to on
///   MRI; Puma gives any other Ruby 16 (`Puma.mri? ? 5 : 16`).
/// - `TestIntegrationPumactl#test_control_gc_stats_tcp` and `_unix` read
///   `GC.stat` under `count` on MRI and under TruffleRuby's `used` on any
///   other Ruby.
/// - `TestRackUp::RackUp#test_bin` and `#test_rackup1` skip unless the Ruby
///   is MRI, and `TestThreadPool#test_thread_name_linux` unless it is MRI on
///   Linux.
/// - `TestIntegrationSSLSession` (8 tests) is only defined when the Ruby is
///   MRI (`end if Puma::HAS_SSL && Puma::IS_MRI`).
///
/// Spinel's `RUBY_ENGINE` is `"spinel"`: it does not report itself as MRI.
#[cfg(target_os = "linux")]
const EXCLUDED: &str = "/\\A(?:TestWorkerGemIndependence|TestIntegrationSSLSession)#\
                        |\\A(?:TestPluginSystemd#test_systemd_(?:cluster_)?notify\
                        |TestIntegrationPumactl#test_control_gc_stats_(?:tcp|unix)\
                        |TestRackUp::RackUp#test_(?:bin|rackup1)\
                        |TestThreadPool#test_thread_name_linux)\\z/";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture() -> PathBuf {
    fixtures().join("puma")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The platform the `-spinel` gems' builds for this host are published under.
fn host_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") if !cfg!(target_env = "musl") => "x86_64-linux-gnu",
        ("linux", "aarch64") if !cfg!(target_env = "musl") => "aarch64-linux-gnu",
        ("macos", "aarch64") => "arm64-darwin",
        ("macos", "x86_64") => "x86_64-darwin",
        (os, arch) => panic!("puma-spinel has no platform named for {arch}-{os}"),
    }
}

/// Scrub what would change where gems come from or go, reach a proxy, or
/// change how Puma's suite runs (`GITHUB_ACTIONS` turns on a step summary
/// the recorded run did not write; the suite's run sets `CI` itself).
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
        "CI",
        "GITHUB_ACTIONS",
        "GITHUB_STEP_SUMMARY",
        "PUMA_DEBUG",
        "PUMA_TEST_DEBUG",
        "TEST_CASE_TIMEOUT",
        "MT_CPU",
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

/// `gems/<name>/build.rb`, run by this `spinel`, into `root`: the `.gem` it
/// writes, whose path it answers. Cargo's target directory is shared by the
/// tests, which take it in turn: cargo's own lock is not enough, as every
/// cargo run, fresh or not, removes and relinks the library in the profile
/// directory, where another test's `build.rb` may be looking for it.
fn build_gem(root: &Path, name: &str, version: &str) -> PathBuf {
    static BUILDING: Mutex<()> = Mutex::new(());
    let _turn = BUILDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let out = root.join(name);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../gems")
        .join(name)
        .join("build.rb");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("puma/target");
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
        out.join(format!("{name}-{version}-{}.gem", host_platform())),
        "{printed}"
    );
    gem
}

/// One `-spinel` gem the source serves: its name, version and bytes, and
/// its `/info` body.
struct Built {
    name: String,
    file: String,
    info: String,
    bytes: Vec<u8>,
}

impl Built {
    fn new(gem: &Path, name: &str, version: &str, ruby: &str, deps: &str) -> Built {
        let bytes = fs::read(gem).unwrap();
        let info = format!(
            "---\n{version}-{} {deps}|checksum:{},ruby:{ruby}\n",
            host_platform(),
            hex(&Sha256::digest(&bytes))
        );
        Built {
            name: name.to_owned(),
            file: gem.file_name().unwrap().to_string_lossy().into_owned(),
            info,
            bytes,
        }
    }
}

/// rubygems.org as `fixtures/puma/index` has it, plus the `-spinel` gems:
/// `/versions` with their lines added, `/info/<name>-spinel` each one's
/// build, and `/gems/` those builds and the `.gem`s in
/// `fixtures/install/gems`. Records every path asked for.
struct Source {
    url: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Source {
    fn start(built: Vec<Built>) -> Source {
        let mut versions = fs::read_to_string(fixture().join("index/versions")).unwrap();
        for gem in &built {
            let version = gem.info.lines().nth(1).unwrap().split(' ').next().unwrap();
            versions.push_str(&format!(
                "{} {version} {}\n",
                gem.name,
                hex(&Md5::digest(gem.info.as_bytes()))
            ));
        }
        let files = Arc::new((versions, built));

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&asked);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, files) = (Arc::clone(&log), Arc::clone(&files));
                std::thread::spawn(move || {
                    let (versions, built) = &*files;
                    serve(stream, &log, |path| {
                        if path == "versions" {
                            return Some(versions.clone().into_bytes());
                        }
                        for gem in built {
                            if path == format!("info/{}", gem.name) {
                                return Some(gem.info.clone().into_bytes());
                            }
                            if path == format!("gems/{}", gem.file) {
                                return Some(gem.bytes.clone());
                            }
                        }
                        match path.strip_prefix("gems/") {
                            Some(name) => fs::read(fixtures().join("install/gems").join(name)).ok(),
                            None => fs::read(fixture().join("index").join(path)).ok(),
                        }
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

/// `from` copied into `to`, directories and all, each file's bytes passed
/// through `edit` when they are text.
fn copy_tree(from: &Path, to: &Path, edit: &dyn Fn(String) -> String) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, edit);
        } else if kind.is_symlink() {
            let _ = fs::remove_file(&target);
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &target).unwrap();
        } else {
            let bytes = fs::read(entry.path()).unwrap();
            match String::from_utf8(bytes) {
                Ok(text) => fs::write(&target, edit(text)).unwrap(),
                Err(bytes) => fs::write(&target, bytes.into_bytes()).unwrap(),
            }
            let mode = fs::metadata(entry.path()).unwrap().permissions();
            fs::set_permissions(&target, mode).unwrap();
        }
    }
}

/// A scratch directory for one test: the gems' builds, and a copy of one of
/// `fixtures/puma`'s apps with a Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("puma")
            .join(name);
        let _ = fs::remove_dir_all(root.join("project"));
        let _ = fs::remove_dir_all(root.join("home"));
        fs::create_dir_all(root.join("project")).unwrap();
        Sandbox { root }
    }

    /// The app, rubygems.org replaced by `source`.
    fn copy_app(&self, app: &str, source: &Source) {
        copy_tree(&fixture().join(app), &self.project(), &|text| {
            text.replace(RUBYGEMS, &source.url)
        });
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
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

    /// `spinel install --frozen` with puma and nio4r on the list of
    /// substitutes.
    fn install(&self) -> Output {
        self.spinel()
            .args(["install", "--frozen"])
            .env("SPINEL_SUBSTITUTES", "puma,nio4r")
            .output()
            .expect("spinel runs")
    }
}

/// What one test installed: the sandbox, the source, and the lockfile as
/// Bundler wrote it.
struct Installed {
    sandbox: Sandbox,
    source: Source,
    lock: String,
}

/// The substitution `spinel install` records for the locked `name`
/// `version` by `<name>-spinel` `substitute`.
fn substitution(name: &str, version: &str, substitute: &str) -> String {
    format!(
        r#"    {{
      "kind": "native",
      "name": "{name}",
      "platform": "ruby",
      "substitute": "{name}-spinel",
      "substitute_platform": "{}",
      "substitute_version": "{substitute}",
      "version": "{version}"
    }}"#,
        host_platform()
    )
}

/// The substitution `spinel install` records for a locked gem Spinel has
/// built in.
fn builtin(name: &str, version: &str, builtin: &str) -> String {
    format!(
        r#"    {{
      "kind": "builtin",
      "name": "{name}",
      "platform": "ruby",
      "substitute_version": "{builtin}",
      "version": "{version}"
    }}"#
    )
}

/// Build the gems, serve them, and install `app` with them in puma's and
/// nio4r's place, checking the install did what substitution says: the app
/// locks both from rubygems.org, or, as Puma's suite does, Puma by its
/// gemspec from the checkout (`checkout`), which then holds puma-spinel's
/// `lib/` and `bin/`.
fn installed(name: &str, app: &str, listed: &str, checkout: bool) -> Installed {
    let sandbox = Sandbox::new(name);
    let puma = build_gem(&sandbox.root, "puma-spinel", "8.1.2.1");
    let nio4r = build_gem(&sandbox.root, "nio4r-spinel", "2.7.5.1");
    let source = Source::start(vec![
        Built::new(&puma, "puma-spinel", "8.1.2.1", ">= 3.0", "nio4r:~> 2.0"),
        Built::new(&nio4r, "nio4r-spinel", "2.7.5.1", ">= 2.6", ""),
    ]);
    sandbox.copy_app(app, &source);
    if checkout {
        unpack(&puma, &sandbox.project());
    }
    let lock = sandbox.read("Gemfile.lock");
    let platform = host_platform();
    // Bundler's lockfile: upstream Puma and nio4r, source gems, nothing of
    // Spinel's.
    if checkout {
        assert!(
            lock.starts_with("PATH\n  remote: .\n  specs:\n    puma (8.1.2)\n"),
            "{lock}"
        );
    } else {
        assert!(lock.contains("    puma (8.1.2)\n"), "{lock}");
    }
    assert!(lock.contains("    nio4r (2.7.5)\n"), "{lock}");
    assert!(!lock.contains("spinel"), "{lock}");

    let out = sandbox.install();
    // The output names the locked gems, as for any other.
    assert_eq!(ok(&out, "spinel install --frozen"), listed);
    assert_eq!(sandbox.read("Gemfile.lock"), lock, "the lockfile changed");
    // The suite's lockfile also pins two of Spinel's built-ins.
    let mut substituted = if checkout {
        vec![
            builtin("bigdecimal", "4.1.3", "4.0.1"),
            builtin("json", "2.18.3", "2.18.0"),
        ]
    } else {
        Vec::new()
    };
    substituted.push(substitution("nio4r", "2.7.5", "2.7.5.1"));
    let mut fetched = vec![format!("/gems/nio4r-spinel-2.7.5.1-{platform}.gem")];
    if !checkout {
        substituted.push(substitution("puma", "8.1.2", "8.1.2.1"));
        fetched.push(format!("/gems/puma-spinel-8.1.2.1-{platform}.gem"));
    }
    assert_eq!(
        sandbox.read(".spinel/substitutions.json"),
        format!(
            "{{\n  \"substitutions\": [\n{}\n  ]\n}}\n",
            substituted.join(",\n")
        )
    );
    // Upstream's gems were never fetched; the substitutes were, once each.
    let mut gems: Vec<String> = source
        .asked()
        .into_iter()
        .filter(|p| p.starts_with("/gems/puma") || p.starts_with("/gems/nio4r"))
        .collect();
    gems.sort();
    assert_eq!(gems, fetched, "{:?}", source.asked());
    Installed {
        sandbox,
        source,
        lock,
    }
}

/// The `lib/` and `bin/` of the `.gem` at `gem` into `into`, as a checkout
/// of Puma has them.
fn unpack(gem: &Path, into: &Path) {
    let mut tar = Command::new("tar");
    tar.arg("-xOf").arg(gem).arg("data.tar.gz");
    let data = tar.output().expect("tar runs");
    assert!(data.status.success(), "{data:?}");
    let mut untar = Command::new("tar")
        .args(["-xzf", "-", "lib", "bin"])
        .current_dir(into)
        .stdin(Stdio::piped())
        .spawn()
        .expect("tar runs");
    untar.stdin.take().unwrap().write_all(&data.stdout).unwrap();
    assert!(untar.wait().unwrap().success());
}

/// What `spinel install` prints for the Puma suite's lockfile. bigdecimal
/// and json are Spinel's built-ins; the lock's bigdecimal is a minor
/// version past the built-in's, which the install says on stderr.
#[cfg(target_os = "linux")]
const APP_INSTALLED: &str = "+ bake 0.25.0\n+ bigdecimal 4.1.3\n+ concurrent-ruby 1.3.8\n\
                             + console 1.38.0\n+ fiber-annotation 0.2.0\n+ fiber-local 1.1.0\n\
                             + fiber-storage 1.0.1\n+ json 2.18.3\n+ localhost 1.8.0\n\
                             + logger 1.7.0\n+ minitest 5.27.0\n+ minitest-mock 5.27.0\n\
                             + minitest-proveit 1.0.1\n+ minitest-retry 0.3.1\n\
                             + minitest-stub-const 0.6\n+ nio4r 2.7.5\n+ puma 8.1.2\n\
                             + rack 3.2.8\n+ rackup 2.3.1\n+ samovar 2.5.1\n";

/// What `spinel install` prints for the Sinatra app's lockfile.
const SINATRA_INSTALLED: &str = "+ base64 0.3.0\n+ logger 1.7.0\n+ mustermann 3.1.1\n+ nio4r 2.7.5\n\
                                 + puma 8.1.2\n+ rack 3.2.8\n+ rack-protection 4.2.1\n\
                                 + rack-session 2.1.2\n+ rackup 2.3.1\n+ sinatra 4.2.1\n\
                                 + tilt 2.9.0\n";

/// The `/` the Sinatra app on Puma answers, as CRuby 4.0.9 with upstream
/// Puma 8.1.2 answered it, `Connection: close` asked.
#[test]
fn sinatra_serves_a_request_on_puma_spinel() {
    let Installed { sandbox, lock, .. } = installed("sinatra", "sinatra", SINATRA_INSTALLED, false);

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut server = sandbox
        .spinel()
        .args(["run", "app.rb"])
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    let deadline = Instant::now() + Duration::from_secs(60);
    let response = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                stream
                    .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                    .unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).unwrap();
                break response;
            }
            Err(_) if Instant::now() < deadline => {
                if let Some(status) = server.try_wait().unwrap() {
                    let mut stderr = String::new();
                    server
                        .stderr
                        .take()
                        .unwrap()
                        .read_to_string(&mut stderr)
                        .unwrap();
                    panic!("app.rb ended ({status}) before it served\n{stderr}");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = server.kill();
                panic!("app.rb never listened on {port}: {e}");
            }
        }
    };
    let _ = server.kill();
    let _ = server.wait();
    let expected = fs::read(fixture().join("sinatra.txt")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&response),
        String::from_utf8_lossy(&expected)
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}

/// The C extension's API as CRuby's Puma 8.1.2 answers it, call for call,
/// from Puma's own `require`.
#[test]
fn puma_spinel_answers_as_pumas_c_extension() {
    let Installed { sandbox, .. } = installed("surface", "sinatra", SINATRA_INSTALLED, false);

    // Bundler.setup activates the Puma the lock names, and `require` loads
    // puma-spinel's files and its extension.
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            r#"require "bundler/setup"
require "puma"
puts Gem.loaded_specs["puma"].full_name
puts $LOADED_FEATURES.grep(/puma_spinel\.spinel\z/).map { |f| File.basename(f) }
p Puma::HAS_SSL, Puma::Const::PUMA_VERSION"#,
        ])
        .output()
        .unwrap();
    assert_eq!(
        ok(&out, "spinel run -e"),
        "puma-8.1.2\npuma_spinel.spinel\ntrue\n\"8.1.2\"\n"
    );

    let out = sandbox
        .spinel()
        .arg("-rbundler/setup")
        .arg(fixture().join("surface.rb"))
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
}

/// The TLS 1.3 client `TestPumaServerSSLKeyUpdate` floods Puma's server
/// with KeyUpdates from, which the test compiles from C with
/// `RbConfig::CONFIG["CC"]`, a C compiler Spinel does not name (#867):
/// `gems/puma-spinel/ext/puma_spinel/examples/key_update_peer.rs`, the C's
/// OpenSSL calls in Rust, built by cargo into `build_gem`'s target
/// directory, offline. Its path, for `PUMA_KEY_UPDATE_PEER`, which
/// `fixtures/puma/key_update_peer.rb` hands the test in the C's place.
#[cfg(target_os = "linux")]
fn key_update_peer() -> PathBuf {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("puma/target");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = scrubbed(Command::new(cargo));
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
        .args(["--quiet", "--package", "puma-spinel", "--example"])
        .arg("key_update_peer")
        .arg("--target-dir")
        .arg(&target)
        .args(["--profile", "dev", "--offline", "--jobs", "2"]);
    ok(
        &command.output().expect("cargo runs"),
        "cargo build key_update_peer",
    );
    let peer = target.join("debug/examples/key_update_peer");
    assert!(peer.is_file(), "cargo wrote no {}", peer.display());
    peer
}

/// Puma's own suite, every file under `test/` but the [`EXCLUDED`] tests,
/// as its `test/runner` runs it from a checkout: the installed gem's `lib/`
/// and `bin/` at the project's root, where the suite starts `bin/puma` with
/// `-Ilib`; `TestPumaServerSSLKeyUpdate` with [`key_update_peer`]'s client
/// in place of the C one it compiles, as the CRuby run had it. Each test
/// ends as it ended on CRuby (`suite.txt`): passed, or skipped by the suite
/// itself where it skipped there. Linux only: the suite runs and skips
/// tests by platform, and `suite.txt` is Linux's. And only built with
/// optimizations, as CI's Linux leg runs it.
#[test]
#[cfg(target_os = "linux")]
fn pumas_own_suite_passes_as_on_cruby() {
    // The suite times its servers against seconds-long deadlines, which an
    // unoptimized interpreter does not keep; CI runs it with `--release`.
    if cfg!(debug_assertions) {
        eprintln!(
            "skipped: Puma's suite runs against an optimized spinel: \
             cargo test --release -p spinel-cli --test puma pumas_own_suite"
        );
        return;
    }
    let Installed { sandbox, .. } = installed("suite", "app", APP_INSTALLED, true);
    let peer = key_update_peer();

    let written = sandbox.root.join("results.txt");
    let _ = fs::remove_file(&written);
    let out = sandbox
        .spinel()
        .arg("-rbundler/setup")
        .arg(format!("-r{}", fixture().join("results.rb").display()))
        .arg(format!(
            "-r{}",
            fixture().join("key_update_peer.rb").display()
        ))
        .args(["-Ilib", "test/runner", "-v"])
        .arg(format!("--exclude={EXCLUDED}"))
        // minitest-retry, which Puma's own CI runs its suite with for the
        // tests that race the server, and the recorded run had on.
        .env("CI", "1")
        // The classes that ask to run their tests in parallel run one test
        // at a time, as the recorded run did: their tests time servers
        // against fixed sleeps, and a Ractor's threads take turns.
        .env("MT_CPU", "1")
        .env("PUMA_SUITE_RESULTS", &written)
        .env("PUMA_KEY_UPDATE_PEER", &peer)
        .output()
        .unwrap();
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    let report = format!(
        "{}\nstderr: {}",
        printed
            .lines()
            .rev()
            .take(200)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
        String::from_utf8_lossy(&out.stderr)
    );

    let ran: BTreeMap<String, String> = results(&fs::read_to_string(&written).unwrap_or_default());
    let recorded: BTreeMap<String, String> =
        results(&fs::read_to_string(fixture().join("suite.txt")).unwrap());
    let differ: Vec<String> = recorded
        .keys()
        .chain(ran.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|name| ran.get(*name) != recorded.get(*name))
        .map(|name| {
            format!(
                "{name}: CRuby {}, Spinel {}",
                recorded.get(name).map_or("did not run", String::as_str),
                ran.get(name).map_or("did not run", String::as_str)
            )
        })
        .collect();
    assert!(
        differ.is_empty(),
        "{} of {} tests ended otherwise than on CRuby:\n{}\n\n{report}",
        differ.len(),
        recorded.len(),
        differ.join("\n")
    );
    // CRuby passed or skipped every test it ran, so the run passes too.
    assert!(out.status.success(), "{report}");
    assert!(
        recorded.values().all(|mark| mark == "." || mark == "S"),
        "suite.txt records a failure"
    );
}

#[cfg(target_os = "linux")]
/// Each test's mark by name, from `<mark> <Class>#<test>` lines as
/// `fixtures/puma/results.rb` writes them and `suite.txt` keeps them.
fn results(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(mark, name)| (name.to_owned(), mark.to_owned()))
        .collect()
}

/// Stock RubyGems and Bundler against the source that lists puma-spinel:
/// CRuby 4.0's `gem` and `bundle`, which `SPINEL_CRUBY_BUNDLER` says are on
/// `PATH`, resolve `puma` by name and never ask for `puma-spinel`. `bundle
/// lock --update` resolves the Sinatra app afresh to the same lockfile;
/// `bundle install` and `gem install puma` go for upstream's own gem, which
/// the source does not carry, so each stops there.
#[test]
fn stock_rubygems_and_bundler_never_pick_the_spinel_gem() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!(
            "skipped: set SPINEL_CRUBY_BUNDLER, with CRuby 4.0's gem and bundle on PATH, to run \
             stock RubyGems and Bundler against the source with puma-spinel"
        );
        return;
    }
    let Installed {
        sandbox,
        source,
        lock,
    } = installed("stock", "sinatra", SINATRA_INSTALLED, false);

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
    assert!(asked.iter().any(|p| p == "/info/puma"), "{asked:?}");
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");

    let (out, asked) = asked_by(&mut bundle(&["install"]));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{said}");
    assert!(
        asked
            .iter()
            .any(|p| p == "/gems/puma-8.1.2.gem" || p == "/gems/nio4r-2.7.5.gem"),
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
            "puma",
            "--version",
            "8.1.2",
            "--ignore-dependencies",
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
    assert!(asked.iter().any(|p| p == "/info/puma"), "{asked:?}");
    assert!(
        asked
            .iter()
            .any(|p| p.starts_with("/quick/Marshal.4.8/puma-8.1.2")),
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("spinel")), "{asked:?}");
}
