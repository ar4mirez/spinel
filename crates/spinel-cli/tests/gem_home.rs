//! A project's GEM_HOME linked from the global store (#82,
//! docs/package-manager.md, "Global store"): `spinel fetch --gem-home`
//! fetches the gem-store fixtures from a local source and links them into
//! `.spinel/gem_home/`, which must be laid out as `gem install` lays one out
//! (`tests/fixtures/gem_home/`, made by RubyGems itself), and which RubyGems
//! and Bundler running on Spinel must read unchanged: `require`, a bin stub,
//! `Bundler.setup` and `bundle exec`. Nothing touches the network beyond
//! 127.0.0.1.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use md5::Md5;
use sha2::{Digest as _, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A gem source on 127.0.0.1 serving the gem-store fixtures and a compact
/// index that lists their checksums.
struct Server {
    url: String,
}

impl Server {
    fn start() -> Server {
        let gems = [
            ("alpha", "1.0.0", "alpha-1.0.0.gem"),
            ("beta", "2.1.0", "beta-2.1.0.gem"),
            (
                "gamma",
                "0.3.0-x86_64-linux",
                "gamma-0.3.0-x86_64-linux.gem",
            ),
        ];
        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        let mut versions = b"created_at: 2026-01-01T00:00:00Z\n---\n".to_vec();
        for (name, version, file) in gems {
            let body = fs::read(fixtures().join("gem_store/gems").join(file)).unwrap();
            let info = format!("---\n{version} |checksum:{}\n", hex(&Sha256::digest(&body)));
            versions.extend(format!("{name} 0 {}\n", hex(&Md5::digest(info.as_bytes()))).bytes());
            files.insert(format!("/info/{name}"), info.into_bytes());
            files.insert(format!("/gems/{file}"), body);
        }
        files.insert("/versions".to_owned(), versions);
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
    let mut stream = stream;
    let _ = match body {
        Some(body) => stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .and_then(|()| stream.write_all(&body)),
        None => stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
    };
    let _ = stream.flush();
}

/// A Spinel home and a project for one test, under Cargo's scratch directory
/// for tests, so nothing touches the real `~/.spinel`.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("gem-home-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("project")).unwrap();
        Sandbox(dir)
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    fn gem_home(&self) -> PathBuf {
        self.project().join(".spinel/gem_home")
    }

    fn store_gems(&self) -> PathBuf {
        self.home().join("store/gems")
    }

    /// `spinel` with this sandbox's home, run in the project, with the
    /// binary's directory on `PATH` so a bin stub's `#!/usr/bin/env spinel`
    /// finds it.
    fn spinel(&self) -> Command {
        let mut command = Command::new(SPINEL);
        let bin = Path::new(SPINEL).parent().unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin.to_owned()];
        paths.extend(std::env::split_paths(&path));
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.home())
            .env("PATH", std::env::join_paths(paths).unwrap());
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
            "RUBYOPT",
            "RUBYLIB",
        ] {
            command.env_remove(var);
        }
        command
    }

    fn fetch(&self, server: &Server, gems: &[&str]) -> Output {
        self.spinel()
            .args([
                "fetch",
                "--source",
                &server.url,
                "--gem-home",
                ".spinel/gem_home",
            ])
            .args(gems)
            .output()
            .expect("spinel runs")
    }

    /// Run `spinel fetch --gem-home`, expecting success, and return its lines
    /// from `gem home:` on.
    fn link(&self, server: &Server, gems: &[&str]) -> Vec<String> {
        let out = self.fetch(server, gems);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "spinel fetch failed\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let lines: Vec<String> = stdout
            .lines()
            .skip_while(|l| !l.starts_with("gem home: "))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            lines.first(),
            Some(&format!("gem home: {}", self.gem_home().display())),
            "{stdout}"
        );
        lines[1..].to_vec()
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.project().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `spinel run <file>` in the project: stdout, after checking it
    /// succeeded.
    fn run(&self, file: &str, args: &[&str]) -> String {
        let out = self
            .spinel()
            .arg("run")
            .arg(file)
            .args(args)
            .output()
            .expect("spinel runs");
        ok(&out, file)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ok(out: &Output, what: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{what} failed ({})\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

const ALL: [&str; 3] = ["alpha@1.0.0", "beta@2.1.0", "gamma@0.3.0-x86_64-linux"];

/// Every path under `dir`, as `build.sh` lists a GEM_HOME: directories with
/// a trailing `/`, through `gems/`' symlinks but not into them.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.push(format!("{rel}/"));
                if !rel.starts_with("gems/") {
                    walk(root, &path, out);
                }
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_gem_home_is_laid_out_as_gem_install_lays_it_out() {
    let server = Server::start();
    let sandbox = Sandbox::new("layout");
    assert_eq!(
        sandbox.link(&server, &ALL),
        [
            "alpha-1.0.0: linked",
            "beta-2.1.0: linked",
            "gamma-0.3.0-x86_64-linux: linked",
        ]
    );
    let gem_home = sandbox.gem_home();
    let expected_dir = fixtures().join("gem_home");
    let expected: Vec<String> = fs::read_to_string(expected_dir.join("tree.txt"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(tree(&gem_home), expected);

    // The files RubyGems writes itself, byte for byte, with its modes.
    for dir in ["specifications", "bin"] {
        for entry in fs::read_dir(expected_dir.join(dir)).unwrap() {
            let want = entry.unwrap().path();
            let got = gem_home.join(dir).join(want.file_name().unwrap());
            assert_eq!(
                fs::read_to_string(&got).unwrap(),
                fs::read_to_string(&want).unwrap(),
                "{}",
                got.display()
            );
            assert_eq!(mode(&got), mode(&want), "{}", got.display());
        }
    }

    // Each gem's directory is its store entry's files, shared.
    let entries: Vec<PathBuf> = fs::read_dir(sandbox.store_gems())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    for gem in ["alpha-1.0.0", "beta-2.1.0", "gamma-0.3.0-x86_64-linux"] {
        let link = gem_home.join("gems").join(gem);
        let target = fs::read_link(&link).unwrap();
        assert!(
            entries.iter().any(|e| e.join("gem") == target),
            "{gem} -> {}",
            target.display()
        );
    }
    assert_eq!(
        mode(&gem_home.join("gems/alpha-1.0.0/exe/alpha")),
        0o755,
        "an executable is executable, as gem install leaves it"
    );
    assert_eq!(
        fs::read_to_string(gem_home.join("gems/beta-2.1.0/lib/beta/current.rb")).unwrap(),
        "module Beta\n  VERSION = \"2.1.0\"\nend\n"
    );
}

#[test]
fn linking_again_writes_nothing_and_a_smaller_set_removes_the_rest() {
    let server = Server::start();
    let sandbox = Sandbox::new("relink");
    sandbox.link(&server, &ALL);
    let gem_home = sandbox.gem_home();
    let gemspec = gem_home.join("specifications/alpha-1.0.0.gemspec");
    let inode = fs::metadata(&gemspec).unwrap().ino();

    assert_eq!(
        sandbox.link(&server, &ALL),
        [
            "alpha-1.0.0: current",
            "beta-2.1.0: current",
            "gamma-0.3.0-x86_64-linux: current",
        ]
    );
    assert_eq!(
        fs::metadata(&gemspec).unwrap().ino(),
        inode,
        "not rewritten"
    );

    // What the link did not make stays: a gem `gem install` put there.
    fs::create_dir_all(gem_home.join("gems/own-1.0/lib")).unwrap();
    fs::write(gem_home.join("specifications/own-1.0.gemspec"), "# own\n").unwrap();
    fs::write(gem_home.join("bin/own"), "# own\n").unwrap();

    assert_eq!(
        sandbox.link(&server, &["beta@2.1.0"]),
        [
            "beta-2.1.0: current",
            "alpha-1.0.0: removed",
            "gamma-0.3.0-x86_64-linux: removed",
        ]
    );
    let mut expected = vec![
        "bin/",
        "bin/own",
        "build_info/",
        "cache/",
        "doc/",
        "extensions/",
        "gems/",
        "gems/beta-2.1.0/",
        "gems/own-1.0/",
        "plugins/",
        "specifications/",
        "specifications/beta-2.1.0.gemspec",
        "specifications/own-1.0.gemspec",
    ];
    expected.sort_unstable();
    assert_eq!(tree(&gem_home), expected);

    // Back again: the removed gems are linked from the store, fetching
    // nothing.
    assert_eq!(
        sandbox.link(&server, &ALL),
        [
            "alpha-1.0.0: linked",
            "beta-2.1.0: current",
            "gamma-0.3.0-x86_64-linux: linked",
        ]
    );
    assert!(gem_home.join("bin/alpha").is_file());

    // Under another Spinel home, its store's entries take the links' place,
    // and a gem no longer named goes, though it was linked from the first.
    let other = sandbox.0.join("other-home");
    let out = sandbox
        .spinel()
        .env("SPINEL_HOME", &other)
        .args([
            "fetch",
            "--source",
            &server.url,
            "--gem-home",
            ".spinel/gem_home",
        ])
        .args(["alpha@1.0.0", "beta@2.1.0"])
        .output()
        .unwrap();
    let stdout = ok(&out, "spinel fetch under another home");
    assert!(
        stdout.ends_with(
            "alpha-1.0.0: linked\nbeta-2.1.0: linked\ngamma-0.3.0-x86_64-linux: removed\n"
        ),
        "{stdout}"
    );
    let target = fs::read_link(gem_home.join("gems/alpha-1.0.0")).unwrap();
    assert!(
        target.starts_with(other.join("store/gems")),
        "{}",
        target.display()
    );
}

#[test]
fn a_gem_that_fails_to_fetch_leaves_the_gem_home_alone() {
    let server = Server::start();
    let sandbox = Sandbox::new("fails");
    sandbox.link(&server, &["alpha@1.0.0"]);
    let out = sandbox.fetch(&server, &["beta@2.1.0", "delta@1.0.0"]);
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("gem home:"), "{stdout}");
    let gems: Vec<String> = fs::read_dir(sandbox.gem_home().join("gems"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(gems, ["alpha-1.0.0"], "neither linked nor pruned");
}

#[test]
fn rubygems_on_spinel_activates_gems_and_runs_their_executables() {
    let server = Server::start();
    let sandbox = Sandbox::new("rubygems");
    sandbox.link(&server, &ALL);
    sandbox.write(
        "app.rb",
        r#"require "beta"
puts Beta::VERSION
puts Gem.loaded_specs.keys.sort.join(",")
spec = Gem::Specification.find_by_name("alpha")
puts spec.full_name, spec.gem_dir == File.join(Gem.dir, "gems", "alpha-1.0.0")
puts Gem::Specification.map(&:full_name).sort.join(",")
"#,
    );
    assert_eq!(
        sandbox.run("app.rb", &[]),
        "2.1.0\nalpha,beta\nalpha-1.0.0\ntrue\n\
         alpha-1.0.0,beta-2.1.0,gamma-0.3.0-x86_64-linux\n"
    );
    // The stub runs on Spinel: `#!/usr/bin/env spinel` finds it on PATH.
    let out = sandbox
        .spinel()
        .arg(sandbox.gem_home().join("bin/alpha"))
        .output()
        .unwrap();
    let stub = Command::new(sandbox.gem_home().join("bin/alpha"))
        .envs(
            sandbox
                .spinel()
                .get_envs()
                .filter_map(|(k, v)| Some((k, v?))),
        )
        .current_dir(sandbox.project())
        .output()
        .unwrap();
    assert_eq!(ok(&stub, "bin/alpha"), "1.0.0\n");
    assert_eq!(ok(&out, "spinel bin/alpha"), "1.0.0\n");
}

/// A Gemfile and the lockfile Bundler 4 writes for it, against `server`.
///
/// And Bundler's `bundle`: the `libexec/bundle` Spinel extracts beside its
/// stdlib (#71), found as `Bundler.setup` finds it to record in
/// `BUNDLE_BIN_PATH`. Returns the executable's path.
fn bundle(sandbox: &Sandbox, server: &Server) -> PathBuf {
    let out = sandbox
        .spinel()
        .args([
            "run",
            "-e",
            "require 'bundler'; print Bundler::SharedHelpers.bundle_bin_path",
        ])
        .output()
        .expect("spinel runs");
    let exe = PathBuf::from(ok(&out, "bundle_bin_path"));
    assert!(
        exe.ends_with("libexec/bundle") && exe.is_file(),
        "{}",
        exe.display()
    );
    sandbox.write(
        "Gemfile",
        &format!("source \"{}\"\n\ngem \"beta\"\n", server.url),
    );
    sandbox.write(
        "Gemfile.lock",
        &format!(
            "GEM
  remote: {}/
  specs:
    alpha (1.0.0)
    beta (2.1.0)
      alpha (~> 1.0)

PLATFORMS
  ruby

DEPENDENCIES
  beta

BUNDLED WITH
  4.0.16
",
            server.url
        ),
    );
    exe
}

#[test]
fn bundler_setup_on_spinel_reads_the_gem_home() {
    let server = Server::start();
    let sandbox = Sandbox::new("bundler-setup");
    sandbox.link(&server, &ALL);
    bundle(&sandbox, &server);
    sandbox.write(
        "app.rb",
        r#"require "bundler/setup"
require "beta"
puts Beta::VERSION
puts Bundler.load.specs.map(&:full_name).sort.join(",")
puts $LOAD_PATH.select { |p| p.start_with?(Gem.dir) }.map { |p| p.delete_prefix(Gem.dir) }.sort.join(",")
begin
  require "gamma"
  puts "gamma loaded"
rescue LoadError
  puts "gamma is not in the bundle"
end
"#,
    );
    assert_eq!(
        sandbox.run("app.rb", &[]),
        "2.1.0\nalpha-1.0.0,beta-2.1.0,bundler-4.0.16\n\
         /gems/alpha-1.0.0/lib,/gems/beta-2.1.0/lib\n\
         gamma is not in the bundle\n"
    );
}

#[test]
fn bundle_exec_on_spinel_accepts_the_gem_home() {
    let server = Server::start();
    let sandbox = Sandbox::new("bundle-exec");
    sandbox.link(&server, &ALL);
    let bundle = bundle(&sandbox, &server);
    let bundle = bundle.to_str().unwrap();
    sandbox.write(
        "check.rb",
        r#"puts Gem.loaded_specs.keys.sort.join(",")
puts ENV["BUNDLE_GEMFILE"] == File.expand_path("Gemfile")
require "alpha"
puts Alpha::VERSION
"#,
    );
    assert_eq!(
        sandbox.run(bundle, &["exec", "spinel", "check.rb"]),
        "alpha,beta,bundler\ntrue\n1.0.0\n"
    );
    // An executable of the bundle, found on PATH through the GEM_HOME's
    // `bin/`.
    assert_eq!(sandbox.run(bundle, &["exec", "alpha"]), "1.0.0\n");
}
