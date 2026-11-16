//! Locked preferences (#79, docs/package-manager.md, "Locked preferences"):
//! `spinel resolve --locked` keeps a lockfile's versions as Bundler 4.0.20
//! keeps them when no update is asked for.
//!
//! Each case under `fixtures/resolver/locked/` is a run of Gemfile edits,
//! `1.Gemfile`, `2.Gemfile`, ..., and the lockfile Bundler wrote after each
//! (`<n>.Gemfile.lock`, by `fixtures/resolver/locked.sh`, which locks each
//! step with the one before it in place). The test replays them: step 1 with
//! no lockfile, every later step with the lockfile Bundler wrote for the step
//! before, against the resolver's snapshot served on 127.0.0.1, and checks
//! Spinel writes Bundler's lockfile byte for byte. The cases are an unchanged
//! Gemfile (the same lockfile again, without the index being read), a gem
//! added (nothing else moves), a gem taken out, a requirement loosened (the
//! locked version still meets it, so nothing moves), one tightened (that gem
//! and what must move with it move), one a locked gem can only meet by going
//! down (the floor on it gives way, and Bundler's retry decides how far), a
//! lockfile for two platforms, a newly named gem pinned to its locked
//! version until a conflict loosens it, a `ruby` directive, and `rails
//! new`'s Gemfile on seven platforms against the Rails-sized snapshot
//! (`index=lockfile` in `case.env`).
//!
//! And every lockfile Bundler wrote for the fixtures whose Gemfile Spinel can
//! resolve comes back byte for byte when its Gemfile is resolved again with
//! it, with no request made of the index.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolver")
}

/// `fixtures/<set>/index` as a compact index (`resolver`, or `lockfile` for
/// the Rails-sized snapshot), and a count of the requests it has served.
fn serve_index(set: &str) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let root = fixtures().join("..").join(set).join("index");
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let root = root.clone();
            std::thread::spawn(move || serve(stream, &root));
        }
    });
    (url, requests)
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

/// One case's `case.env`: `key=value` lines.
fn setting(case: &Path, key: &str) -> String {
    optional_setting(case, key).unwrap_or_else(|| panic!("{}: no {key}", case.display()))
}

fn optional_setting(case: &Path, key: &str) -> Option<String> {
    let env = fs::read_to_string(case.join("case.env")).expect("case.env");
    env.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .map(str::to_owned)
}

/// Evaluate the Gemfile in `dir` and resolve it with `spinel resolve
/// --lockfile`, for `platforms` (the local one first) and the case's Ruby,
/// with `--locked` the lockfile at `locked` when there is one, updating the
/// gems `update` names (`--update`, every gem when it names none).
fn resolve(
    dir: &Path,
    case: &Path,
    source: &str,
    platforms: &[&str],
    locked: Option<&Path>,
    update: Option<&[String]>,
) -> String {
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shims/gemfile_eval.rb");
    let eval = Command::new(SPINEL)
        .arg("run")
        .arg(&shim)
        .current_dir(dir)
        .env_remove("BUNDLE_GEMFILE")
        .env("SPINEL_RESOLVER_SOURCE", source)
        .output()
        .expect("spinel runs");
    let deps = ok(&eval, &format!("{}: evaluating", dir.display()));

    let mut cmd = Command::new(SPINEL);
    cmd.args(["resolve", "--lockfile", "-"]);
    for platform in platforms {
        cmd.arg("--platform").arg(platform);
    }
    if let Some(locked) = locked {
        cmd.arg("--locked").arg(locked);
    }
    match update {
        None => {}
        Some([]) => {
            cmd.arg("--update");
        }
        Some(gems) => {
            cmd.arg(format!("--update={}", gems.join(",")));
        }
    }
    cmd.arg("--ruby-version")
        .arg(setting(case, "ruby"))
        .arg("--rubygems-version")
        .arg(setting(case, "rubygems"))
        .env("SPINEL_HOME", dir.join("home"))
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
    ok(&out, &format!("{}: spinel resolve", dir.display())).replace(source, "@SOURCE@")
}

/// A case's step: its Gemfile evaluated in a directory of its own, then
/// `spinel resolve --lockfile`, with `--locked` the lockfile Bundler wrote
/// for the step before when there is one. The lockfile written, with the
/// source spelled `@SOURCE@`.
struct Step<'a> {
    case: &'a str,
    step: usize,
    source: &'a str,
    /// The test's own name for its scratch directory, so tests replaying
    /// the same step in parallel never share one.
    test: &'a str,
}

impl Step<'_> {
    fn dir(&self) -> PathBuf {
        fixtures().join("locked").join(self.case)
    }

    fn work(&self) -> PathBuf {
        let work = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("locked")
            .join(self.test)
            .join(self.case)
            .join(self.step.to_string());
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();
        work
    }

    fn lock(&self) -> String {
        let dir = self.dir();
        let work = self.work();
        fs::copy(
            dir.join(format!("{}.Gemfile", self.step)),
            work.join("Gemfile"),
        )
        .unwrap();
        let platforms = setting(&dir, "platforms");
        let platforms: Vec<&str> = platforms.split_whitespace().collect();
        if self.step == 1 {
            return resolve(&work, &dir, self.source, &platforms, None, None);
        }
        let before = fs::read_to_string(dir.join(format!("{}.Gemfile.lock", self.step - 1)))
            .unwrap()
            .replace("@SOURCE@", self.source);
        let lock = work.join("Gemfile.lock");
        fs::write(&lock, before).unwrap();
        let update: Option<Vec<String>> =
            fs::read_to_string(dir.join(format!("{}.update", self.step)))
                .ok()
                .map(|gems| gems.split_whitespace().map(str::to_owned).collect());
        resolve(
            &work,
            &dir,
            self.source,
            &platforms[..1],
            Some(&lock),
            update.as_deref(),
        )
    }

    fn bundlers(&self) -> String {
        fs::read_to_string(self.dir().join(format!("{}.Gemfile.lock", self.step))).unwrap()
    }
}

/// Replay every step of `case` and compare each lockfile with Bundler's.
fn replays_as_bundler_does(case: &str) {
    let dir = fixtures().join("locked").join(case);
    let index = optional_setting(&dir, "index").unwrap_or_else(|| "resolver".to_owned());
    let (source, _) = serve_index(&index);
    let mut step = 1;
    while dir.join(format!("{step}.Gemfile")).exists() {
        let s = Step {
            case,
            step,
            source: &source,
            test: "replay",
        };
        assert_eq!(
            s.lock(),
            s.bundlers(),
            "{case}, step {step}: not the lockfile Bundler wrote"
        );
        step += 1;
    }
    assert!(step > 2, "{case}: a case is two steps or more");
}

/// The gems and versions a lockfile's `specs:` lists.
fn specs(lock: &str) -> Vec<String> {
    lock.lines()
        .filter(|l| l.starts_with("    ") && !l.starts_with("      "))
        .map(|l| l.trim().to_owned())
        .collect()
}

/// An old lockfile kept: `rack < 3` pinned Sinatra 3 and Rack 2, the pin is
/// taken out and nothing moves, the same Gemfile again writes the same
/// lockfile byte for byte, and adding WEBrick moves nothing else.
#[test]
fn add_gem_moves_nothing_else() {
    replays_as_bundler_does("add_gem");
    let dir = fixtures().join("locked/add_gem");
    let before = specs(&fs::read_to_string(dir.join("3.Gemfile.lock")).unwrap());
    let after = specs(&fs::read_to_string(dir.join("4.Gemfile.lock")).unwrap());
    let added: Vec<&String> = after.iter().filter(|s| !before.contains(s)).collect();
    assert_eq!(added, ["webrick (1.9.2)"]);
    assert!(
        before.iter().all(|s| after.contains(s)),
        "{before:?} {after:?}"
    );
}

/// An unchanged Gemfile: the lockfile comes back byte for byte, and the
/// index is never asked for anything.
#[test]
fn unchanged_gemfile_writes_the_same_lockfile_without_the_index() {
    let (source, requests) = serve_index("resolver");
    let s = Step {
        case: "add_gem",
        step: 3,
        source: &source,
        test: "unchanged",
    };
    let before = fs::read_to_string(s.dir().join("2.Gemfile.lock")).unwrap();
    assert_eq!(s.lock(), before);
    assert_eq!(requests.load(Ordering::SeqCst), 0, "the index was read");
}

/// A lockfile Bundler 4.0.10 wrote, with its `BUNDLED WITH` and a checksum
/// line for itself, and an unchanged Gemfile: `write_lock` crosses no major
/// version and leaves it byte for byte, without the index.
#[test]
fn a_lockfile_from_an_earlier_bundler_4_comes_back_unchanged() {
    let (source, requests) = serve_index("resolver");
    let dir = fixtures().join("locked/add_gem");
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("locked-bundler-4.0.10");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();
    fs::copy(dir.join("2.Gemfile"), work.join("Gemfile")).unwrap();
    let before = fs::read_to_string(dir.join("2.Gemfile.lock"))
        .unwrap()
        .replace(
            "  mustermann (3.1.1) sha256=",
            "  bundler (4.0.10) sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n  mustermann (3.1.1) sha256=",
        )
        .replace("BUNDLED WITH\n  4.0.20\n", "BUNDLED WITH\n  4.0.10\n");
    assert!(before.contains("BUNDLED WITH\n  4.0.10\n") && before.contains("bundler (4.0.10)"));
    let lock = work.join("Gemfile.lock");
    fs::write(&lock, before.replace("@SOURCE@", &source)).unwrap();
    let ours = resolve(&work, &dir, &source, &["x86_64-linux"], Some(&lock), None);
    assert_eq!(ours, before);
    assert_eq!(requests.load(Ordering::SeqCst), 0, "the index was read");
}

/// `~> 3.0` to `>= 3.0`: Sinatra 3.2.0 still meets it, so nothing moves and
/// only `DEPENDENCIES` changes.
#[test]
fn loosened_requirement_moves_nothing() {
    replays_as_bundler_does("loosen");
}

/// `~> 3.0` to `~> 4.0`: Sinatra moves, and so do the gems Sinatra 4 needs
/// newer (Rack, rack-protection, and rackup, which Rack 3 takes past 1.x);
/// tilt and mustermann stay.
#[test]
fn tightened_requirement_moves_what_bundler_moves() {
    replays_as_bundler_does("tighten");
}

/// `rack < 3` added to a lockfile with Rack 3: Rack can only go down, and
/// the floors on Sinatra and rackup give way as Bundler's retries take them
/// off, which takes both back to versions with no upper bound on Rack.
#[test]
fn a_gem_that_must_go_down_unlocks_as_bundler_does() {
    replays_as_bundler_does("downgrade");
}

/// A `java` and `x86_64-linux` lockfile: json's locked builds stay, for
/// both, when its pin is taken out and when Rack is added.
#[test]
fn keeps_each_platforms_locked_builds() {
    replays_as_bundler_does("platforms");
}

/// `rails new`'s Gemfile on its seven platforms: Rails 8.1.4, nokogiri
/// 1.19.6 (six native builds), Puma 8.0.3 and Rack 3.2.7 locked under pins,
/// kept when the pins are taken out; the same Gemfile again writes the same
/// lockfile; rubocop-minitest added moves nothing else; Puma's
/// requirement raised past its locked version moves Puma alone; `--update
/// nokogiri` moves its six native builds to 1.19.7; and `--update` moves
/// Rails and the rest (#87).
#[test]
fn rails_lockfile_keeps_its_versions_as_bundler_does() {
    replays_as_bundler_does("rails");
}

/// Every lockfile Bundler wrote for a fixture Gemfile with one rubygems
/// source, Gemfile unchanged: the same lockfile, byte for byte, and no
/// request made of the index.
#[test]
fn every_bundler_lockfile_comes_back_unchanged() {
    let cases = [
        ("lockfile", "multi_platform"),
        ("lockfile", "no_checksums"),
        ("lockfile", "rails"),
        ("lockfile", "ruby_version"),
        ("resolver", "backtrack"),
        ("resolver", "json_two_platforms"),
        ("resolver", "no_prerelease"),
        ("resolver", "old_ruby"),
        ("resolver", "prerelease"),
        ("resolver", "sinatra"),
    ];
    for (set, name) in cases {
        let (source, requests) = serve_index(set);
        let case = fixtures().join("..").join(set).join("cases").join(name);
        let work = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("locked-unchanged")
            .join(set)
            .join(name);
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();
        fs::copy(case.join("Gemfile"), work.join("Gemfile")).unwrap();
        let bundlers = fs::read_to_string(case.join("Gemfile.lock")).unwrap();
        let lock = work.join("Gemfile.lock");
        fs::write(&lock, bundlers.replace("@SOURCE@", &source)).unwrap();
        let platforms = setting(&case, "platforms");
        let local = platforms.split_whitespace().next().unwrap();
        let ours = resolve(&work, &case, &source, &[local], Some(&lock), None);
        assert_eq!(ours, bundlers, "{set}/{name}: the lockfile changed");
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "{set}/{name}: the index was read"
        );
    }
}

/// Rack named in the Gemfile with rackup 2, which needs Rack 3, in one edit:
/// the locked Rack 2.2.25 meets the new `rack` line, so it is pinned to it
/// exactly; the conflict loosens the pin to a floor and takes the floor off
/// Sinatra, and Rack, Sinatra and rack-protection move.
#[test]
fn an_exact_pin_on_a_newly_named_gem_gives_way_as_bundler_does() {
    replays_as_bundler_does("pin_transitive");
}

/// A `ruby` directive: `RUBY VERSION` is written, kept when Rack's pin is
/// taken out and when the directive changes to one the locked Ruby still
/// meets, dropped with the directive, and back with it, Rack 2.2.25 kept
/// throughout.
#[test]
fn ruby_version_is_kept_while_the_directive_allows_it() {
    replays_as_bundler_does("ruby_directive");
}

/// Updates (#87), as `bundle lock --update` makes them: Sinatra, rackup,
/// rake, Rack and tilt locked old under pins, the pins taken out (nothing
/// moves), then `--update rake` moves rake alone while newer versions of
/// every other gem exist, `--update rackup` moves rackup and the Rack it
/// depends on (WEBrick, which rackup 2.3 no longer needs, goes) and leaves
/// Sinatra and tilt, and `--update` with no gems moves everything to the
/// newest the Gemfile allows.
#[test]
fn updates_move_what_bundler_moves() {
    replays_as_bundler_does("update");
    let dir = fixtures().join("locked/update");
    let specs_of =
        |step: usize| specs(&fs::read_to_string(dir.join(format!("{step}.Gemfile.lock"))).unwrap());
    let moved = |from: usize, to: usize| {
        let (before, after) = (specs_of(from), specs_of(to));
        let mut moved: Vec<String> = after.into_iter().filter(|s| !before.contains(s)).collect();
        moved.sort();
        moved
    };
    assert_eq!(moved(2, 3), ["rake (13.4.2)"]);
    assert_eq!(moved(3, 4), ["rack (3.2.8)", "rackup (2.3.1)"]);
    assert_eq!(
        moved(4, 5),
        ["rack-protection (4.2.1)", "sinatra (4.2.1)", "tilt (2.9.0)"]
    );
}
