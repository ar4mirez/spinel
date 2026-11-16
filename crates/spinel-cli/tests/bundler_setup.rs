//! Bundler running on Spinel, and plain Bundler beside it (#71, docs/cli.md,
//! "Bundler running on Spinel"): `fixtures/bundler-setup` installed by
//! `spinel install --frozen` from its lockfile, against rubygems.org's own `.gem`s
//! served on 127.0.0.1 (`fixtures/install/`, as `tests/install.rs` serves
//! them), then its `app.rb` booted through `require "bundler/setup"` under
//! `spinel run`, with exactly the locked gems on the load path. Opt-in, with
//! `SPINEL_CRUBY_BUNDLER` set and CRuby 4.0's `bundle` on `PATH`, CRuby's own
//! `bundle check` and `bundle exec ruby app.rb` in the same project, which
//! must find the same gems through the `.bundle/config` `spinel install`
//! wrote. And `fixtures/plain` (rake) installed, whose executable each
//! engine's `bundle exec rake` runs on that engine. Nothing touches the
//! network beyond 127.0.0.1.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const RUBYGEMS: &str = "https://rubygems.org";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// rubygems.org as `fixtures/install/` has it: `/versions` and
/// `/info/<gem>` from its `index/`, `/gems/<file>` from its `gems/`.
fn serve_rubygems() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    url
}

fn serve(stream: TcpStream) {
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

/// `fixtures/<fixture>` copied into a project of its own, rubygems.org
/// replaced by the local server, with a Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(fixture: &str, name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("bundler-setup")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        let server = serve_rubygems();
        for entry in fs::read_dir(fixtures().join(fixture)).unwrap().flatten() {
            let text = fs::read_to_string(entry.path()).unwrap();
            fs::write(
                root.join("project").join(entry.file_name()),
                text.replace(RUBYGEMS, &server),
            )
            .unwrap();
        }
        Sandbox { root }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    /// `program`, run in the project with nothing of the environment's
    /// gems, Ruby options or Bundler settings.
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.root.join("home"));
        for (var, _) in std::env::vars_os() {
            let name = var.to_string_lossy();
            if name.starts_with("BUNDLE_")
                || name.starts_with("BUNDLER_")
                || name.starts_with("GEM_")
                || name.to_ascii_lowercase().ends_with("_proxy")
                || ["RUBYOPT", "RUBYLIB"].contains(&name.as_ref())
            {
                command.env_remove(&var);
            }
        }
        command
    }

    fn spinel(&self, args: &[&str]) -> Output {
        self.command(SPINEL)
            .args(args)
            .output()
            .expect("spinel runs")
    }

    /// CRuby's `bundle`, which `SPINEL_CRUBY_BUNDLER` says is on `PATH`,
    /// with its own settings kept out of the user's home.
    fn cruby_bundle(&self, args: &[&str]) -> Output {
        self.command("bundle")
            .args(args)
            .env("BUNDLE_USER_HOME", self.root.join("bundle-user-home"))
            .output()
            .expect("CRuby's bundle runs")
    }

    /// `spinel install --frozen`, expecting it to install the lockfile as
    /// it is. Frozen, so the lockfile is the fixture's on any platform: one
    /// whose platform it does not list would otherwise add it.
    fn install(&self) {
        let lock = self.read("Gemfile.lock");
        let out = self.spinel(&["install", "--frozen"]);
        assert_eq!(
            ok(&out, "spinel install --frozen"),
            "+ base64 0.3.0\n+ rack 3.2.8\n+ rack-session 2.1.2\n"
        );
        assert_eq!(self.read("Gemfile.lock"), lock, "the lockfile is kept");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Its standard output, after checking it exited 0.
fn ok(out: &Output, what: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{what} failed ({:?})\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// What `app.rb` prints, booted by a Bundler running on `engine`: the same
/// gems either way, read through the GEM_HOME's view for that engine.
fn app_output(engine: &str) -> String {
    format!(
        "visit 1\nvisit 2\nrack 3.2.8, rack-session 2.1.2\n\
         base64-0.3.0 rack-3.2.8 rack-session-2.1.2\n\
         base64 bundler rack rack-session\n\
         .spinel/gem_home/{engine}/4.0.0\n"
    )
}

#[test]
fn bundler_setup_boots_the_app_spinel_install_installed() {
    let sandbox = Sandbox::new("bundler-setup", "spinel");
    sandbox.install();
    // `bundle config set --local path .spinel/gem_home`'s file.
    assert_eq!(
        sandbox.read(".bundle/config"),
        "---\nBUNDLE_PATH: \".spinel/gem_home\"\n"
    );
    // Where each engine's Bundler reads that path: the GEM_HOME itself,
    // but for CRuby's `bin/` (`gem_executables_run_on_spinel_...`).
    let gem_home = sandbox.project().join(".spinel/gem_home");
    for engine in ["ruby", "spinel"] {
        let scope = gem_home.join(engine).join("4.0.0");
        for sub in ["gems", "specifications", "bin", "extensions"] {
            if engine == "ruby" && sub == "bin" {
                assert!(scope.join(sub).is_dir() && !scope.join(sub).is_symlink());
                continue;
            }
            assert_eq!(
                fs::read_link(scope.join(sub)).unwrap(),
                Path::new("../..").join(sub),
                "{engine}/4.0.0/{sub}"
            );
        }
    }
    let out = sandbox.spinel(&["run", "app.rb"]);
    assert_eq!(ok(&out, "spinel run app.rb"), app_output("spinel"));
    // A second install has nothing to do.
    assert_eq!(
        ok(&sandbox.spinel(&["install", "--frozen"]), "spinel install"),
        ""
    );
}

/// CRuby's `bundle`, which `SPINEL_CRUBY_BUNDLER` says is on `PATH`, for
/// CRuby 4.0: its `RbConfig::CONFIG["ruby_version"]` is the `4.0.0` the
/// views are made for.
#[test]
fn plain_bundle_exec_resolves_the_same_gems() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!("skipped: set SPINEL_CRUBY_BUNDLER to run CRuby's bundle against the project");
        return;
    }
    let sandbox = Sandbox::new("bundler-setup", "cruby");
    sandbox.install();
    let check = sandbox.cruby_bundle(&["check"]);
    assert_eq!(
        ok(&check, "bundle check"),
        "The Gemfile's dependencies are satisfied\n"
    );
    let exec = sandbox.cruby_bundle(&["exec", "ruby", "app.rb"]);
    assert_eq!(ok(&exec, "bundle exec ruby app.rb"), app_output("ruby"));
}

/// `fixtures/plain` installed: rake, which has an executable.
fn install_rake(name: &str) -> Sandbox {
    let sandbox = Sandbox::new("plain", name);
    let out = sandbox.spinel(&["install"]);
    assert_eq!(ok(&out, "spinel install"), "+ rake 13.4.2\n");
    sandbox
}

/// Bundler running on Spinel execs rake's stub from `spinel/4.0.0/bin`,
/// the GEM_HOME's own, which runs it on Spinel.
#[test]
fn gem_executables_run_on_spinel_under_bundler_on_spinel() {
    let sandbox = install_rake("rake-spinel");
    let out = sandbox.spinel(&[
        "run",
        "-e",
        "require 'bundler'; print Bundler::SharedHelpers.bundle_bin_path",
    ]);
    let bundle = ok(&out, "bundle_bin_path");
    let spinel_dir = Path::new(SPINEL).parent().unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(spinel_dir.to_owned()).chain(std::env::split_paths(&path)),
    )
    .unwrap();
    let exec = |args: &[&str]| {
        let out = sandbox
            .command(SPINEL)
            .env("PATH", &path)
            .args(["run", &bundle, "exec"])
            .args(args)
            .output()
            .expect("spinel runs");
        ok(&out, &format!("bundle exec {}", args.join(" ")))
    };
    assert_eq!(
        exec(&["rake", "-e", "puts RUBY_ENGINE, Rake::VERSION"]),
        "spinel\n13.4.2\n"
    );
}

/// CRuby's plain `bundle exec` execs rake's stub from `ruby/4.0.0/bin`,
/// the one CRuby's RubyGems writes, which runs it on CRuby.
#[test]
fn plain_bundle_exec_runs_gem_executables_on_cruby() {
    if std::env::var_os("SPINEL_CRUBY_BUNDLER").is_none() {
        eprintln!("skipped: set SPINEL_CRUBY_BUNDLER to run CRuby's bundle against the project");
        return;
    }
    let sandbox = install_rake("rake-cruby");
    let exec = |args: &[&str]| {
        let out = sandbox.cruby_bundle(&[&["exec"], args].concat());
        ok(&out, &format!("bundle exec {}", args.join(" ")))
    };
    assert_eq!(exec(&["rake", "--version"]), "rake, version 13.4.2\n");
    assert_eq!(exec(&["rake", "-e", "puts RUBY_ENGINE"]), "ruby\n");
    // Byte for byte the stub `gem install` writes on CRuby, with the
    // `--env-shebang` Bundler installs with.
    let install_dir = sandbox.root.join("gem-install");
    let out = sandbox
        .command("gem")
        .args([
            "install",
            "--local",
            "--env-shebang",
            "--no-document",
            "--install-dir",
        ])
        .arg(&install_dir)
        .arg(fixtures().join("install/gems/rake-13.4.2.gem"))
        .output()
        .expect("CRuby's gem runs");
    ok(&out, "gem install rake");
    assert_eq!(
        sandbox.read(".spinel/gem_home/ruby/4.0.0/bin/rake"),
        fs::read_to_string(install_dir.join("bin/rake")).unwrap()
    );
}
