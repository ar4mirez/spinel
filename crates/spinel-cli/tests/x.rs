//! `spinel x` (#68, docs/cli.md, "`spinel x`"): a gem's executable run from
//! a GEM_HOME of its own under the Spinel home's `x/`, with the project in
//! the current directory left as it was. The gems are the real ones
//! `tests/fixtures/x/build.sh` builds with `gem build`, served with a compact
//! index on 127.0.0.1; nothing touches the network beyond it.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use md5::Md5;
use sha2::{Digest as _, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Default)]
struct State {
    files: HashMap<String, Vec<u8>>,
    seen: Vec<String>,
}

/// A gem source on 127.0.0.1 serving the fixture gems, with `/versions` and
/// `/info/<gem>` written from their checksums: `GET` of those files, nothing
/// more.
struct Server {
    url: String,
    state: Arc<Mutex<State>>,
}

/// Each fixture gem: name, version, its `/info` dependencies field.
const GEMS: [(&str, &str, &str); 4] = [
    ("greet", "1.0.0", ""),
    ("greet", "2.0.0", "shout:~> 1.0"),
    ("pair", "1.0.0", ""),
    ("shout", "1.0.0", ""),
];

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        let server = Server { url, state };
        server.publish();
        server
    }

    fn publish(&self) {
        let gems = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/x/gems");
        let mut infos: BTreeMap<&str, (Vec<&str>, Vec<u8>)> = BTreeMap::new();
        let mut state = self.state.lock().unwrap();
        for (name, version, deps) in GEMS {
            let file = format!("{name}-{version}.gem");
            let body = fs::read(gems.join(&file)).unwrap();
            let sha = hex(&Sha256::digest(&body));
            let (versions, info) = infos
                .entry(name)
                .or_insert_with(|| (Vec::new(), b"---\n".to_vec()));
            versions.push(version);
            info.extend(format!("{version} {deps}|checksum:{sha}\n").into_bytes());
            state.files.insert(format!("/gems/{file}"), body);
        }
        let mut list = b"created_at: 2026-01-01T00:00:00Z\n---\n".to_vec();
        for (name, (versions, info)) in infos {
            list.extend(
                format!(
                    "{name} {} {}\n",
                    versions.join(","),
                    hex(&Md5::digest(&info))
                )
                .into_bytes(),
            );
            state.files.insert(format!("/info/{name}"), info);
        }
        state.files.insert("/versions".to_owned(), list);
    }

    /// The `/gems/` requests since the last call, sorted.
    fn take_gem_requests(&self) -> Vec<String> {
        let mut gems: Vec<String> = std::mem::take(&mut self.state.lock().unwrap().seen)
            .into_iter()
            .filter(|p| p.starts_with("/gems/"))
            .collect();
        gems.sort();
        gems
    }
}

fn serve(stream: TcpStream, shared: &Mutex<State>) {
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
    let body = {
        let mut state = shared.lock().unwrap();
        state.seen.push(path.clone());
        state.files.get(&path).cloned()
    };
    let mut stream = stream;
    let _ = match body {
        None => stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 9\r\n\r\nnot found",
        ),
        Some(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream
                .write_all(head.as_bytes())
                .and_then(|()| stream.write_all(&body))
        }
    };
}

/// A project and a Spinel home of its own, under the test's scratch
/// directory. The project has a Gemfile, a lockfile and a `.spinel/` of its
/// own, none of which `spinel x` may read or touch.
struct Sandbox {
    root: PathBuf,
    server: Server,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("x").join(name);
        let _ = fs::remove_dir_all(&root);
        let project = root.join("project");
        fs::create_dir_all(project.join(".spinel/gem_home/bin")).unwrap();
        fs::write(
            project.join("Gemfile"),
            "source \"https://rubygems.org\"\ngem \"greet\", \"= 1.0.0\"\n",
        )
        .unwrap();
        fs::write(project.join("Gemfile.lock"), "not a lockfile\n").unwrap();
        fs::write(
            project.join(".spinel/gem_home/bin/greet"),
            "#!/usr/bin/env spinel\nputs \"the project's greet\"\n",
        )
        .unwrap();
        Sandbox {
            root,
            server: Server::start(),
        }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// `spinel x --source <server> <args>`, from the project.
    fn x(&self, args: &[&str]) -> Output {
        Command::new(SPINEL)
            .arg("x")
            .args(["--source", &self.server.url])
            .args(args)
            .current_dir(self.project())
            .env("SPINEL_HOME", self.home())
            .env_remove("GEM_HOME")
            .env_remove("GEM_PATH")
            .env_remove("RUBYOPT")
            .env_remove("BUNDLE_GEMFILE")
            .output()
            .expect("run spinel x")
    }

    /// Every path under the project with its contents, to compare before
    /// and after.
    fn project_tree(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let rel = path.strip_prefix(base).unwrap().to_owned();
                if path.is_dir() {
                    out.insert(rel, Vec::new());
                    walk(&path, base, out);
                } else {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut tree = BTreeMap::new();
        walk(&self.project(), &self.project(), &mut tree);
        tree
    }

    /// The GEM_HOMEs under the Spinel home's `x/`.
    fn x_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = fs::read_dir(self.home().join("x"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        dirs.sort();
        dirs
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The line of the fixture's output that starts with `key `.
fn field(out: &Output, key: &str) -> String {
    let stdout = text(&out.stdout);
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key} ")).map(str::to_owned))
        .unwrap_or_else(|| panic!("no `{key}` line in:\n{stdout}\n{}", text(&out.stderr)))
}

#[test]
fn runs_the_newest_version_and_reuses_its_gem_home() {
    let sandbox = Sandbox::new("newest");
    let before = sandbox.project_tree();

    let out = sandbox.x(&["greet", "hello", "two words", "--flag"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    // The newest greet, with its dependency activated.
    assert_eq!(text(&out.stdout).lines().next(), Some("GREET 2.0.0!"));
    assert_eq!(field(&out, "argv"), r#"["hello", "two words", "--flag"]"#);
    assert_eq!(field(&out, "pwd"), sandbox.project().display().to_string());
    // Its own GEM_HOME, under `x/`, and never the project's.
    let dirs = sandbox.x_dirs();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert!(dirs[0].starts_with("greet-2.0.0-"), "{dirs:?}");
    assert_eq!(
        field(&out, "gem_home"),
        sandbox
            .home()
            .join("x")
            .join(&dirs[0])
            .display()
            .to_string()
    );
    assert_eq!(text(&out.stderr), "+ greet 2.0.0\n+ shout 1.0.0\n");
    assert_eq!(
        sandbox.server.take_gem_requests(),
        ["/gems/greet-2.0.0.gem", "/gems/shout-1.0.0.gem"]
    );
    assert_eq!(sandbox.project_tree(), before, "the project changed");

    // The second run is a cache hit: nothing fetched, nothing linked.
    let gem_home = sandbox.home().join("x").join(&dirs[0]);
    let spec = gem_home.join("specifications/greet-2.0.0.gemspec");
    let modified = fs::metadata(&spec).unwrap().modified().unwrap();
    let again = sandbox.x(&["greet", "again"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert_eq!(text(&again.stdout).lines().next(), Some("GREET 2.0.0!"));
    assert_eq!(field(&again, "argv"), r#"["again"]"#);
    assert_eq!(text(&again.stderr), "");
    assert!(sandbox.server.take_gem_requests().is_empty());
    assert_eq!(sandbox.x_dirs(), dirs);
    assert_eq!(fs::metadata(&spec).unwrap().modified().unwrap(), modified);
    assert_eq!(sandbox.project_tree(), before, "the project changed");
}

#[test]
fn a_version_requirement_is_honored() {
    let sandbox = Sandbox::new("requirement");
    let out = sandbox.x(&["greet@1.0.0"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).lines().next(), Some("greet 1.0.0"));
    assert_eq!(text(&out.stderr), "+ greet 1.0.0\n");
    assert_eq!(
        sandbox.server.take_gem_requests(),
        ["/gems/greet-1.0.0.gem"]
    );
    let dirs = sandbox.x_dirs();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert!(dirs[0].starts_with("greet-1.0.0-"), "{dirs:?}");

    // Another requirement that resolves to the same gems is the same
    // GEM_HOME, and a cache hit.
    for requirement in ["greet@~> 1.0", "greet@< 2, >= 1"] {
        let out = sandbox.x(&[requirement]);
        assert!(out.status.success(), "{requirement}: {}", text(&out.stderr));
        assert_eq!(text(&out.stdout).lines().next(), Some("greet 1.0.0"));
        assert_eq!(text(&out.stderr), "", "{requirement}");
        assert!(sandbox.server.take_gem_requests().is_empty());
        assert_eq!(sandbox.x_dirs(), dirs, "{requirement}");
    }

    // The newest is another GEM_HOME beside it.
    let out = sandbox.x(&["greet@>= 2"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).lines().next(), Some("GREET 2.0.0!"));
    assert_eq!(sandbox.x_dirs().len(), 2);
}

#[test]
fn the_exit_status_and_arguments_pass_through() {
    let sandbox = Sandbox::new("status");
    let out = sandbox.x(&["greet@1", "--exit", "7"]);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    assert_eq!(field(&out, "argv"), r#"["--exit", "7"]"#);
    // Arguments that look like `spinel x`'s own options are the command's.
    let out = sandbox.x(&["greet@1", "--source", "nowhere", "--help", "-g"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        field(&out, "argv"),
        r#"["--source", "nowhere", "--help", "-g"]"#
    );
}

#[test]
fn an_unknown_gem_or_version_is_an_error() {
    let sandbox = Sandbox::new("unknown");
    let out = sandbox.x(&["nosuchgem", "arg"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, b"");
    assert_eq!(
        text(&out.stderr),
        format!(
            "spinel: x: could not find a valid gem 'nosuchgem' (>= 0) in {}\n",
            sandbox.server.url
        )
    );
    let out = sandbox.x(&["greet@3.0"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        text(&out.stderr),
        format!(
            "spinel: x: could not find a valid gem 'greet' (= 3.0) in {}\n",
            sandbox.server.url
        )
    );
    let out = sandbox.x(&["greet@not a version"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(sandbox.server.take_gem_requests().is_empty());
    assert!(sandbox.x_dirs().is_empty());
}

#[test]
fn the_executable_is_chosen_as_gem_exec_chooses_it() {
    let sandbox = Sandbox::new("choice");
    // A gem whose one executable is named otherwise runs it.
    let out = sandbox.x(&["shout", "hi", "there"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "HI THERE!\n");

    // Two executables, neither named like the gem, is gem exec's error.
    let out = sandbox.x(&["pair"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, b"");
    assert!(
        text(&out.stderr).contains(
            "spinel: x: ambiguous which executable from gem `pair` should be run: \
             the options are pair-a, pair-b"
        ),
        "{}",
        text(&out.stderr)
    );
    // `--gem` names the gem and the word the executable.
    let out = sandbox.x(&["--gem", "pair", "pair-b"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "pair-b\n");
    // An executable the gem does not have.
    let out = sandbox.x(&["--gem", "pair", "pair-c"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "spinel: x: failed to load executable `pair-c`, are you sure the gem `pair` contains it?\n"
    );
}
