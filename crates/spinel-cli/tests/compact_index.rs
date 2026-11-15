//! The compact index client against a local server (#77,
//! docs/package-manager.md, "Index client"): a full fetch, ETag revalidation,
//! the incremental `Range` update checked byte for byte against a full
//! refetch, the stale and truncated caches Bundler's updater recovers from,
//! and the cap of 32 requests in flight. Nothing here touches the network
//! beyond 127.0.0.1.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use md5::Md5;
use sha2::{Digest as _, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/compact_index")
        .join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `/versions` before and after the server appends to it.
fn versions_before_and_after() -> (Vec<u8>, Vec<u8>) {
    let before = fixture("versions");
    let mut after = before.clone();
    after.extend(fixture("versions.tail"));
    (before, after)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One request as the server saw it.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    range: Option<String>,
    if_none_match: Option<String>,
    status: u16,
}

#[derive(Default)]
struct State {
    files: HashMap<String, Vec<u8>>,
    /// Send no `Repr-Digest`.
    no_digest: bool,
    /// Send a `Repr-Digest` that matches nothing.
    wrong_digest: bool,
    /// Answer a range request with the whole file, as a server may.
    ignore_range: bool,
    /// Hold each `/info` request until this many are in flight, or until
    /// `expected_info` have arrived, so the count reaches the client's cap.
    hold: usize,
    expected_info: usize,
    arrived_info: usize,
    in_flight: usize,
    max_in_flight: usize,
    seen: Vec<Seen>,
}

/// A compact index on 127.0.0.1: `GET` of the files in `State`, with strong
/// ETags, `Repr-Digest`, `If-None-Match` and single `bytes=N-` ranges.
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
        f(&mut self.state.0.lock().unwrap())
    }

    fn put(&self, path: &str, body: &[u8]) {
        self.with(|s| s.files.insert(path.to_owned(), body.to_vec()));
    }

    /// The requests since the last call.
    fn take_seen(&self) -> Vec<Seen> {
        self.with(|s| std::mem::take(&mut s.seen))
    }
}

fn etag_of(body: &[u8]) -> String {
    hex(&Sha256::digest(body))[..20].to_owned()
}

fn serve(stream: TcpStream, shared: &(Mutex<State>, Condvar)) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.trim_end().split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    let (lock, wake) = shared;
    let is_info = path.starts_with("/info/");
    if is_info {
        let mut state = lock.lock().unwrap();
        state.in_flight += 1;
        state.arrived_info += 1;
        state.max_in_flight = state.max_in_flight.max(state.in_flight);
        wake.notify_all();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.hold > 0
            && state.in_flight < state.hold
            && state.arrived_info < state.expected_info
            && Instant::now() < deadline
        {
            state = wake
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        drop(state);
        // Long enough that a request past the cap would overlap these.
        std::thread::sleep(Duration::from_millis(30));
    }

    let mut state = lock.lock().unwrap();
    let range = headers.get("range").cloned();
    let if_none_match = headers.get("if-none-match").cloned();
    let (status, extra, body) = match state.files.get(&path) {
        None => (404, Vec::new(), b"not found".to_vec()),
        Some(file) => {
            let etag = format!("\"{}\"", etag_of(file));
            let digest = if state.wrong_digest {
                Sha256::digest(b"something else")
            } else {
                Sha256::digest(file)
            };
            let mut extra = vec![format!("ETag: {etag}")];
            if !state.no_digest {
                // Quoted, as rubygems.org sends it, and as `Digest` too.
                let b64 = base64::engine::general_purpose::STANDARD.encode(digest);
                extra.push(format!("Repr-Digest: sha-256=\"{b64}\""));
                extra.push(format!("Digest: sha-256=\"{b64}\""));
            }
            let from = range
                .as_deref()
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|r| r.strip_suffix('-'))
                .and_then(|n| n.parse::<usize>().ok());
            if if_none_match.as_deref() == Some(etag.as_str()) {
                (304, extra, Vec::new())
            } else if let (Some(from), false) = (from, state.ignore_range) {
                if from >= file.len() {
                    (
                        416,
                        vec![format!("Content-Range: bytes */{}", file.len())],
                        Vec::new(),
                    )
                } else {
                    extra.push(format!(
                        "Content-Range: bytes {from}-{}/{}",
                        file.len() - 1,
                        file.len()
                    ));
                    (206, extra, file[from..].to_vec())
                }
            } else {
                (200, extra, file.clone())
            }
        }
    };
    state.seen.push(Seen {
        path,
        range,
        if_none_match,
        status,
    });
    if is_info {
        state.in_flight -= 1;
    }
    drop(state);

    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        304 => "Not Modified",
        404 => "Not Found",
        _ => "Range Not Satisfiable",
    };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    for header in extra {
        response.push_str(&header);
        response.push_str("\r\n");
    }
    if status != 304 {
        response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    response.push_str("\r\n");
    let mut stream = stream;
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

/// A Spinel home for one test, under Cargo's scratch directory for tests.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("compact-index-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    /// The cache directory for `server`, as the client names it.
    fn cache(&self, server: &Server) -> PathBuf {
        let authority = server.url.strip_prefix("http://").unwrap();
        self.0.join("index").join(authority)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn index(home: &Home, server: &Server, args: &[&str]) -> Output {
    let mut command = Command::new(SPINEL);
    command
        .arg("index")
        .arg("--source")
        .arg(&server.url)
        .args(args)
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
    command.output().expect("spinel runs")
}

/// Run `spinel index`, expecting success, and return its report lines after
/// the first, which names the cache directory.
fn index_ok(home: &Home, server: &Server, args: &[&str]) -> Vec<String> {
    let out = index(home, server, args);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "spinel index failed\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut lines = stdout.lines().map(str::to_owned);
    assert_eq!(
        lines.next(),
        Some(format!("index: {}", home.cache(server).display())),
        "the first line names the cache"
    );
    lines.collect()
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn versions_fetch_revalidate_and_append_byte_identical_to_a_full_refetch() {
    let (before, after) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &before);
    let home = Home::new("append");
    let cache = home.cache(&server);

    // Cold: the whole file, no range and no ETag to send.
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: fetched {} bytes", before.len())]
    );
    let seen = server.take_seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0].range.as_deref(), seen[0].if_none_match.as_deref()),
        (None, None)
    );
    assert_eq!(read(&cache.join("versions")), before);
    assert_eq!(
        read(&cache.join("versions.etag")),
        etag_of(&before).as_bytes()
    );

    // Warm and unchanged: one ranged, conditional request, and a 304.
    assert_eq!(index_ok(&home, &server, &[]), ["versions: not modified"]);
    let seen = server.take_seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].range, Some(format!("bytes={}-", before.len() - 1)));
    assert_eq!(
        seen[0].if_none_match,
        Some(format!("\"{}\"", etag_of(&before)))
    );
    assert_eq!(seen[0].status, 304);

    // The server appended: one 206, and only the new bytes travel.
    server.put("/versions", &after);
    let tail = after.len() - before.len();
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: appended {tail} bytes")]
    );
    let seen = server.take_seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].status, 206);
    assert_eq!(
        read(&cache.join("versions.etag")),
        etag_of(&after).as_bytes()
    );

    // The same file a fresh cache fetches whole.
    let fresh = Home::new("append-fresh");
    assert_eq!(
        index_ok(&fresh, &server, &[]),
        [format!("versions: fetched {} bytes", after.len())]
    );
    assert_eq!(
        read(&cache.join("versions")),
        read(&fresh.cache(&server).join("versions"))
    );
    assert_eq!(read(&cache.join("versions")), after);
    assert!(
        fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")),
        "no temporary file is left behind"
    );
}

/// Seed a cache with `versions` content and an ETag, as a previous run left it.
fn seed(home: &Home, server: &Server, versions: &[u8], etag: &str) {
    let cache = home.cache(server);
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("versions"), versions).unwrap();
    fs::write(cache.join("versions.etag"), etag).unwrap();
}

#[test]
fn a_truncated_cache_is_completed_by_the_range() {
    let (before, after) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &after);
    let home = Home::new("truncated");
    // Cut mid-line, as an interrupted write without the rename would.
    let cut = before.len() - 17;
    seed(&home, &server, &before[..cut], &etag_of(&before));
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: appended {} bytes", after.len() - cut)]
    );
    assert_eq!(server.take_seen().len(), 1);
    assert_eq!(read(&home.cache(&server).join("versions")), after);
}

#[test]
fn a_corrupt_cache_fails_the_digest_and_is_fetched_again() {
    let (before, after) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &after);
    let home = Home::new("corrupt");
    let mut corrupt = before.clone();
    corrupt[40] ^= 0x20;
    seed(&home, &server, &corrupt, &etag_of(&before));
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: fetched {} bytes", after.len())]
    );
    let seen = server.take_seen();
    let statuses: Vec<(u16, bool)> = seen.iter().map(|s| (s.status, s.range.is_some())).collect();
    assert_eq!(
        statuses,
        [(206, true), (200, false)],
        "the append, then the whole file"
    );
    assert_eq!(read(&home.cache(&server).join("versions")), after);
}

#[test]
fn without_a_digest_nothing_is_appended() {
    let (before, after) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &after);
    server.with(|s| s.no_digest = true);
    let home = Home::new("no-digest");
    seed(&home, &server, &before, &etag_of(&before));
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: fetched {} bytes", after.len())]
    );
    let statuses: Vec<u16> = server.take_seen().iter().map(|s| s.status).collect();
    assert_eq!(statuses, [206, 200]);
    assert_eq!(read(&home.cache(&server).join("versions")), after);
    // The response carried no digest to check against, but an ETag: stored.
    assert_eq!(
        read(&home.cache(&server).join("versions.etag")),
        etag_of(&after).as_bytes()
    );
}

#[test]
fn a_server_that_ignores_the_range_replaces_the_cache() {
    let (before, after) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &after);
    server.with(|s| s.ignore_range = true);
    let home = Home::new("ignore-range");
    seed(&home, &server, &before, &etag_of(&before));
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: fetched {} bytes", after.len())]
    );
    let seen = server.take_seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].status, 200);
    assert_eq!(read(&home.cache(&server).join("versions")), after);
}

#[test]
fn a_cache_longer_than_the_file_starts_over() {
    let (before, after) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &before);
    let home = Home::new("longer");
    seed(&home, &server, &after, &etag_of(&after));
    assert_eq!(
        index_ok(&home, &server, &[]),
        [format!("versions: fetched {} bytes", before.len())]
    );
    let statuses: Vec<u16> = server.take_seen().iter().map(|s| s.status).collect();
    assert_eq!(statuses, [416, 200]);
    assert_eq!(read(&home.cache(&server).join("versions")), before);
}

#[test]
fn a_whole_file_that_fails_its_digest_is_never_written() {
    let (before, _) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &before);
    server.with(|s| s.wrong_digest = true);
    let home = Home::new("wrong-digest");
    let out = index(&home, &server, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not match the digest"), "{stderr}");
    assert!(!home.cache(&server).join("versions").exists());
}

#[test]
fn a_missing_index_is_an_error() {
    let server = Server::start();
    let home = Home::new("missing");
    let out = index(&home, &server, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("/versions: HTTP 404"), "{stderr}");
}

#[test]
fn names_are_fetched_and_revalidated() {
    let (before, _) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &before);
    server.put("/names", b"---\nrack\nrails\n");
    let home = Home::new("names");
    assert_eq!(
        index_ok(&home, &server, &["--names"]),
        [
            format!("versions: fetched {} bytes", before.len()),
            "names: fetched 15 bytes".to_owned()
        ]
    );
    assert_eq!(
        index_ok(&home, &server, &["--names"]),
        ["versions: not modified", "names: not modified"]
    );
    assert_eq!(
        read(&home.cache(&server).join("names")),
        b"---\nrack\nrails\n"
    );
}

/// `/versions` listing `gems` with the MD5 of each one's info file.
fn versions_for(infos: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut versions = b"created_at: 2026-11-01T00:00:00Z\n---\n".to_vec();
    for (gem, info) in infos {
        versions.extend(format!("{gem} 1.0.0 {}\n", hex(&Md5::digest(info))).into_bytes());
    }
    versions
}

#[test]
fn info_requests_are_capped_at_32_in_flight_and_skipped_when_current() {
    const GEMS: usize = 100;
    let infos: Vec<(String, Vec<u8>)> = (0..GEMS)
        .map(|i| {
            let gem = format!("gem-{i:03}");
            let info = format!(
                "---\n1.0.0 |checksum:{}\n",
                hex(&Sha256::digest(gem.as_bytes()))
            );
            (gem, info.into_bytes())
        })
        .collect();
    let server = Server::start();
    let versions = versions_for(&infos);
    server.put("/versions", &versions);
    for (gem, info) in &infos {
        server.put(&format!("/info/{gem}"), info);
    }
    server.with(|s| {
        s.hold = 32;
        s.expected_info = GEMS;
    });
    let home = Home::new("info");
    let mut args: Vec<&str> = infos.iter().map(|(gem, _)| gem.as_str()).collect();
    args.push("not-a-gem-here");
    // Named twice, reported and fetched once.
    args.push("gem-000");

    let report = index_ok(&home, &server, &args);
    assert_eq!(report.len(), GEMS + 2);
    for ((gem, info), line) in infos.iter().zip(&report[1..]) {
        assert_eq!(line, &format!("info/{gem}: fetched {} bytes", info.len()));
        assert_eq!(&read(&home.cache(&server).join("info").join(gem)), info);
    }
    assert_eq!(report[GEMS + 1], "info/not-a-gem-here: not in the index");
    assert_eq!(
        server.with(|s| s.max_in_flight),
        32,
        "the cap is reached and never passed"
    );
    let seen = server.take_seen();
    assert_eq!(
        seen.iter().filter(|s| s.path.starts_with("/info/")).count(),
        GEMS
    );

    // Warm: every info file has the checksum /versions lists, so the only
    // request is the 304 for /versions.
    server.with(|s| s.hold = 0);
    let report = index_ok(&home, &server, &args[..GEMS]);
    assert_eq!(report[0], "versions: not modified");
    assert!(
        report[1..].iter().all(|line| line.ends_with(": current")),
        "{report:?}"
    );
    assert_eq!(server.take_seen().len(), 1);

    // One gem gains a version: /versions and that info file are appended to,
    // and the rest stay current.
    let mut updated = infos[7].1.clone();
    updated.extend(b"1.1.0 |checksum:abc\n");
    server.put("/info/gem-007", &updated);
    let mut grown = versions.clone();
    grown.extend(format!("gem-007 1.1.0 {}\n", hex(&Md5::digest(&updated))).into_bytes());
    server.put("/versions", &grown);
    let report = index_ok(&home, &server, &["gem-007", "gem-008"]);
    assert_eq!(
        report,
        [
            format!("versions: appended {} bytes", grown.len() - versions.len()),
            format!(
                "info/gem-007: appended {} bytes",
                updated.len() - infos[7].1.len()
            ),
            "info/gem-008: current".to_owned(),
        ]
    );
    assert_eq!(read(&home.cache(&server).join("info/gem-007")), updated);
    let seen = server.take_seen();
    assert_eq!(
        seen.iter().map(|s| s.status).collect::<Vec<_>>(),
        [206, 206]
    );
}

#[test]
fn a_gem_name_that_could_leave_the_cache_is_refused() {
    let (before, _) = versions_before_and_after();
    let server = Server::start();
    server.put("/versions", &before);
    let home = Home::new("bad-name");
    let out = index(&home, &server, &["../escape"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("\"../escape\": not a gem name"), "{stderr}");
}

/// The real rubygems.org, opt-in: `SPINEL_NETWORK_TESTS=1 cargo test -p
/// spinel-cli --test compact_index -- --ignored`. Two runs: a full fetch,
/// then a revalidation or an append.
#[test]
#[ignore = "talks to rubygems.org; run with SPINEL_NETWORK_TESTS=1 and --ignored"]
fn rubygems_org() {
    if std::env::var_os("SPINEL_NETWORK_TESTS").is_none() {
        return;
    }
    let home = Home::new("rubygems");
    let run = || {
        let out = Command::new(SPINEL)
            .args(["index", "rack"])
            .env("SPINEL_HOME", &home.0)
            .output()
            .expect("spinel runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let first = run();
    assert!(first.contains("versions: fetched"), "{first}");
    assert!(first.contains("info/rack: fetched"), "{first}");
    let second = run();
    assert!(
        second.contains("versions: not modified") || second.contains("versions: appended"),
        "{second}"
    );
    assert!(home.0.join("index/rubygems.org/versions").is_file());
}
