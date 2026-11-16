//! `Gemfile.lock` both ways (#80, docs/package-manager.md, "Lockfile"),
//! against lockfiles Bundler 4.0.20 wrote.
//!
//! Bundler's to Spinel: `spinel lockfile` reads every lockfile Bundler wrote
//! for the fixtures and writes it back out byte for byte. They are each case
//! under `fixtures/lockfile/cases/` (`bundler.sh` locks them against the
//! Rails-sized snapshot in `fixtures/lockfile/index/`: Rails, a lockfile for
//! ten platforms, a `ruby` directive, one without `CHECKSUMS`, and one with
//! GIT, PATH and two GEM sections), each resolved case under
//! `fixtures/resolver/cases/`, and the Sinatra app's.
//!
//! Spinel's to Bundler: for each of those Gemfiles Spinel can resolve (one
//! rubygems source, `CHECKSUMS` on, as Bundler 4 writes a new lockfile),
//! `spinel resolve --lockfile` against the same snapshot, served on
//! 127.0.0.1, writes the lockfile Bundler wrote, byte for byte; `bundle
//! install` of it changes nothing, as `install_check.sh` (opt-in, it needs
//! Ruby) shows with Bundler itself. Nothing here touches the network beyond
//! 127.0.0.1.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// `index/` under `fixtures/<set>` as a compact index: `GET` of its files.
fn serve_index(set: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let root = fixtures().join(set).join("index");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = root.clone();
            std::thread::spawn(move || serve(stream, &root));
        }
    });
    url
}

fn serve(stream: TcpStream, root: &Path) {
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
    let file = path
        .strip_prefix('/')
        .filter(|p| !p.split('/').any(|part| part == ".." || part.is_empty()))
        .and_then(|p| fs::read(root.join(p)).ok());
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

/// Every lockfile Bundler wrote for the fixtures.
fn bundler_lockfiles() -> Vec<PathBuf> {
    let mut locks = Vec::new();
    for set in ["lockfile/cases", "resolver/cases"] {
        let mut cases: Vec<PathBuf> = fs::read_dir(fixtures().join(set))
            .unwrap()
            .map(|e| e.unwrap().path().join("Gemfile.lock"))
            .filter(|p| p.exists())
            .collect();
        cases.sort();
        locks.extend(cases);
    }
    locks.push(fixtures().join("sinatra-lock/Gemfile.lock"));
    locks
}

#[test]
fn every_bundler_lockfile_round_trips_byte_for_byte() {
    let locks = bundler_lockfiles();
    // The five lockfile cases, the six resolver cases that resolve, the
    // Sinatra app.
    assert_eq!(locks.len(), 12, "{locks:?}");
    for lock in locks {
        let out = Command::new(SPINEL)
            .arg("lockfile")
            .arg(&lock)
            .output()
            .expect("spinel runs");
        let written = ok(&out, &format!("spinel lockfile {}", lock.display()));
        assert_eq!(
            written,
            fs::read_to_string(&lock).unwrap(),
            "{} did not come back unchanged",
            lock.display()
        );
    }
}

#[test]
fn reads_a_lockfile_from_standard_input() {
    let lock = fs::read(fixtures().join("lockfile/cases/sources/Gemfile.lock")).unwrap();
    let mut child = Command::new(SPINEL)
        .args(["lockfile", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    child.stdin.take().unwrap().write_all(&lock).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(ok(&out, "spinel lockfile -").into_bytes(), lock);
}

#[test]
fn an_unreadable_lockfile_is_refused_as_bundler_refuses_it() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lockfile-unreadable");
    fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("Gemfile.lock");
    fs::write(
        &lock,
        "GEM\n  remote: https://rubygems.org/\n  specs:\n    rack (3..2)\n",
    )
    .unwrap();
    let out = Command::new(SPINEL)
        .arg("lockfile")
        .arg(&lock)
        .output()
        .expect("spinel runs");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "Your lockfile is unreadable. Run `rm {0}` and then `bundle install` to generate a \
             new lockfile. The error occurred while evaluating {0}:4:1\n",
            lock.display()
        )
    );
}

/// One case's `case.env`: `key=value` lines.
fn setting(case: &Path, key: &str) -> String {
    let env = fs::read_to_string(case.join("case.env")).expect("case.env");
    env.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("{}: no {key}", case.display()))
        .to_owned()
}

/// Evaluate a case's Gemfile and write its lockfile with `spinel resolve
/// --lockfile`, for the case's platforms, Ruby and RubyGems; the source is
/// spelled `@SOURCE@` in what comes back.
fn spinel_lockfile(set: &str, name: &str) -> String {
    let source = serve_index(set);
    let case = fixtures().join(set).join("cases").join(name);
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shims/gemfile_eval.rb");
    let eval = Command::new(SPINEL)
        .arg("run")
        .arg(&shim)
        .current_dir(&case)
        .env_remove("BUNDLE_GEMFILE")
        .env("SPINEL_RESOLVER_SOURCE", &source)
        .output()
        .expect("spinel runs");
    let deps = ok(&eval, &format!("{name}: evaluating the Gemfile"));

    let home = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("lockfile")
        .join(set)
        .join(name);
    let _ = fs::remove_dir_all(&home);
    let mut cmd = Command::new(SPINEL);
    cmd.args(["resolve", "--lockfile", "-"]);
    for platform in setting(&case, "platforms").split_whitespace() {
        cmd.arg("--platform").arg(platform);
    }
    cmd.arg("--ruby-version")
        .arg(setting(&case, "ruby"))
        .arg("--rubygems-version")
        .arg(setting(&case, "rubygems"))
        .env("SPINEL_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spinel runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(deps.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("spinel resolve exits");
    ok(&out, &format!("{name}: spinel resolve --lockfile")).replace(&source, "@SOURCE@")
}

fn assert_writes_what_bundler_wrote(set: &str, name: &str) {
    let ours = spinel_lockfile(set, name);
    let bundlers = fs::read_to_string(
        fixtures()
            .join(set)
            .join("cases")
            .join(name)
            .join("Gemfile.lock"),
    )
    .unwrap();
    assert_eq!(
        ours, bundlers,
        "{set}/{name}: not the lockfile Bundler wrote"
    );
}

/// `rails new`'s Gemfile: 118 gems, `bundler` among their dependencies, and
/// the platforms Bundler adds to a new lockfile for nokogiri's and sqlite3's
/// native builds, `x86_64-linux` given up for `x86_64-linux-gnu`.
#[test]
fn writes_rails_lockfile_as_bundler_does() {
    assert_writes_what_bundler_wrote("lockfile", "rails");
}

/// Four platforms asked for; seven more Bundler adds, `x86_64-linux` given
/// up for `x86_64-linux-gnu`; and each one's builds.
#[test]
fn writes_a_multi_platform_lockfile_as_bundler_does() {
    assert_writes_what_bundler_wrote("lockfile", "multi_platform");
}

#[test]
fn writes_ruby_version_as_bundler_does() {
    assert_writes_what_bundler_wrote("lockfile", "ruby_version");
}

#[test]
fn writes_the_resolver_cases_lockfiles_as_bundler_does() {
    for name in [
        "backtrack",
        "json_two_platforms",
        "no_prerelease",
        "old_ruby",
        "prerelease",
        "sinatra",
    ] {
        assert_writes_what_bundler_wrote("resolver", name);
    }
}
