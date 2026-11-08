//! The vendored `webrick` (#54): a server on 127.0.0.1, on a port the system
//! picks, serving Net::HTTP requests over one kept-alive connection — a proc
//! servlet, a form POST with a cookie in the answer, a static file by GET and
//! HEAD, a 404 and a raised status — then shutting down. ruby/spec has no
//! `library/webrick`: it is a gem, not the standard library. Expected output
//! is what ruby 4.0.9 prints for the same fixture with `-I stdlib`.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

const EXPECTED: &str = r#"listening: ["127.0.0.1"] true
["200", "text/plain", "20", "Keep-Alive", "hello spinel via GET"]
WEBrick/1.9.2
["201", "application/json", "yes", "k=v", "application/x-www-form-urlencoded|a=1&b=two|[[\"a\", \"1\"], [\"b\", \"two\"]]"]
["200", "text/plain", "12", "static file\n"]
["200", "12", nil]
["404", "text/html; charset=ISO-8859-1", "Not Found"]
["403", "Forbidden"]
GET /hello?name=spinel 200 20
POST /echo 201 70
GET /index.txt 200 12
HEAD /index.txt 200 0
GET /missing 404 278
GET /boom 403 259
status: Stop
listeners: 0
ERROR '/missing' not found.
ERROR no
"#;

#[test]
fn webrick_serves_http_requests() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webrick/serve.rb");
    let mut child = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    // The fixture shuts its server down in an `ensure`. If even that hangs,
    // the process is killed within a minute, which closes the listener.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(Box::new(child.stdout.take().expect("piped stdout")));
    let stderr = drain(Box::new(child.stderr.take().expect("piped stderr")));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("spinel waits") {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    let status = status.unwrap_or_else(|| panic!("timed out\nstdout: {stdout}\nstderr: {stderr}"));
    assert!(status.success(), "stderr: {stderr}");
    assert_eq!(stdout, EXPECTED, "stderr: {stderr}");
}

/// What WEBrick reaches for as it loads: `etc` loads, and `RbConfig.ruby`
/// names the interpreter, which for Spinel is the binary itself.
#[test]
fn etc_loads_and_rbconfig_ruby_is_the_binary() {
    let output = Command::new(SPINEL)
        .args([
            "-e",
            "require 'etc'; puts Etc::VERSION; puts RbConfig.ruby; puts RbConfig::CONFIG['bindir']",
        ])
        .output()
        .expect("spinel runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    let binary = std::fs::canonicalize(SPINEL).expect("the binary exists");
    let bindir = binary.parent().expect("a directory holds it");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("1.4.6\n{}\n{}\n", binary.display(), bindir.display())
    );
}
