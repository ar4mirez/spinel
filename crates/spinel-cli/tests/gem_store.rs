//! The global gem store against a local source (#81, docs/package-manager.md,
//! "Global store"): `spinel fetch` reads each gem's checksum from the compact
//! index, downloads the `.gem`s in parallel, verifies and unpacks them as
//! RubyGems does, and keeps one entry per gem and checksum for every project.
//! The gems are the real ones `tests/fixtures/gem_store/build.sh` builds with
//! `gem build`; the corrupt and malicious ones are made from them here.
//! Nothing touches the network beyond 127.0.0.1.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use md5::Md5;
use sha2::{Digest as _, Sha256, Sha512};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gem_store")
}

fn fixture_gem(file: &str) -> Vec<u8> {
    let path = fixtures().join("gems").join(file);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

#[derive(Default)]
struct State {
    files: HashMap<String, Vec<u8>>,
    /// Hold each `/gems/` request until this many are in flight (or a few
    /// seconds pass), so a client that fetched one at a time is seen to.
    hold_gems: usize,
    gems_in_flight: usize,
    max_gems_in_flight: usize,
    /// Send half of this file's body, with the whole length promised, and
    /// close the connection.
    cut: HashSet<String>,
    /// Send half of this file's body and then wait, until `stall` is
    /// cleared, with `stalled` set meanwhile.
    stall: Option<String>,
    stalled: bool,
    seen: Vec<String>,
}

/// A gem source on 127.0.0.1: `GET` of the files in `State`, nothing more.
/// The compact index client sees no ETag and no digest, so it simply
/// fetches the index files whole.
struct Server {
    url: String,
    state: Arc<(Mutex<State>, Condvar)>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        Server { url, state }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let result = f(&mut self.state.0.lock().unwrap());
        self.state.1.notify_all();
        result
    }

    /// Serve these gems, with a compact index listing each one's checksum.
    /// `listed` is the SHA-256 the index gives, which a test can make wrong.
    fn publish(&self, gems: &[Published]) {
        let mut versions = b"created_at: 2026-01-01T00:00:00Z\n---\n".to_vec();
        let mut infos: HashMap<&str, Vec<u8>> = HashMap::new();
        for gem in gems {
            let info = infos.entry(gem.name).or_insert_with(|| b"---\n".to_vec());
            info.extend(format!("{} |checksum:{}\n", gem.version, gem.listed).into_bytes());
        }
        for (name, info) in &infos {
            versions.extend(format!("{name} {} {}\n", "0", hex(&Md5::digest(info))).into_bytes());
            self.with(|s| s.files.insert(format!("/info/{name}"), info.clone()));
        }
        self.with(|s| {
            s.files.insert("/versions".to_owned(), versions);
            for gem in gems {
                s.files
                    .insert(format!("/gems/{}", gem.file), gem.body.clone());
            }
        });
    }

    /// The `/gems/` requests since the last call.
    fn take_gem_requests(&self) -> Vec<String> {
        self.with(|s| {
            std::mem::take(&mut s.seen)
                .into_iter()
                .filter(|p| p.starts_with("/gems/"))
                .collect()
        })
    }
}

/// One `.gem` as a source serves it.
#[derive(Clone)]
struct Published {
    name: &'static str,
    /// As the `/info` line's first field spells it.
    version: &'static str,
    file: String,
    body: Vec<u8>,
    listed: String,
}

impl Published {
    fn new(name: &'static str, version: &'static str, file: &str, body: Vec<u8>) -> Published {
        Published {
            name,
            version,
            file: file.to_owned(),
            listed: sha256(&body),
            body,
        }
    }

    fn fixture(name: &'static str, version: &'static str) -> Published {
        let file = format!("{name}-{version}.gem");
        Published::new(name, version, &file, fixture_gem(&file))
    }

    fn spec(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// The store entry's name: `<name>-<version>-<platform>-<sha256>`.
    fn entry(&self) -> String {
        let (version, platform) = self
            .version
            .split_once('-')
            .unwrap_or((self.version, "ruby"));
        format!("{}-{version}-{platform}-{}", self.name, self.listed)
    }

    fn full_name(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }
}

fn serve(stream: TcpStream, shared: &(Mutex<State>, Condvar)) {
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
    let (lock, wake) = shared;
    let is_gem = path.starts_with("/gems/");
    let mut state = lock.lock().unwrap();
    state.seen.push(path.clone());
    if is_gem {
        state.gems_in_flight += 1;
        state.max_gems_in_flight = state.max_gems_in_flight.max(state.gems_in_flight);
        wake.notify_all();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Once the count has reached the hold, every request goes: one
        // answered first must not hold back the others.
        while state.max_gems_in_flight < state.hold_gems && Instant::now() < deadline {
            state = wake
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }
    let body = state.files.get(&path).cloned();
    let cut = state.cut.contains(&path);
    let stall = state.stall.as_deref() == Some(path.as_str());
    drop(state);

    let mut stream = stream;
    match body {
        None => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 9\r\n\r\nnot found",
            );
        }
        Some(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            if cut || stall {
                let _ = stream.write_all(&body[..body.len() / 2]);
                let _ = stream.flush();
                if stall {
                    let mut state = lock.lock().unwrap();
                    state.stalled = true;
                    wake.notify_all();
                    let deadline = Instant::now() + Duration::from_secs(20);
                    while state.stall.is_some() && Instant::now() < deadline {
                        state = wake
                            .wait_timeout(state, Duration::from_millis(50))
                            .unwrap()
                            .0;
                    }
                }
            } else {
                let _ = stream.write_all(&body);
            }
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
    if is_gem {
        lock.lock().unwrap().gems_in_flight -= 1;
        wake.notify_all();
    }
}

/// A Spinel home for one test, under Cargo's scratch directory for tests,
/// so the store it holds is never the real `~/.spinel`.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("gem-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    fn store(&self) -> PathBuf {
        self.0.join("store")
    }

    /// What `store/gems/` holds.
    fn entries(&self) -> Vec<String> {
        names(&self.store().join("gems"))
    }

    /// What `store/tmp/` holds.
    fn staging(&self) -> Vec<String> {
        names(&self.store().join("tmp"))
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

fn command(home: &Home, server: &Server, specs: &[String]) -> Command {
    let mut command = Command::new(SPINEL);
    command
        .arg("fetch")
        .arg("--source")
        .arg(&server.url)
        .args(specs)
        .env("SPINEL_HOME", &home.0);
    for proxy in [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ] {
        command.env_remove(proxy);
    }
    command
}

fn fetch(home: &Home, server: &Server, gems: &[&Published]) -> Output {
    let specs: Vec<String> = gems.iter().map(|g| g.spec()).collect();
    command(home, server, &specs).output().expect("spinel runs")
}

/// Run `spinel fetch`, expecting success, and return its lines after the
/// first, which names the store.
fn fetch_ok(home: &Home, server: &Server, gems: &[&Published]) -> Vec<String> {
    let out = fetch(home, server, gems);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "spinel fetch failed\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut lines = stdout.lines().map(str::to_owned);
    assert_eq!(
        lines.next(),
        Some(format!("store: {}", home.store().display()))
    );
    lines.collect()
}

/// Run `spinel fetch`, expecting it to fail, and return its stderr.
fn fetch_err(home: &Home, server: &Server, gems: &[&Published]) -> String {
    let out = fetch(home, server, gems);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.status.code(),
        Some(1),
        "spinel fetch did not fail\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    stderr
}

fn fetched(gem: &Published) -> String {
    format!(
        "{}: fetched {} bytes (gems/{})",
        gem.full_name(),
        gem.body.len(),
        gem.entry()
    )
}

fn in_store(gem: &Published) -> String {
    format!("{}: in the store (gems/{})", gem.full_name(), gem.entry())
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn gunzip(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    GzDecoder::new(data).read_to_end(&mut out).unwrap();
    out
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(data).unwrap();
    gz.finish().unwrap()
}

/// The parts of a `.gem`, by name, in order.
fn parts(gem: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = tar::Archive::new(gem);
    archive
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            (name, body)
        })
        .collect()
}

fn part(gem: &[u8], name: &str) -> Vec<u8> {
    parts(gem)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .1
}

/// `checksums.yaml` as RubyGems writes it for these two parts.
fn checksums_yaml(metadata_gz: &[u8], data_tar_gz: &[u8]) -> String {
    format!(
        "---\nSHA256:\n  metadata.gz: {}\n  data.tar.gz: {}\nSHA512:\n  metadata.gz: {}\n  data.tar.gz: {}\n",
        sha256(metadata_gz),
        sha256(data_tar_gz),
        hex(&Sha512::digest(metadata_gz)),
        hex(&Sha512::digest(data_tar_gz)),
    )
}

/// A `.gem` of these parts, laid out as `gem build` lays one out.
fn gem_of(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, body) in parts {
        let mut header = tar::Header::new_ustar();
        header.set_path(name).unwrap();
        header.set_size(body.len() as u64);
        header.set_mode(0o444);
        header.set_cksum();
        builder.append(&header, *body).unwrap();
    }
    builder.into_inner().unwrap()
}

/// One extra entry for a gem's `data.tar.gz`, its name and any link target
/// written into the header as bytes, past the checks `tar::Header` makes,
/// since a malicious archive is exactly what those checks keep out.
enum Extra<'a> {
    File(&'a str),
    Symlink(&'a str, &'a str),
}

/// The fixture `alpha` with `extra` appended to its `data.tar.gz`, and a
/// `checksums.yaml.gz` that matches, so the archive entry is all that is
/// wrong with it.
fn alpha_with(extra: &[Extra]) -> Vec<u8> {
    let alpha = fixture_gem("alpha-1.0.0.gem");
    let data = gunzip(&part(&alpha, "data.tar.gz"));
    // Keep the entries, not the zero blocks that end the archive, and append.
    let used: u64 = tar::Archive::new(&data[..])
        .entries()
        .unwrap()
        .map(|e| 512 + e.unwrap().header().entry_size().unwrap().div_ceil(512) * 512)
        .sum();
    let mut tar = data[..used as usize].to_vec();
    for extra in extra {
        let mut header = tar::Header::new_ustar();
        let (name, link, body): (&str, Option<&str>, &[u8]) = match extra {
            Extra::File(name) => (name, None, b"escaped\n"),
            Extra::Symlink(name, target) => (name, Some(target), b""),
        };
        let ustar = header.as_ustar_mut().unwrap();
        // A name too long for the name field goes in ustar's prefix, split
        // at a `/`, as tar readers join it back.
        let split = if name.len() <= 100 {
            0
        } else {
            name.rfind('/').unwrap()
        };
        let (prefix, rest) = name.split_at(split);
        let rest = rest.strip_prefix('/').filter(|_| split > 0).unwrap_or(rest);
        ustar.prefix[..prefix.len()].copy_from_slice(prefix.as_bytes());
        ustar.name[..rest.len()].copy_from_slice(rest.as_bytes());
        if let Some(target) = link {
            ustar.linkname[..target.len()].copy_from_slice(target.as_bytes());
        }
        header.set_entry_type(if link.is_some() {
            tar::EntryType::Symlink
        } else {
            tar::EntryType::Regular
        });
        header.set_mode(0o644);
        header.set_size(body.len() as u64);
        header.set_mtime(1_700_000_000);
        header.set_cksum();
        tar.extend_from_slice(header.as_bytes());
        tar.extend_from_slice(body);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    let data_tar_gz = gzip(&tar);
    let metadata_gz = part(&alpha, "metadata.gz");
    let checksums = gzip(checksums_yaml(&metadata_gz, &data_tar_gz).as_bytes());
    gem_of(&[
        ("metadata.gz", &metadata_gz),
        ("data.tar.gz", &data_tar_gz),
        ("checksums.yaml.gz", &checksums),
    ])
}

fn alpha() -> Published {
    Published::fixture("alpha", "1.0.0")
}

fn beta() -> Published {
    Published::fixture("beta", "2.1.0")
}

fn gamma() -> Published {
    Published::fixture("gamma", "0.3.0-x86_64-linux")
}

#[test]
fn gems_are_fetched_in_parallel_and_unpacked_as_rubygems_unpacks_them() {
    let (alpha, beta, gamma) = (alpha(), beta(), gamma());
    let server = Server::start();
    server.publish(&[alpha.clone(), beta.clone(), gamma.clone()]);
    // All three must be in flight at once before any is answered.
    server.with(|s| s.hold_gems = 3);
    let home = Home::new("parallel");

    assert_eq!(
        fetch_ok(&home, &server, &[&alpha, &beta, &gamma]),
        [fetched(&alpha), fetched(&beta), fetched(&gamma)]
    );
    assert_eq!(server.with(|s| s.max_gems_in_flight), 3);
    let mut requests = server.take_gem_requests();
    requests.sort();
    assert_eq!(
        requests,
        [
            "/gems/alpha-1.0.0.gem",
            "/gems/beta-2.1.0.gem",
            "/gems/gamma-0.3.0-x86_64-linux.gem"
        ]
    );
    let mut entries = vec![alpha.entry(), beta.entry(), gamma.entry()];
    entries.sort();
    assert_eq!(home.entries(), entries);
    assert_eq!(home.staging(), Vec::<String>::new());

    // The files, with their modes: the executable stays one.
    let src = fixtures().join("src");
    let alpha_dir = home.store().join("gems").join(alpha.entry());
    for file in ["lib/alpha.rb", "exe/alpha"] {
        assert_eq!(
            fs::read(alpha_dir.join("gem").join(file)).unwrap(),
            fs::read(src.join("alpha").join(file)).unwrap(),
            "{file}"
        );
    }
    assert_eq!(mode(&alpha_dir.join("gem/exe/alpha")), 0o755);
    assert_eq!(mode(&alpha_dir.join("gem/lib/alpha.rb")), 0o644);
    // The gemspec, as the `.gem` carries it.
    assert_eq!(
        fs::read(alpha_dir.join("metadata")).unwrap(),
        gunzip(&part(&alpha.body, "metadata.gz"))
    );
    let metadata = fs::read_to_string(alpha_dir.join("metadata")).unwrap();
    assert!(metadata.contains("name: alpha"), "{metadata}");

    // A symlink inside the gem is kept as one.
    let beta_dir = home.store().join("gems").join(beta.entry()).join("gem");
    assert_eq!(
        fs::read_link(beta_dir.join("lib/beta/current.rb")).unwrap(),
        Path::new("version.rb")
    );
    assert_eq!(
        fs::read(beta_dir.join("lib/beta/current.rb")).unwrap(),
        fs::read(src.join("beta/lib/beta/version.rb")).unwrap()
    );

    // The platform is part of the gem's name, and of its entry's.
    let gamma_dir = home.store().join("gems").join(gamma.entry()).join("gem");
    assert!(gamma.entry().starts_with("gamma-0.3.0-x86_64-linux-"));
    assert!(gamma_dir.join("lib/gamma.rb").is_file());
}

#[test]
fn a_gem_already_stored_is_fetched_once_across_projects_and_sources() {
    let (alpha, beta) = (alpha(), beta());
    let server = Server::start();
    server.publish(&[alpha.clone(), beta.clone()]);
    let home = Home::new("reuse");

    assert_eq!(fetch_ok(&home, &server, &[&alpha]), [fetched(&alpha)]);
    assert_eq!(server.take_gem_requests(), ["/gems/alpha-1.0.0.gem"]);

    // Another project, in another directory, wanting alpha and beta: alpha
    // is not asked for again.
    let project = home.0.join("project-b");
    fs::create_dir_all(&project).unwrap();
    let specs = [alpha.spec(), beta.spec()];
    let out = command(&home, &server, &specs)
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().skip(1).collect::<Vec<_>>(),
        [in_store(&alpha), fetched(&beta)]
    );
    assert_eq!(server.take_gem_requests(), ["/gems/beta-2.1.0.gem"]);

    // A second source serving the same bytes, so the same checksum: the
    // entry is the gem's content, wherever it came from.
    let mirror = Server::start();
    mirror.publish(std::slice::from_ref(&alpha));
    assert_eq!(fetch_ok(&home, &mirror, &[&alpha]), [in_store(&alpha)]);
    assert_eq!(mirror.take_gem_requests(), Vec::<String>::new());

    // The same gem asked for twice in one run is one gem.
    assert_eq!(fetch_ok(&home, &server, &[&beta, &beta]), [in_store(&beta)]);
    let mut entries = vec![alpha.entry(), beta.entry()];
    entries.sort();
    assert_eq!(home.entries(), entries);
}

#[test]
fn a_gem_that_does_not_match_its_checksums_is_not_stored() {
    let server = Server::start();
    // The index lists alpha's checksum; the source serves beta's bytes.
    let mut swapped = alpha();
    swapped.body = beta().body;
    // A gem whose own checksums.yaml.gz disagrees with its data.tar.gz,
    // though the index's checksum of the whole file is right.
    let original = fixture_gem("beta-2.1.0.gem");
    let metadata_gz = part(&original, "metadata.gz");
    let data_tar_gz = part(&original, "data.tar.gz");
    let wrong = checksums_yaml(&metadata_gz, b"other bytes");
    let inner = Published::new(
        "beta",
        "2.1.0",
        "beta-2.1.0.gem",
        gem_of(&[
            ("metadata.gz", &metadata_gz),
            ("data.tar.gz", &data_tar_gz),
            ("checksums.yaml.gz", &gzip(wrong.as_bytes())),
        ]),
    );
    server.publish(&[swapped.clone(), inner.clone()]);
    let home = Home::new("checksum");

    let stderr = fetch_err(&home, &server, &[&swapped, &inner]);
    assert!(
        stderr.contains(&format!(
            "spinel: fetch: alpha-1.0.0: the downloaded .gem has SHA-256 {}, but the index lists {}",
            sha256(&swapped.body),
            swapped.listed
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains("spinel: fetch: beta-2.1.0: SHA256 checksum mismatch for data.tar.gz"),
        "{stderr}"
    );
    assert_eq!(home.entries(), Vec::<String>::new());
    assert_eq!(home.staging(), Vec::<String>::new());

    // Once the source serves the right file, it is fetched.
    let good = alpha();
    server.publish(std::slice::from_ref(&good));
    assert_eq!(fetch_ok(&home, &server, &[&good]), [fetched(&good)]);
}

#[test]
fn an_archive_entry_or_symlink_leading_outside_the_gem_is_refused() {
    let home = Home::new("traversal");
    // From `store/tmp/<staging>/gem/`, four levels up is the Spinel home.
    let outside = home.0.join("escaped.rb");
    let outside_abs = outside.to_str().unwrap().to_owned();
    let cases: Vec<(&str, Vec<u8>, String)> = vec![
        (
            "1.0.1",
            alpha_with(&[Extra::File("../../../../escaped.rb")]),
            "archive entry \"../../../../escaped.rb\" would be written outside the gem's directory"
                .to_owned(),
        ),
        (
            "1.0.2",
            alpha_with(&[Extra::File(&outside_abs)]),
            format!("archive entry {outside_abs:?} would be written outside the gem's directory"),
        ),
        (
            "1.0.3",
            alpha_with(&[Extra::Symlink("lib/up", "../../../../..")]),
            "symlink \"lib/up\" points to \"../../../../..\", outside the gem's directory"
                .to_owned(),
        ),
        (
            "1.0.4",
            alpha_with(&[Extra::Symlink("lib/root", "/")]),
            "symlink \"lib/root\" points to \"/\", outside the gem's directory".to_owned(),
        ),
        // Inside by its spelling, outside through the symlink before it.
        (
            "1.0.5",
            alpha_with(&[
                Extra::Symlink("a/b/c/up", "../.."),
                Extra::Symlink("a/b/c/up/out", "../../.."),
            ]),
            "symlink \"a/b/c/up/out\" points to \"../../..\", outside the gem's directory"
                .to_owned(),
        ),
    ];
    let server = Server::start();
    let published: Vec<Published> = cases
        .iter()
        .map(|(version, body, _)| {
            Published::new(
                "alpha",
                version,
                &format!("alpha-{version}.gem"),
                body.clone(),
            )
        })
        .collect();
    server.publish(&published);
    let refs: Vec<&Published> = published.iter().collect();

    let stderr = fetch_err(&home, &server, &refs);
    for (version, _, message) in &cases {
        let want = format!("spinel: fetch: alpha-{version}: {message}");
        assert!(stderr.contains(&want), "want {want}\nstderr: {stderr}");
    }
    assert!(!outside.exists(), "a file was written outside the gem");
    assert_eq!(names(&home.0), ["index", "store"]);
    assert_eq!(home.entries(), Vec::<String>::new());
    assert_eq!(home.staging(), Vec::<String>::new());
}

#[test]
fn an_interrupted_download_leaves_no_store_entry() {
    let alpha = alpha();
    let server = Server::start();
    server.publish(std::slice::from_ref(&alpha));
    let home = Home::new("interrupted");

    // The connection closes halfway through the body.
    server.with(|s| s.cut.insert("/gems/alpha-1.0.0.gem".to_owned()));
    let stderr = fetch_err(&home, &server, &[&alpha]);
    assert!(stderr.contains("/gems/alpha-1.0.0.gem"), "{stderr}");
    assert_eq!(home.entries(), Vec::<String>::new());
    assert_eq!(home.staging(), Vec::<String>::new());

    // The process is killed halfway through the body: what it leaves is in
    // `store/tmp/`, never `store/gems/`.
    server.with(|s| {
        s.cut.clear();
        s.stall = Some("/gems/alpha-1.0.0.gem".to_owned());
    });
    let mut child = command(&home, &server, &[alpha.spec()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(20);
    // Until the server has sent half the body and the client has written
    // some of it.
    let partial = home.store().join("tmp").join(format!("{pid}-0.gem"));
    while !server.with(|s| s.stalled) || fs::metadata(&partial).map_or(0, |m| m.len()) == 0 {
        assert!(Instant::now() < deadline, "the download never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    server.with(|s| s.stall = None);
    assert_eq!(home.entries(), Vec::<String>::new());
    assert_eq!(home.staging(), [format!("{pid}-0.gem")]);

    // A staging directory left by a process that is still running is that
    // process's; one whose process is gone is removed.
    let live = format!("{}-7", std::process::id());
    fs::create_dir_all(home.store().join("tmp").join(&live).join("gem")).unwrap();

    assert_eq!(fetch_ok(&home, &server, &[&alpha]), [fetched(&alpha)]);
    assert_eq!(home.entries(), [alpha.entry()]);
    assert_eq!(home.staging(), [live]);
    assert!(
        home.store()
            .join("gems")
            .join(alpha.entry())
            .join("gem/lib/alpha.rb")
            .is_file()
    );
}

#[test]
fn a_gem_the_index_does_not_list_is_an_error() {
    let alpha = alpha();
    let server = Server::start();
    server.publish(std::slice::from_ref(&alpha));
    let home = Home::new("unlisted");
    let mut other = alpha.clone();
    other.version = "9.9.9";
    let stderr = fetch_err(&home, &server, &[&other]);
    assert!(
        stderr.contains(&format!(
            "spinel: fetch: alpha-9.9.9: {} does not list it",
            server.url
        )),
        "{stderr}"
    );
    assert_eq!(server.take_gem_requests(), Vec::<String>::new());
}
