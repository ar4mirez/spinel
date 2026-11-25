//! A gem with a CRuby C extension and no Spinel-native substitute (#898,
//! docs/package-manager.md, section 6, step 4): Spinel does not build the
//! extension, so RubyGems would never activate the gem, and `spinel install`
//! and `spinel add` fail at once, naming every such gem and linking the
//! Spinel-native gems list, rather than install it and leave `require
//! "bundler/setup"` to fail later. Nothing is written: not the GEM_HOME, the
//! Gemfile or the lockfile. A gem with a C extension that has a substitute
//! (`SPINEL_SUBSTITUTES`, as the built-in list owns no name yet) still
//! installs and loads.
//!
//! The source is a server on 127.0.0.1 that serves
//! `fixtures/unbuilt_extension/`'s index and `.gem`s (`build.rb` makes them),
//! so nothing touches the network.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const LIST_URL: &str =
    "https://github.com/ar4mirez/spinel/blob/main/docs/package-manager.md#6-native-extensions";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unbuilt_extension")
}

/// The fixture's source, `/versions` and `/info/<gem>` from `index/`,
/// `/gems/<file>` from `gems/`: its URL.
fn start_server() -> String {
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
    url: String,
}

impl Sandbox {
    /// A project whose Gemfile lists `gems` from the fixture's source.
    fn new(name: &str, gems: &[&str]) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("unbuilt_extension")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        let sandbox = Sandbox {
            root,
            url: start_server(),
        };
        sandbox.write_gemfile(gems);
        sandbox
    }

    fn write_gemfile(&self, gems: &[&str]) {
        let mut text = format!("source \"{}\"\n\n", self.url);
        for gem in gems {
            text.push_str(&format!("gem \"{gem}\"\n"));
        }
        fs::write(self.project().join("Gemfile"), text).unwrap();
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.project().join(name)).ok()
    }

    /// The names in the GEM_HOME's `gems/`, sorted; none when it is not
    /// there.
    fn linked(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.project().join(".spinel/gem_home/gems"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `spinel ARGS` in the project, with nothing of the shell's that would
    /// change where gems come from or go, and `substitutes` as
    /// `SPINEL_SUBSTITUTES` when given.
    fn spinel(&self, args: &[&str], substitutes: Option<&str>) -> Output {
        let mut command = Command::new(SPINEL);
        command
            .args(args)
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
        if let Some(names) = substitutes {
            command.env("SPINEL_SUBSTITUTES", names);
        }
        command.output().expect("spinel runs")
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

/// What `command` prints, exit 1, for the gems given.
fn refused(command: &str, gems: &[&str]) -> String {
    let mut text = String::new();
    for gem in gems {
        text.push_str(&format!(
            "spinel: {command}: {gem} has a C extension, which Spinel does not build, and no Spinel-native substitute\n"
        ));
    }
    text.push_str(&format!(
        "spinel: {command}: the Spinel-native gems list: {LIST_URL}\n"
    ));
    text
}

fn assert_refused(out: &Output, command: &str, gems: &[&str]) {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(String::from_utf8_lossy(&out.stderr), refused(command, gems));
}

#[test]
fn install_fails_naming_every_gem_with_an_unbuilt_extension() {
    let sandbox = Sandbox::new("install", &["plain", "ratchet", "cog"]);
    let out = sandbox.spinel(&["install"], None);
    assert_refused(&out, "install", &["cog-1.0.0", "ratchet-1.0.0"]);
    // Nothing half-installed: no lockfile, nothing linked, no Bundler
    // settings pointing at a GEM_HOME.
    assert_eq!(sandbox.read("Gemfile.lock"), None);
    assert_eq!(sandbox.linked(), [""; 0]);
    assert_eq!(sandbox.read(".bundle/config"), None);
}

#[test]
fn add_fails_and_leaves_the_project_as_it_was() {
    let sandbox = Sandbox::new("add", &["plain"]);
    ok(&sandbox.spinel(&["install"], None), "spinel install");
    let gemfile = sandbox.read("Gemfile").unwrap();
    let lock = sandbox.read("Gemfile.lock").unwrap();
    assert_eq!(sandbox.linked(), ["plain-1.0.0"]);

    let out = sandbox.spinel(&["add", "cog"], None);
    assert_refused(&out, "add", &["cog-1.0.0"]);
    assert_eq!(sandbox.read("Gemfile").unwrap(), gemfile);
    assert_eq!(sandbox.read("Gemfile.lock").unwrap(), lock);
    assert_eq!(sandbox.linked(), ["plain-1.0.0"]);

    // The project still loads as it did.
    let out = sandbox.spinel(
        &[
            "run",
            "-e",
            r#"require "bundler/setup"; require "plain"; puts Plain::IMPL"#,
        ],
        None,
    );
    assert_eq!(ok(&out, "spinel run -e"), "plain 1.0.0\n");
}

#[test]
fn a_gem_with_a_substitute_still_installs_and_loads() {
    let sandbox = Sandbox::new("substituted", &["plain", "sprocket"]);
    // Off the list, sprocket's extension is unbuilt: refused.
    let out = sandbox.spinel(&["install"], None);
    assert_refused(&out, "install", &["sprocket-1.0.0"]);

    // On it, its substitute stands in, and the project loads.
    let out = sandbox.spinel(&["install"], Some("sprocket"));
    assert_eq!(
        ok(&out, "spinel install"),
        "+ plain 1.0.0\n+ sprocket 1.0.0\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(sandbox.linked(), ["plain-1.0.0", "sprocket-1.0.0"]);
    let substitutions = sandbox.read(".spinel/substitutions.json").unwrap();
    assert!(
        substitutions.contains("\"substitute\": \"sprocket-spinel\""),
        "{substitutions}"
    );
    let out = sandbox.spinel(
        &[
            "run",
            "-e",
            r#"require "bundler/setup"; require "plain"; require "sprocket"; puts Plain::IMPL, Sprocket::IMPL"#,
        ],
        None,
    );
    assert_eq!(
        ok(&out, "spinel run -e"),
        "plain 1.0.0\nsprocket-spinel 1.0.0.1\n"
    );
}
