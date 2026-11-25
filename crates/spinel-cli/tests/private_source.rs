//! Private gem sources (#89, docs/package-manager.md, "Index client"):
//! `spinel install` against gem servers on 127.0.0.1 that `fixtures/
//! private_source/` holds (`snapshot.rb` built them from the real `.gem`s in
//! `fixtures/install/gems/`): a compact index behind HTTP Basic credentials,
//! taken from the source's URL, the environment or `.bundle/config`; a
//! server with no compact index, whose legacy `specs.4.8.gz` is read
//! instead; and a public global source beside a private one scoped to one
//! gem. Each lockfile is the one Bundler 4.0.20 wrote for the same Gemfile
//! (`fixtures/private_source/bundler.sh`, `expected/`), byte for byte; a
//! refused credential is Bundler's message and exit status; and the
//! credential is in neither the lockfile nor anything printed, nor sent
//! anywhere but its own source.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");
const SECRET: &str = "s3cret-pw";

/// Each request's path and `Authorization` header.
type Requests = Arc<Mutex<Vec<(String, Option<String>)>>>;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[derive(Clone, Copy, PartialEq)]
enum Index {
    /// `/versions` and `/info/<gem>`.
    Compact,
    /// A 404 for those, and `specs.4.8.gz` and `quick/Marshal.4.8/`.
    Legacy,
}

/// A gem server on 127.0.0.1. With `auth`, a request without that Basic
/// credential is a 401. Every request's path and `Authorization` header
/// are kept.
struct Server {
    port: u16,
    requests: Requests,
}

impl Server {
    fn start(index: Index, auth: Option<&str>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let expected = auth.map(|a| format!("Basic {}", base64(a.as_bytes())));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (seen, expected) = (Arc::clone(&seen), expected.clone());
                std::thread::spawn(move || serve(stream, index, expected.as_deref(), &seen));
            }
        });
        Server { port, requests }
    }

    /// The source on this server, under `host`, with `userinfo` if given.
    fn url(&self, host: &str, userinfo: Option<&str>) -> String {
        match userinfo {
            Some(userinfo) => format!("http://{userinfo}@{host}:{}", self.port),
            None => format!("http://{host}:{}", self.port),
        }
    }

    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn authorizations(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, a)| a.clone())
            .collect()
    }
}

fn serve(
    stream: TcpStream,
    index: Index,
    expected: Option<&str>,
    seen: &Mutex<Vec<(String, Option<String>)>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut authorization = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("authorization")
        {
            authorization = Some(value.trim().to_owned());
        }
    }
    seen.lock()
        .unwrap()
        .push((path.clone(), authorization.clone()));
    let root = fixtures().join("private_source");
    let (status, body) = if expected.is_some() && authorization.as_deref() != expected {
        ("401 Unauthorized", b"unauthorized".to_vec())
    } else {
        let file = path
            .strip_prefix('/')
            .filter(|p| !p.split('/').any(|part| part == ".." || part.is_empty()))
            .and_then(|p| match p.strip_prefix("gems/") {
                Some(gem) => fs::read(fixtures().join("install/gems").join(gem)).ok(),
                None => match index {
                    Index::Compact => fs::read(root.join("compact").join(p)).ok(),
                    Index::Legacy => fs::read(root.join("legacy").join(p)).ok(),
                },
            });
        match file {
            Some(body) => ("200 OK", body),
            None => ("404 Not Found", b"not found".to_vec()),
        }
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&body);
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A project and a Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("private_source")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        fs::create_dir_all(root.join("user")).unwrap();
        Sandbox { root }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.project().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    /// `spinel install`, with nothing of the environment's gems, proxies or
    /// Bundler settings, and `env` added.
    fn install(&self, env: &[(&str, &str)]) -> Output {
        self.install_with(&[], env)
    }

    /// [`Sandbox::install`] with `args` after `install`.
    fn install_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(SPINEL);
        command
            .arg("install")
            .args(args)
            .current_dir(self.project())
            .env("SPINEL_HOME", self.root.join("home"))
            .env("HOME", self.root.join("user"));
        for (var, _) in std::env::vars() {
            if var.starts_with("BUNDLE_")
                || var.to_ascii_lowercase().ends_with("_proxy")
                || ["GEM_HOME", "GEM_PATH", "RUBYOPT", "RUBYLIB"].contains(&var.as_str())
            {
                command.env_remove(var);
            }
        }
        command.envs(env.iter().copied());
        command.output().expect("spinel runs")
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

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed ({:?})\n{}",
        out.status,
        text(out)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `expected/<name>`, its sources put back.
fn expected(name: &str, sources: &[(&str, String)]) -> String {
    let mut text =
        fs::read_to_string(fixtures().join("private_source/expected").join(name)).unwrap();
    for (placeholder, url) in sources {
        text = text.replace(placeholder, url);
    }
    // Bundler ran on x86_64-linux; its `PLATFORMS` names the local
    // platform beside `ruby`, sorted, so name this host's instead.
    let mut platforms = ["ruby".to_owned(), local_platform()];
    platforms.sort();
    text.replace(
        "PLATFORMS\n  ruby\n  x86_64-linux\n",
        &format!("PLATFORMS\n  {}\n  {}\n", platforms[0], platforms[1]),
    )
}

/// This host as `Gem::Platform.local` spells it in a lockfile.
fn local_platform() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "linux" if cfg!(target_env = "musl") => format!("{arch}-linux-musl"),
        "linux" => format!("{arch}-linux"),
        "macos" => {
            let cpu = if arch == "aarch64" { "arm64" } else { arch };
            let out = Command::new("uname").arg("-r").output().unwrap();
            let release = String::from_utf8(out.stdout).unwrap();
            let major = release.trim().split('.').next().unwrap().to_owned();
            format!("{cpu}-darwin-{major}")
        }
        os => panic!("no lockfile platform for {arch}-{os}"),
    }
}

/// Neither the lockfile nor the output has the credential.
fn no_secret(sandbox: &Sandbox, out: &Output) {
    assert!(
        !text(out).contains(SECRET),
        "printed the credential:\n{}",
        text(out)
    );
    if let Ok(lock) = fs::read_to_string(sandbox.project().join("Gemfile.lock")) {
        assert!(!lock.contains(SECRET), "locked the credential:\n{lock}");
    }
}

const SESSION: [&str; 3] = ["base64-0.3.0", "rack-3.2.8", "rack-session-2.1.2"];

#[test]
fn a_private_compact_index_resolves_and_installs_with_the_credential_in_its_url() {
    let server = Server::start(Index::Compact, Some(&format!("user:{SECRET}")));
    let sandbox = Sandbox::new("url");
    let url = server.url("127.0.0.1", Some(&format!("user:{SECRET}")));
    sandbox.write(
        "Gemfile",
        &format!("source {url:?}\ngem \"rack-session\"\n"),
    );
    let out = sandbox.install(&[]);
    let printed = ok(&out, "install");
    no_secret(&sandbox, &out);
    assert_eq!(
        printed,
        "+ base64 0.3.0\n+ rack 3.2.8\n+ rack-session 2.1.2\n"
    );
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        expected(
            "compact.lock",
            &[("@SOURCE@", server.url("127.0.0.1", None))]
        )
    );
    assert_eq!(sandbox.linked(), SESSION);
    assert!(server.paths().contains(&"/versions".to_owned()));

    // Installed, it is left alone.
    let out = sandbox.install(&[]);
    assert_eq!(ok(&out, "second install"), "Nothing to change\n");
    no_secret(&sandbox, &out);
}

#[test]
fn credentials_come_from_the_environment_or_bundle_config_by_host() {
    let server = Server::start(Index::Compact, Some(&format!("user:{SECRET}")));
    let url = server.url("127.0.0.1", None);
    let gemfile = format!("source {url:?}\ngem \"rack-session\"\n");
    let lock = expected("compact.lock", &[("@SOURCE@", url.clone())]);

    let env = Sandbox::new("env");
    env.write("Gemfile", &gemfile);
    let out = env.install(&[("BUNDLE_127__0__0__1", &format!("user:{SECRET}"))]);
    ok(&out, "install with BUNDLE_127__0__0__1");
    no_secret(&env, &out);
    assert_eq!(env.read("Gemfile.lock"), lock);

    let config = Sandbox::new("config");
    config.write("Gemfile", &gemfile);
    config.write(
        ".bundle/config",
        &format!("---\nBUNDLE_127__0__0__1: \"user:{SECRET}\"\n"),
    );
    let out = config.install(&[]);
    ok(&out, "install with .bundle/config");
    no_secret(&config, &out);
    assert_eq!(config.read("Gemfile.lock"), lock);
    // The setting is kept beside the one install writes.
    assert!(
        config
            .read(".bundle/config")
            .contains("BUNDLE_127__0__0__1")
    );
}

#[test]
fn a_refused_credential_fails_as_bundler_does_without_printing_it() {
    let server = Server::start(Index::Compact, Some(&format!("user:{SECRET}")));
    for (name, userinfo, message) in [
        ("bad", Some("user:wrong-pw"), "bad_credentials.txt"),
        ("none", None, "no_credentials.txt"),
    ] {
        let sandbox = Sandbox::new(name);
        let url = server.url("127.0.0.1", userinfo);
        sandbox.write(
            "Gemfile",
            &format!("source {url:?}\ngem \"rack-session\"\n"),
        );
        let out = sandbox.install(&[]);
        let shown = format!(
            "{}exit {}\n",
            String::from_utf8_lossy(&out.stderr),
            out.status.code().unwrap_or(-1)
        );
        assert_eq!(shown, expected(message, &[]), "{name}");
        assert!(
            !text(&out).contains("wrong-pw"),
            "{name}: printed the credential"
        );
        assert!(!sandbox.project().join("Gemfile.lock").exists(), "{name}");
    }
}

#[test]
fn a_source_with_no_compact_index_falls_back_to_specs_4_8() {
    let server = Server::start(Index::Legacy, Some(&format!("user:{SECRET}")));
    let sandbox = Sandbox::new("legacy");
    let url = server.url("127.0.0.1", Some(&format!("user:{SECRET}")));
    sandbox.write(
        "Gemfile",
        &format!("source {url:?}\ngem \"rack-session\"\n"),
    );
    let out = sandbox.install(&[]);
    ok(&out, "install");
    no_secret(&sandbox, &out);
    // Bundler's lockfile once `bundle install` has the gems: each checksum
    // is that of the `.gem` that arrived, the legacy index having none.
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        expected(
            "legacy.lock",
            &[("@SOURCE@", server.url("127.0.0.1", None))]
        )
    );
    assert_eq!(sandbox.linked(), SESSION);
    let paths = server.paths();
    for path in [
        "/versions",
        "/specs.4.8.gz",
        "/prerelease_specs.4.8.gz",
        "/quick/Marshal.4.8/rack-session-2.1.2.gemspec.rz",
        "/gems/rack-session-2.1.2.gem",
    ] {
        assert!(
            paths.contains(&path.to_owned()),
            "{path} not asked for: {paths:?}"
        );
    }

    let out = sandbox.install(&[]);
    assert_eq!(ok(&out, "second install"), "Nothing to change\n");
}

#[test]
fn a_scoped_private_source_beside_a_public_one_locks_as_bundler_does() {
    let public = Server::start(Index::Compact, None);
    let private = Server::start(Index::Compact, Some(&format!("user:{SECRET}")));
    let sandbox = Sandbox::new("scoped");
    let (public_url, private_url) = (
        public.url("localhost", None),
        private.url("127.0.0.1", None),
    );
    sandbox.write(
        "Gemfile",
        &format!(
            "source {public_url:?}\ngem \"rack\"\nsource {private_url:?} do\n  gem \"rackup\"\nend\n"
        ),
    );
    let credential = format!("user:{SECRET}");
    let env = [("BUNDLE_127__0__0__1", credential.as_str())];
    let out = sandbox.install(&env);
    ok(&out, "install");
    no_secret(&sandbox, &out);
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        expected(
            "scoped.lock",
            &[
                ("@SOURCE@", public_url.clone()),
                ("@PRIVATE@", private_url.clone())
            ]
        )
    );
    assert_eq!(sandbox.linked(), ["rack-3.2.8", "rackup-2.3.1"]);
    // rackup came from the private source, rack from the public one, and
    // the credential went to its own host only.
    assert!(
        private
            .paths()
            .contains(&"/gems/rackup-2.3.1.gem".to_owned())
    );
    assert!(public.paths().contains(&"/gems/rack-3.2.8.gem".to_owned()));
    assert!(!public.paths().iter().any(|p| p.contains("rackup")));
    assert_eq!(public.authorizations(), Vec::<String>::new());

    // Nothing changed: nothing resolved, nothing written.
    let before = public.paths().len() + private.paths().len();
    let out = sandbox.install(&env);
    assert_eq!(ok(&out, "second install"), "Nothing to change\n");
    assert_eq!(public.paths().len() + private.paths().len(), before);

    // `--frozen` takes the lockfile it just wrote (#894): rackup is locked
    // from the source the Gemfile scopes it to, as `BUNDLE_FROZEN=true
    // bundle install` takes Bundler's.
    let out = sandbox.install_with(&["--frozen"], &env);
    assert_eq!(ok(&out, "frozen install"), "Nothing to change\n");
    assert_eq!(public.paths().len() + private.paths().len(), before);

    // A gem that did move between sources is one `--frozen` refuses, as
    // `BUNDLE_FROZEN=true bundle install` 4.0.20 does with exit 16: rackup
    // taken out of its block, and rack put into it.
    let lock = sandbox.read("Gemfile.lock");
    let frozen_refuses = |gemfile: String, headline: &str, change: &str| {
        sandbox.write("Gemfile", &gemfile);
        let out = sandbox.install_with(&["--frozen"], &env);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(16), "{}", text(&out));
        assert!(
            stderr.starts_with(&format!(
                "{headline}, but the lockfile can't be updated because frozen mode is set\n\n\
                 You have changed in the Gemfile:\n{change}\n"
            )),
            "{stderr}"
        );
        assert_eq!(sandbox.read("Gemfile.lock"), lock);
    };
    frozen_refuses(
        format!("source {public_url:?}\ngem \"rack\"\ngem \"rackup\"\n"),
        "The list of sources changed",
        &format!("* rackup from `rubygems repository {private_url}/` to `no specified source`"),
    );
    frozen_refuses(
        format!(
            "source {public_url:?}\nsource {private_url:?} do\n  gem \"rack\"\n  gem \"rackup\"\nend\n"
        ),
        "Some dependencies were deleted from your gemfile",
        &format!("* rack from `no specified source` to `rubygems repository {private_url}/`"),
    );
}
