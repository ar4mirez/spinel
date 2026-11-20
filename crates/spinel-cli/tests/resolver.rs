//! The resolver against a real snapshot of rubygems.org's compact index
//! (#78, docs/package-manager.md, "Resolver"): each case under
//! `fixtures/resolver/cases/` is a Gemfile, evaluated by the built `spinel`
//! (`shims/gemfile_eval.rb`) and resolved by `spinel resolve` against
//! `fixtures/resolver/index/` served on 127.0.0.1, for the platforms, Ruby
//! and RubyGems its `case.env` names.
//!
//! What Spinel must choose is what Bundler 4.0.20 chose for the same Gemfile,
//! snapshot, platforms and Ruby (`fixtures/resolver/bundler.sh`): the specs
//! of its `Gemfile.lock`, or, for a Gemfile it cannot resolve, its
//! explanation in `bundler.txt`, word for word. A case with a `spinel.txt`
//! is one whose explanation differs from Bundler's, and that file is what
//! Spinel says; its test checks both name the same requirements. Nothing
//! here touches the network beyond 127.0.0.1.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolver")
}

/// The snapshot as a compact index: `GET` of the files under `index/`.
fn serve_index() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let root = fixtures().join("index");
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

/// One case's `case.env`: `key=value` lines.
fn setting(case: &Path, key: &str) -> String {
    let env = fs::read_to_string(case.join("case.env")).expect("case.env");
    env.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("{}: no {key}", case.display()))
        .to_owned()
}

/// Evaluate the case's Gemfile and resolve it; the source is spelled
/// `@SOURCE@` in what comes back.
fn resolve(name: &str, source: &str) -> Output {
    resolve_with(name, source, &[])
}

/// [`resolve`], with more arguments to `spinel resolve`.
fn resolve_with(name: &str, source: &str, args: &[&str]) -> Output {
    let case = fixtures().join("cases").join(name);
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shims/gemfile_eval.rb");
    let eval = Command::new(SPINEL)
        .arg("run")
        .arg(&shim)
        .current_dir(&case)
        .env_remove("BUNDLE_GEMFILE")
        .env("SPINEL_RESOLVER_SOURCE", source)
        .output()
        .expect("spinel runs");
    assert!(
        eval.status.success(),
        "{name}: the Gemfile does not evaluate: {}",
        String::from_utf8_lossy(&eval.stderr)
    );

    let home = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("resolver")
        .join(name);
    let _ = fs::remove_dir_all(&home);
    let mut cmd = Command::new(SPINEL);
    cmd.arg("resolve").arg("-").args(args);
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
    child.stdin.take().unwrap().write_all(&eval.stdout).unwrap();
    let mut out = child.wait_with_output().expect("spinel resolve exits");
    for stream in [&mut out.stdout, &mut out.stderr] {
        *stream = String::from_utf8_lossy(stream)
            .replace(source, "@SOURCE@")
            .into_bytes();
    }
    out
}

/// The `specs:` of a case's `Gemfile.lock`, as Bundler wrote it.
fn bundler_specs(name: &str) -> Vec<String> {
    let lock = fs::read_to_string(fixtures().join("cases").join(name).join("Gemfile.lock"))
        .expect("Gemfile.lock");
    lock.lines()
        .skip_while(|line| *line != "  specs:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.strip_prefix("    "))
        .filter(|line| !line.starts_with(' '))
        .map(str::to_owned)
        .collect()
}

/// What Bundler said when it could not resolve the case: its explanation,
/// without the progress lines `bundler.sh` already dropped.
fn bundler_explanation(name: &str) -> String {
    fs::read_to_string(fixtures().join("cases").join(name).join("bundler.txt"))
        .expect("bundler.txt")
}

fn assert_resolves_as_bundler(name: &str) {
    assert_resolves_as_bundler_with(name, &[]);
}

fn assert_resolves_as_bundler_with(name: &str, args: &[&str]) {
    let source = serve_index();
    let out = resolve_with(name, &source, args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ours: Vec<&str> = stdout.lines().collect();
    assert_eq!(ours, bundler_specs(name), "{name}: not what Bundler chose");
}

fn assert_explains(name: &str) -> String {
    let source = serve_index();
    let out = resolve(name, &source);
    assert_eq!(out.status.code(), Some(1), "{name}: should not resolve");
    assert!(out.stdout.is_empty(), "{name}: printed a resolution");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_explains_as_bundler(name: &str) {
    let explanation = assert_explains(name);
    assert_eq!(explanation, bundler_explanation(name), "{name}");
}

/// The Sinatra fixture's Gemfile (`fixtures/sinatra-lock`): every gem of the
/// real dependency set at the version Bundler picks.
#[test]
fn resolves_sinatra_as_bundler_does() {
    assert_resolves_as_bundler("sinatra");
}

/// On Ruby 2.6, Sinatra 4 (`ruby: >= 2.7.8`) is out, and the order Bundler
/// decides gems in takes rack 3 for rackup first, which leaves Sinatra 1.0.
#[test]
fn filters_by_required_ruby_version_as_bundler_does() {
    assert_resolves_as_bundler("old_ruby");
}

/// rackup 2 needs rack 3, which every Sinatra 2 and 3 refuses: back to 1.0.
#[test]
fn backtracks_as_bundler_does() {
    assert_resolves_as_bundler("backtrack");
}

/// json has `java` builds: one for each platform resolved for.
#[test]
fn takes_each_platforms_build_as_bundler_does() {
    assert_resolves_as_bundler("json_two_platforms");
}

/// A requirement naming a prerelease lets prereleases in; without one,
/// `< 3.0.0` does not take `3.0.0.rc1`.
#[test]
fn takes_prereleases_only_when_asked_as_bundler_does() {
    assert_resolves_as_bundler("prerelease");
    assert_resolves_as_bundler("no_prerelease");
}

/// sinatra ~> 4.0 needs rack 3, and the Gemfile asks for rack ~> 2.2.
#[test]
fn explains_a_conflict_as_bundler_does() {
    assert_explains_as_bundler("conflict");
}

/// Sinatra 4 needs Ruby 2.7.8, and the Ruby is 2.6.10.
#[test]
fn explains_a_ruby_version_conflict_as_bundler_does() {
    assert_explains_as_bundler("conflict_ruby");
}

/// The conflict is between two of the Gemfile's gems' dependencies.
#[test]
fn explains_a_transitive_conflict_as_bundler_does() {
    assert_explains_as_bundler("conflict_transitive");
}

/// Three gems, two steps. Bundler states a dependency for every version
/// that shares it, prereleases included once it retries with them, where
/// pubgrub-rs states it for the versions it tried: so where Bundler says
/// `sinatra >= 2.0.0.beta1, < 3.0.0`, Spinel says `sinatra >= 2.0.0, <
/// 3.0.0`. Everything else is Bundler's, line for line.
#[test]
fn explains_a_two_step_conflict_as_bundler_does_but_one_bound() {
    let explanation = assert_explains("conflict_three");
    let expected = fs::read_to_string(fixtures().join("cases/conflict_three/spinel.txt")).unwrap();
    assert_eq!(explanation, expected);
    let bundler = bundler_explanation("conflict_three");
    assert_eq!(
        explanation.replace(
            "sinatra >= 2.0.0, < 3.0.0",
            "sinatra >= 2.0.0.beta1, < 3.0.0"
        ),
        bundler
    );
}

/// A Gemfile gem no version satisfies is reported before resolving, with
/// what the source has, as Bundler reports it; Bundler's names its source
/// "or installed locally" too, which Spinel's is not.
#[test]
fn reports_a_gem_not_found_as_bundler_does() {
    let explanation = assert_explains("not_found");
    let expected = fs::read_to_string(fixtures().join("cases/not_found/spinel.txt")).unwrap();
    assert_eq!(explanation, expected);
    let bundler = bundler_explanation("not_found");
    assert_eq!(
        explanation.replace("@SOURCE@/.", "@SOURCE@/\nor installed locally."),
        bundler
    );
}

/// The port of Bundler's pubgrub-ruby (#816), behind `--solver
/// pubgrub-ruby` until it is the only solver, resolves every case that
/// resolves as Bundler does: Bundler's first try decides each, so the
/// retries the port does not make yet are not needed. Each case that does
/// not resolve fails on it too; its explanation is not Bundler's yet.
#[test]
fn the_pubgrub_ruby_port_resolves_as_bundler_does() {
    let port = ["--solver", "pubgrub-ruby"];
    for name in [
        "sinatra",
        "old_ruby",
        "backtrack",
        "json_two_platforms",
        "prerelease",
        "no_prerelease",
    ] {
        assert_resolves_as_bundler_with(name, &port);
    }
    let source = serve_index();
    for name in [
        "conflict",
        "conflict_ruby",
        "conflict_three",
        "conflict_transitive",
    ] {
        let out = resolve_with(name, &source, &port);
        assert_eq!(out.status.code(), Some(1), "{name}: should not resolve");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.starts_with("Could not find compatible versions\n"),
            "{name}: {stderr}"
        );
    }
    // A Gemfile gem nothing satisfies is reported before either solver runs.
    let out = resolve_with("not_found", &source, &port);
    let expected = fs::read_to_string(fixtures().join("cases/not_found/spinel.txt")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), expected);
}

/// The port takes no lockfile yet: `--locked` with it is refused.
#[test]
fn the_pubgrub_ruby_port_takes_no_lockfile_yet() {
    let out = Command::new(SPINEL)
        .args([
            "resolve",
            "-",
            "--solver",
            "pubgrub-ruby",
            "--locked",
            "Gemfile.lock",
        ])
        .output()
        .expect("spinel runs");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// Every case is one of the above.
#[test]
fn every_case_is_tested() {
    let mut cases: Vec<String> = fs::read_dir(fixtures().join("cases"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    cases.sort();
    assert_eq!(
        cases,
        [
            "backtrack",
            "conflict",
            "conflict_ruby",
            "conflict_three",
            "conflict_transitive",
            "json_two_platforms",
            "no_prerelease",
            "not_found",
            "old_ruby",
            "prerelease",
            "sinatra",
        ]
    );
}
