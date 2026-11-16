//! `spinel update` (#87, docs/package-manager.md, "11. `spinel update`")
//! against the gems `tests/fixtures/update/build.sh` builds with `gem
//! build`, served on 127.0.0.1 with a compact index written from their
//! checksums: rake 1.0.0 and 2.0.0, which depend on rake-dep and shared;
//! other 1.0.0 and 2.0.0, which depend on shared; and rake-dep and shared,
//! 1.0.0 and 2.0.0. The project starts from `fixtures/update/bundler/
//! installed.lock`, the lockfile Bundler 4.0.20 wrote for the Gemfile there
//! with only the 1.0.0 versions published, and every lockfile Spinel writes
//! is compared with the one Bundler wrote for the same update
//! (`fixtures/update/bundler.sh`), byte for byte.

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

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Each fixture gem: name, version, its `/info` dependencies field.
const GEMS: [(&str, &str, &str); 8] = [
    ("other", "1.0.0", "shared:>= 1.0"),
    ("other", "2.0.0", "shared:>= 1.0"),
    ("rake", "1.0.0", "rake-dep:>= 1.0,shared:>= 1.0"),
    ("rake", "2.0.0", "rake-dep:>= 1.0,shared:>= 1.0"),
    ("rake-dep", "1.0.0", ""),
    ("rake-dep", "2.0.0", ""),
    ("shared", "1.0.0", ""),
    ("shared", "2.0.0", ""),
];

/// A gem source on 127.0.0.1: `GET` of `/versions`, `/info/<gem>` and
/// `/gems/<file>`, nothing more.
struct Server {
    url: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let files = Arc::new(Mutex::new(HashMap::new()));
        let shared = Arc::clone(&files);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        Server { url, files }
    }

    /// Publish the gems `which` keeps, and only those.
    fn publish(&self, which: impl Fn(&str, &str) -> bool) {
        let gems = fixtures().join("gems");
        let mut infos: BTreeMap<&str, (Vec<&str>, Vec<u8>)> = BTreeMap::new();
        let mut files = self.files.lock().unwrap();
        files.clear();
        for (name, version, deps) in GEMS {
            if !which(name, version) {
                continue;
            }
            let file = format!("{name}-{version}.gem");
            let body = fs::read(gems.join(&file)).unwrap();
            let sha = hex(&Sha256::digest(&body));
            let (versions, info) = infos
                .entry(name)
                .or_insert_with(|| (Vec::new(), b"---\n".to_vec()));
            versions.push(version);
            info.extend(format!("{version} {deps}|checksum:{sha}\n").into_bytes());
            files.insert(format!("/gems/{file}"), body);
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
            files.insert(format!("/info/{name}"), info);
        }
        files.insert("/versions".to_owned(), list);
    }

    /// Stop serving `path`.
    fn drop_file(&self, path: &str) {
        self.files.lock().unwrap().remove(path);
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

/// A project and a Spinel home of its own, and the server.
struct Sandbox {
    root: PathBuf,
    server: Server,
}

impl Sandbox {
    /// The Gemfile, and with `installed` Bundler's lockfile for the 1.0.0
    /// versions, installed with only those published.
    fn new(name: &str, installed: bool) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("update")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        let sandbox = Sandbox {
            root,
            server: Server::start(),
        };
        let gemfile = fs::read_to_string(fixtures().join("bundler/Gemfile")).unwrap();
        fs::write(
            sandbox.project().join("Gemfile"),
            gemfile.replace("@SOURCE@", &sandbox.server.url),
        )
        .unwrap();
        sandbox.server.publish(|_, version| version == "1.0.0");
        if installed {
            fs::write(
                sandbox.project().join("Gemfile.lock"),
                sandbox.bundlers("installed.lock"),
            )
            .unwrap();
            let out = sandbox.spinel(&["install"]);
            ok(&out, "spinel install");
            // Nothing changed: Bundler's lockfile stays as it was.
            assert_eq!(sandbox.lock(), sandbox.bundlers("installed.lock"));
        }
        sandbox
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    /// The lockfile Bundler wrote, `fixtures/update/bundler/<name>`, for
    /// this server.
    fn bundlers(&self, name: &str) -> String {
        fs::read_to_string(fixtures().join("bundler").join(name))
            .unwrap()
            .replace("@SOURCE@", &self.server.url)
    }

    fn lock(&self) -> String {
        fs::read_to_string(self.project().join("Gemfile.lock")).unwrap()
    }

    fn lock_bytes(&self) -> Option<Vec<u8>> {
        fs::read(self.project().join("Gemfile.lock")).ok()
    }

    fn spinel(&self, args: &[&str]) -> Output {
        let mut command = Command::new(SPINEL);
        command
            .current_dir(self.project())
            .env("SPINEL_HOME", self.root.join("home"))
            .args(args);
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
        command.output().expect("spinel runs")
    }

    fn update(&self, args: &[&str]) -> Output {
        let mut all = vec!["update"];
        all.extend_from_slice(args);
        self.spinel(&all)
    }

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

/// Its standard output, after checking it exited 0 and said nothing on
/// standard error.
fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "{what} ({:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Its standard error, after checking it exited with `code`.
fn failed(out: &Output, code: i32, what: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(code),
        "{what}\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    stderr
}

#[test]
fn update_rake_moves_only_rake_and_its_dependencies() {
    let sandbox = Sandbox::new("named", true);
    assert_eq!(
        sandbox.linked(),
        [
            "other-1.0.0",
            "rake-1.0.0",
            "rake-dep-1.0.0",
            "shared-1.0.0"
        ]
    );

    // Newer versions of every gem; rake, rake-dep and shared move, other
    // stays at 1.0.0, as `bundle update rake` leaves it.
    sandbox.server.publish(|_, _| true);
    let out = sandbox.update(&["rake"]);
    assert_eq!(
        ok(&out, "spinel update rake"),
        "+ rake 2.0.0\n\
         - rake 1.0.0\n\
         + rake-dep 2.0.0\n\
         - rake-dep 1.0.0\n\
         + shared 2.0.0\n\
         - shared 1.0.0\n"
    );
    assert_eq!(sandbox.lock(), sandbox.bundlers("rake.lock"));
    assert_eq!(
        sandbox.linked(),
        [
            "other-1.0.0",
            "rake-2.0.0",
            "rake-dep-2.0.0",
            "shared-2.0.0"
        ]
    );

    // Again, with nothing newer: Bundler's warning, and nothing written.
    let out = sandbox.update(&["rake"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "Bundler attempted to update rake but its version stayed the same\n"
    );
    assert_eq!(sandbox.lock(), sandbox.bundlers("rake.lock"));
}

#[test]
fn update_with_no_gems_moves_everything() {
    for (name, args) in [("all", &[][..]), ("all-flag", &["--all"][..])] {
        let sandbox = Sandbox::new(name, true);
        sandbox.server.publish(|_, _| true);
        let out = sandbox.update(args);
        assert_eq!(
            ok(&out, &format!("spinel update {args:?}")),
            "+ other 2.0.0\n\
             - other 1.0.0\n\
             + rake 2.0.0\n\
             - rake 1.0.0\n\
             + rake-dep 2.0.0\n\
             - rake-dep 1.0.0\n\
             + shared 2.0.0\n\
             - shared 1.0.0\n"
        );
        assert_eq!(sandbox.lock(), sandbox.bundlers("all.lock"));
        assert_eq!(
            sandbox.linked(),
            [
                "other-2.0.0",
                "rake-2.0.0",
                "rake-dep-2.0.0",
                "shared-2.0.0"
            ]
        );
    }
}

#[test]
fn update_with_no_lockfile_installs() {
    // Nothing to unlock: the newest of everything, as `spinel install`.
    let sandbox = Sandbox::new("no-lockfile", false);
    sandbox.server.publish(|_, _| true);
    ok(&sandbox.update(&[]), "spinel update");
    assert_eq!(
        sandbox.linked(),
        [
            "other-2.0.0",
            "rake-2.0.0",
            "rake-dep-2.0.0",
            "shared-2.0.0"
        ]
    );

    // Named gems need a lockfile to unlock them in.
    let sandbox = Sandbox::new("no-lockfile-named", false);
    let out = sandbox.update(&["rake"]);
    assert_eq!(
        failed(&out, 22, "spinel update rake"),
        "This Bundle hasn't been installed yet. \
         Run `spinel install` to update and install the bundled gems.\n"
    );
    assert_eq!(sandbox.lock_bytes(), None);
}

#[test]
fn an_unknown_gem_is_bundlers_error_and_writes_nothing() {
    let sandbox = Sandbox::new("unknown", true);
    sandbox.server.publish(|_, _| true);
    let before = sandbox.lock_bytes();

    // Bundler's words and status for the same lockfile (`nope.txt`).
    let bundler = fs::read_to_string(fixtures().join("bundler/nope.txt")).unwrap();
    let (message, status) = bundler.rsplit_once("exit ").unwrap();
    let out = sandbox.update(&["nope"]);
    assert_eq!(
        failed(&out, status.trim().parse().unwrap(), "spinel update nope"),
        message
    );
    // A near miss is suggested, as `DidYouMean` suggests it.
    let out = sandbox.update(&["rake", "rakedep"]);
    assert_eq!(
        failed(&out, 7, "spinel update rake rakedep"),
        "Could not find gem 'rakedep'.\nDid you mean 'rake-dep'?\n"
    );
    assert_eq!(sandbox.lock_bytes(), before);
    assert_eq!(
        sandbox.linked(),
        [
            "other-1.0.0",
            "rake-1.0.0",
            "rake-dep-1.0.0",
            "shared-1.0.0"
        ]
    );
}

#[test]
fn options_not_supported_yet_are_refused_and_write_nothing() {
    let sandbox = Sandbox::new("options", true);
    sandbox.server.publish(|_, _| true);
    let before = sandbox.lock_bytes();
    for (args, option) in [
        (&["--conservative", "rake"][..], "--conservative"),
        (&["--patch"][..], "--patch"),
        (&["--minor", "rake"][..], "--minor"),
        (&["--major"][..], "--major"),
        (&["--group", "default"][..], "--group"),
    ] {
        let out = sandbox.update(args);
        assert_eq!(
            failed(&out, 15, &format!("spinel update {args:?}")),
            format!("`{option}` is not supported by `spinel update` yet.\n")
        );
    }
    let out = sandbox.update(&["--all", "rake"]);
    assert_eq!(
        failed(&out, 15, "spinel update --all rake"),
        "Cannot specify --all along with specific options.\n"
    );
    assert_eq!(sandbox.lock_bytes(), before);
}

#[test]
fn a_failed_update_leaves_the_lockfile_as_it_was() {
    let sandbox = Sandbox::new("failed", true);
    sandbox.server.publish(|_, _| true);
    sandbox.server.drop_file("/gems/rake-dep-2.0.0.gem");
    let before = sandbox.lock_bytes();
    let out = sandbox.update(&["rake"]);
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(sandbox.lock_bytes(), before);
    assert_eq!(
        sandbox.linked(),
        [
            "other-1.0.0",
            "rake-1.0.0",
            "rake-dep-1.0.0",
            "shared-1.0.0"
        ]
    );
}
