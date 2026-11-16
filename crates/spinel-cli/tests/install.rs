//! `spinel install` (#84, docs/package-manager.md, "Pipeline") against
//! rubygems.org's own files served on 127.0.0.1 (`fixtures/install/`, taken
//! by its `snapshot.sh`): the `plain` fixture resolved afresh and the Sinatra
//! fixture's lockfile installed as it is, each into a project of its own
//! with a Spinel home of its own, and `--frozen`'s answer when the Gemfile
//! and the lockfile agree and when they do not. Each fixture's Gemfile and
//! lockfile name rubygems.org; the copy the test installs names the local
//! server instead, and nothing touches the network beyond 127.0.0.1.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const RUBYGEMS: &str = "https://rubygems.org";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// rubygems.org as the snapshot has it: `/versions` and `/info/<gem>` from
/// `install/index/`, `/gems/<file>` from `install/gems/`. Counts the gems
/// it serves.
struct Server {
    url: String,
    gems_served: Arc<AtomicUsize>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let gems_served = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&gems_served);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let count = Arc::clone(&count);
                std::thread::spawn(move || serve(stream, &count));
            }
        });
        Server { url, gems_served }
    }

    fn gems_served(&self) -> usize {
        self.gems_served.load(Ordering::SeqCst)
    }
}

fn serve(stream: TcpStream, gems_served: &AtomicUsize) {
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
    let root = fixtures().join("install");
    let file = path
        .strip_prefix('/')
        .filter(|p| !p.split('/').any(|part| part == ".." || part.is_empty()))
        .and_then(|p| match p.strip_prefix("gems/") {
            Some(gem) => {
                gems_served.fetch_add(1, Ordering::SeqCst);
                fs::read(root.join("gems").join(gem)).ok()
            }
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

/// The rake the snapshot holds, which a fresh resolution of `plain` takes.
fn rake_version() -> String {
    fs::read_dir(fixtures().join("install/gems"))
        .unwrap()
        .flatten()
        .find_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Some(name.strip_prefix("rake-")?.strip_suffix(".gem")?.to_owned())
        })
        .expect("a rake gem in the snapshot")
}

/// A project and a Spinel home of its own, under the test's scratch
/// directory.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("install")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        Sandbox { root }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// Copy a fixture's files into the project, rubygems.org replaced by
    /// `server`.
    fn copy(&self, fixture: &str, server: &Server) {
        for entry in fs::read_dir(fixtures().join(fixture)).unwrap().flatten() {
            let text = fs::read_to_string(entry.path()).unwrap();
            self.write(
                &entry.file_name().to_string_lossy(),
                &text.replace(RUBYGEMS, &server.url),
            );
        }
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.project().join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    /// `spinel` with this sandbox's home, run in the project, with nothing
    /// of the environment's gems or Bundler settings.
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

    fn install(&self, args: &[&str]) -> Output {
        self.spinel()
            .arg("install")
            .args(args)
            .output()
            .expect("spinel runs")
    }

    /// The gems linked into the project's GEM_HOME, by full name.
    fn linked(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.project().join(".spinel/gem_home/gems"))
            .map(|dir| {
                dir.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Its standard output, after checking it exited 0.
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

/// The lockfile's text from `section` to the blank line after it.
fn section(lock: &str, name: &str) -> String {
    lock.lines()
        .skip_while(|l| *l != name)
        .take_while(|l| !l.is_empty())
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn plain_installs_rake_and_run_requires_it() {
    let server = Server::start();
    let sandbox = Sandbox::new("plain");
    sandbox.copy("plain", &server);
    let rake = rake_version();

    let out = sandbox.install(&[]);
    assert_eq!(ok(&out, "spinel install"), format!("+ rake {rake}\n"));
    assert_eq!(stderr(&out), "");

    // The lockfile `bundle lock` writes where there is none.
    let lock = sandbox.read("Gemfile.lock");
    let sha = fs::read_to_string(fixtures().join("install/index/info/rake"))
        .unwrap()
        .lines()
        .find(|l| l.starts_with(&format!("{rake} ")))
        .and_then(|l| l.split("checksum:").nth(1))
        .map(|rest| rest.split(',').next().unwrap().to_owned())
        .unwrap();
    assert_eq!(
        section(&lock, "GEM"),
        format!(
            "GEM\n  remote: {}/\n  specs:\n    rake ({rake})\n",
            server.url
        )
    );
    // `ruby` and the local platform, sorted as Bundler sorts them: on
    // macOS `arm64-darwin-*` comes before `ruby`, on Linux after it.
    let platforms = section(&lock, "PLATFORMS");
    let platforms: Vec<&str> = platforms.lines().skip(1).collect();
    assert!(platforms.contains(&"  ruby"), "{platforms:?}");
    assert!(platforms.is_sorted(), "{platforms:?}");
    assert_eq!(section(&lock, "DEPENDENCIES"), "DEPENDENCIES\n  rake\n");
    assert_eq!(
        section(&lock, "CHECKSUMS"),
        format!("CHECKSUMS\n  rake ({rake}) sha256={sha}\n")
    );
    assert_eq!(section(&lock, "BUNDLED WITH"), "BUNDLED WITH\n  4.0.20\n");

    assert_eq!(sandbox.linked(), [format!("rake-{rake}")]);
    assert_eq!(
        sandbox.read(".bundle/config"),
        "---\nBUNDLE_PATH: \".spinel/gem_home\"\n"
    );

    // `spinel run -e` is `ruby -e` with the project's gems visible.
    let out = sandbox
        .spinel()
        .args(["run", "-e", "require \"rake\"; puts Rake::VERSION"])
        .output()
        .unwrap();
    assert_eq!(ok(&out, "spinel run -e"), format!("{rake}\n"));
    // A subdirectory of the project sees them too.
    fs::create_dir_all(sandbox.project().join("lib")).unwrap();
    let out = sandbox
        .spinel()
        .current_dir(sandbox.project().join("lib"))
        .args(["run", "-e", "require \"rake\"; puts Rake::VERSION"])
        .output()
        .unwrap();
    assert_eq!(ok(&out, "spinel run -e in lib/"), format!("{rake}\n"));
}

/// Where the require map (#66) answers for each file, once `gem "rake"` has
/// put rake's directory on `$LOAD_PATH`: `map` if the map says some entry
/// has it, else `disk`. Then rake loads.
const REQUIRE_MAP_PROBE: &str = r##"gem "rake"
dirs = Kernel.__expanded_load_path__
%w[rake.rb rake/version.rb optparse.rb].each do |file|
  at = Kernel.__require_map_scan__(dirs, file, 0)
  at = Kernel.__require_map_scan__(dirs, file, at + 1) while at && at >= 0
  puts "#{file}: #{at ? "map" : "disk"}"
end
require "rake"
puts Rake::VERSION
"##;

#[test]
fn install_writes_the_require_map_and_programs_use_it() {
    let server = Server::start();
    let sandbox = Sandbox::new("require-map");
    sandbox.copy("plain", &server);
    ok(&sandbox.install(&[]), "the first install");
    let map = sandbox.project().join(".spinel/require_map.bin");
    assert!(map.is_file(), "spinel install writes {}", map.display());
    let expected = format!(
        "rake.rb: map\nrake/version.rb: map\noptparse.rb: map\n{}\n",
        rake_version()
    );
    let probe = || {
        let out = sandbox
            .spinel()
            .args(["run", "-e", REQUIRE_MAP_PROBE])
            .output()
            .unwrap();
        ok(&out, "the probe")
    };
    assert_eq!(probe(), expected);

    // Every install writes it again: a damaged map, and none at all, are
    // replaced by one the next run uses.
    fs::write(&map, b"not a require map").unwrap();
    assert_eq!(
        probe(),
        format!(
            "rake.rb: disk\nrake/version.rb: disk\noptparse.rb: disk\n{}\n",
            rake_version()
        )
    );
    let out = sandbox.install(&[]);
    assert_eq!(
        (ok(&out, "the second install"), stderr(&out)),
        (String::new(), String::new())
    );
    assert_eq!(probe(), expected);
    fs::remove_file(&map).unwrap();
    ok(&sandbox.install(&["--frozen"]), "a frozen install");
    assert_eq!(probe(), expected);
}

#[test]
fn a_warm_install_fetches_nothing_and_writes_nothing() {
    let server = Server::start();
    let sandbox = Sandbox::new("warm");
    sandbox.copy("plain", &server);
    ok(&sandbox.install(&[]), "the first install");
    let lock = sandbox.read("Gemfile.lock");
    let served = server.gems_served();
    assert_eq!(served, 1);

    // Again: nothing added, nothing removed, nothing fetched.
    let out = sandbox.install(&[]);
    assert_eq!(ok(&out, "the second install"), "");
    assert_eq!(stderr(&out), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert_eq!(server.gems_served(), served);

    // A second project with the same Spinel home links the stored gem.
    let other = sandbox.root.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("Gemfile"), sandbox.read("Gemfile")).unwrap();
    let out = sandbox
        .spinel()
        .current_dir(&other)
        .arg("install")
        .output()
        .unwrap();
    assert_eq!(
        ok(&out, "another project's install"),
        format!("+ rake {}\n", rake_version())
    );
    assert_eq!(server.gems_served(), served);
    assert_eq!(
        fs::read_to_string(other.join("Gemfile.lock")).unwrap(),
        lock
    );
}

#[test]
fn a_gem_taken_out_of_the_gemfile_is_removed() {
    let server = Server::start();
    let sandbox = Sandbox::new("removed");
    sandbox.copy("sinatra-lock", &server);
    ok(&sandbox.install(&[]), "the first install");
    let gemfile = sandbox.read("Gemfile").replace("gem \"webrick\"\n", "");
    sandbox.write("Gemfile", &gemfile);

    let out = sandbox.install(&[]);
    assert_eq!(ok(&out, "spinel install"), "- webrick 1.9.2\n");
    assert!(!sandbox.linked().contains(&"webrick-1.9.2".to_owned()));
    let lock = sandbox.read("Gemfile.lock");
    assert!(!lock.contains("webrick"), "{lock}");
}

#[test]
fn frozen_is_silent_when_the_lock_would_not_change() {
    let server = Server::start();
    let sandbox = Sandbox::new("frozen-silent");
    sandbox.copy("plain", &server);
    ok(&sandbox.install(&[]), "the first install");
    let lock = sandbox.read("Gemfile.lock");

    let out = sandbox.install(&["--frozen"]);
    assert_eq!(ok(&out, "spinel install --frozen"), "");
    assert_eq!(stderr(&out), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

    // From the lockfile alone, into an empty GEM_HOME, it installs.
    fs::remove_dir_all(sandbox.project().join(".spinel")).unwrap();
    let out = sandbox.install(&["--frozen"]);
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        format!("+ rake {}\n", rake_version())
    );
    assert_eq!(stderr(&out), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}

#[test]
fn frozen_errors_when_the_lock_would_change() {
    let server = Server::start();
    let sandbox = Sandbox::new("frozen-error");
    sandbox.copy("plain", &server);
    ok(&sandbox.install(&[]), "the first install");
    let gemfile = sandbox.read("Gemfile");
    let lock = sandbox.read("Gemfile.lock");
    let linked = sandbox.linked();

    // A gem added: Bundler 4.0.20's words, `bundle install` being
    // `spinel install`, and its status.
    sandbox.write("Gemfile", &format!("{gemfile}gem \"rack\", \"~> 3.0\"\n"));
    let out = sandbox.install(&["--frozen"]);
    assert_eq!(out.status.code(), Some(16), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "The dependencies in your gemfile changed, but the lockfile can't be updated \
         because frozen mode is set\n\nYou have added to the Gemfile:\n* rack (~> 3.0)\n\n\
         Run `spinel install` elsewhere and add the updated Gemfile.lock to version control.\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert_eq!(sandbox.linked(), linked);

    // A gem taken out.
    sandbox.write("Gemfile", &gemfile.replace("gem \"rake\"\n", ""));
    let out = sandbox.install(&["--frozen"]);
    assert_eq!(out.status.code(), Some(16), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "Some dependencies were deleted from your gemfile, but the lockfile can't be updated \
         because frozen mode is set\n\nYou have deleted from the Gemfile:\n* rake\n\n\
         Run `spinel install` elsewhere and add the updated Gemfile.lock to version control.\n"
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert_eq!(sandbox.linked(), linked);

    // A requirement the locked version no longer meets.
    sandbox.write(
        "Gemfile",
        &gemfile.replace("gem \"rake\"", "gem \"rake\", \"< 13\""),
    );
    let out = sandbox.install(&["--frozen"]);
    assert_eq!(out.status.code(), Some(16), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "The dependencies in your gemfile changed, but the lockfile can't be updated \
         because frozen mode is set\n\nYou have added to the Gemfile:\n* rake (< 13)\n\n\
         You have deleted from the Gemfile:\n* rake\n\n\
         Run `spinel install` elsewhere and add the updated Gemfile.lock to version control.\n"
    );
    assert_eq!(sandbox.read("Gemfile.lock"), lock);

    // No lockfile at all.
    sandbox.write("Gemfile", &gemfile);
    fs::remove_file(sandbox.project().join("Gemfile.lock")).unwrap();
    let out = sandbox.install(&["--frozen"]);
    assert_eq!(out.status.code(), Some(16), "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "Frozen mode is set, but there's no lockfile\n"
    );
    assert!(!sandbox.project().join("Gemfile.lock").exists());
}

#[test]
fn sinatra_lock_installs_from_its_lockfile() {
    let server = Server::start();
    let sandbox = Sandbox::new("sinatra-lock");
    sandbox.copy("sinatra-lock", &server);
    let lock = sandbox.read("Gemfile.lock");

    let out = sandbox.install(&["--frozen"]);
    assert_eq!(
        ok(&out, "spinel install --frozen"),
        "+ base64 0.3.0\n+ logger 1.7.0\n+ mustermann 3.1.1\n+ rack 3.2.8\n\
         + rack-protection 4.2.1\n+ rack-session 2.1.2\n+ rackup 2.3.1\n\
         + sinatra 4.2.1\n+ tilt 2.9.0\n+ webrick 1.9.2\n"
    );
    assert_eq!(stderr(&out), "");
    // Nothing changed, so the lockfile is Bundler's, byte for byte.
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
    assert_eq!(server.gems_served(), 10);
    assert_eq!(sandbox.linked().len(), 10);

    // The gems load through RubyGems from the GEM_HOME. (`webrick` is
    // left out: Spinel's standard library has one, which `require` finds
    // before it activates the gem.)
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            "require \"rack\"; require \"rackup\"; require \"tilt\"; require \"mustermann\"; \
             puts Rack.release, Gem.loaded_specs.values.map(&:full_name).sort.join(\",\")",
        ])
        .output()
        .unwrap();
    assert_eq!(
        ok(&out, "spinel run -e"),
        "3.2.8\nmustermann-3.1.1,rack-3.2.8,rackup-2.3.1,tilt-2.9.0\n"
    );
}

/// Kills the server the test started, however the test ends.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn sinatra_lock_serves_a_request() {
    let server = Server::start();
    let sandbox = Sandbox::new("sinatra-serve");
    sandbox.copy("sinatra-lock", &server);
    ok(&sandbox.install(&["--frozen"]), "spinel install --frozen");

    // A port nothing listens on: the kernel's, given back for the app.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    // ponytail: Spinel has no openssl yet, which rack-session's cookie
    // store requires as Sinatra loads it, so a stand-in for it goes on the
    // load path (`fixtures/install/openssl-standin/`). Upgrade: drop the
    // `-I` once openssl lands (#106).
    let standin = fixtures().join("install/openssl-standin");
    let mut child = Child(
        sandbox
            .spinel()
            .arg("run")
            .arg("-I")
            .arg(&standin)
            .arg("app.rb")
            .env("PORT", port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spinel runs"),
    );

    // The installed Sinatra, on WEBrick, answers once it has booted.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let response = loop {
        if let Ok(Some(status)) = child.0.try_wait() {
            let mut err = String::new();
            let _ = std::io::Read::read_to_string(child.0.stderr.as_mut().unwrap(), &mut err);
            panic!("the app exited ({status}) before it served:\n{err}");
        }
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(30)))
                .unwrap();
            write!(stream, "GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
            let mut response = String::new();
            std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
            break response;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the app did not listen on {port} in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert!(
        response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200"),
        "{response}"
    );
    assert!(
        response.ends_with("\r\n\r\nHello from Sinatra 4.2.1"),
        "{response}"
    );
}
