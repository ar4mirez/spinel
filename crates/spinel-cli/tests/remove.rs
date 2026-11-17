//! `spinel remove` (#86, docs/package-manager.md, "10. `spinel remove`")
//! against rubygems.org's own files served on 127.0.0.1, as `tests/add.rs`
//! serves them: `/versions` and `/info` from the resolver's snapshot, the
//! `.gem`s from the install snapshot. Each Gemfile is written by the test,
//! so every byte of it outside the lines removed can be checked; the
//! Gemfiles expected are what `bundle remove` 4.0.20 leaves of the same
//! files, but for its last newline, which Spinel keeps as it was.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The index and the gems, on a port of its own, every request a 404 once
/// `down` is set.
fn start_server(down: Arc<AtomicBool>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let down = down.clone();
            std::thread::spawn(move || serve(stream, &down));
        }
    });
    url
}

fn serve(stream: TcpStream, down: &AtomicBool) {
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
        .filter(|_| !down.load(Ordering::SeqCst))
        .filter(|p| !p.split('/').any(|part| part == ".." || part.is_empty()))
        .and_then(|p| match p.strip_prefix("gems/") {
            Some(gem) => fs::read(fixtures().join("install/gems").join(gem)).ok(),
            None => fs::read(fixtures().join("resolver/index").join(p)).ok(),
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

/// A project, a Spinel home of its own and the server.
struct Sandbox {
    root: PathBuf,
    url: String,
    down: Arc<AtomicBool>,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("remove")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        let down = Arc::new(AtomicBool::new(false));
        Sandbox {
            root,
            url: start_server(down.clone()),
            down,
        }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    /// Write the Gemfile, `URL` replaced by the server's.
    fn gemfile(&self, text: &str) -> String {
        let text = text.replace("URL", &self.url);
        fs::write(self.project().join("Gemfile"), &text).unwrap();
        text
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.project().join(name)).unwrap()
    }

    fn try_read(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.project().join(name)).ok()
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

    fn remove(&self, args: &[&str]) -> Output {
        let mut all = vec!["remove"];
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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `lock` without the gems `names`: each one's spec and the dependencies
/// listed under it, its `DEPENDENCIES` line and its checksum.
fn without(lock: &str, names: &[&str]) -> String {
    let named = |line: &str, indent: usize| {
        line.len() > indent
            && line[..indent].bytes().all(|b| b == b' ')
            && !line[indent..].starts_with(' ')
            && names.iter().any(|n| {
                line[indent..]
                    .strip_prefix(n)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
            })
    };
    let mut out = String::new();
    let mut skipping = false;
    let mut section = "";
    for line in lock.lines() {
        if !line.starts_with(' ') {
            section = if line.is_empty() { section } else { line };
        }
        if skipping && line.starts_with("      ") {
            continue;
        }
        skipping = false;
        let gone = match section {
            "GEM" => named(line, 4),
            "DEPENDENCIES" | "CHECKSUMS" => named(line, 2),
            _ => false,
        };
        if gone {
            skipping = section == "GEM";
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn remove_takes_the_lines_out_and_the_orphans_with_them() {
    let sandbox = Sandbox::new("sinatra");
    // Comments, single quotes, odd spacing, a call across two lines, a
    // block and no newline at the end.
    sandbox.gemfile(
        "# frozen_string_literal: true\n\
         source  'URL'   # the index\n\
         \n\
         gem 'rake',    '~> 13.0'   # tasks\n\
         gem \"sinatra\",\n\
         \x20 \"~> 4.2\"     # web\n\
         gem 'rack-session'\n\
         \n\
         group :test do\n\
         \x20 gem 'tilt' # templates\n\
         end\n\
         # the end",
    );
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let lock = sandbox.read("Gemfile.lock");

    // With the index gone: the lockfile is all it reads.
    sandbox.down.store(true, Ordering::SeqCst);
    let out = sandbox.remove(&["sinatra", "tilt"]);
    assert_eq!(
        ok(&out, "spinel remove sinatra tilt"),
        format!(
            "Removing gems from {}\n\
             sinatra (~> 4.2) was removed.\n\
             tilt was removed.\n\
             - logger 1.7.0\n- mustermann 3.1.1\n- rack-protection 4.2.1\n\
             - sinatra 4.2.1\n- tilt 2.9.0\n",
            sandbox.project().join("Gemfile").display()
        )
    );
    // Their lines and the block they left empty; every other byte stays.
    assert_eq!(
        sandbox.read("Gemfile"),
        "# frozen_string_literal: true\n\
         source  'URL'   # the index\n\
         \n\
         gem 'rake',    '~> 13.0'   # tasks\n\
         gem 'rack-session'\n\
         \n\
         # the end"
            .replace("URL", &sandbox.url)
    );
    // Sinatra and what only it needed leave the lockfile; rack-session's
    // rack and base64 stay at their locked versions, and nothing else moves.
    assert_eq!(
        sandbox.read("Gemfile.lock"),
        without(
            &lock,
            &["sinatra", "tilt", "logger", "mustermann", "rack-protection"]
        )
    );
    assert_eq!(
        sandbox.linked(),
        [
            "base64-0.3.0",
            "rack-3.2.8",
            "rack-session-2.1.2",
            "rake-13.4.2"
        ]
    );

    // What `spinel install` makes of that Gemfile: nothing to change.
    let out = sandbox.spinel(&["install", "--frozen"]);
    assert_eq!(ok(&out, "spinel install --frozen"), "Nothing to change\n");
}

#[test]
fn a_gem_not_in_the_gemfile_is_bundlers_error() {
    let sandbox = Sandbox::new("unknown");
    sandbox.gemfile("source \"URL\"\ngem \"rake\"\n");
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let gemfile = sandbox.try_read("Gemfile");
    let lock = sandbox.try_read("Gemfile.lock");

    let out = sandbox.remove(&["rake", "tilt"]);
    assert_eq!(out.status.code(), Some(4));
    let path = sandbox.project().join("Gemfile");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("Removing gems from {}\n", path.display())
    );
    assert_eq!(
        stderr(&out),
        format!(
            "`tilt` is not specified in {} so it could not be removed.\n",
            path.display()
        )
    );
    // Named twice: the second time, it is no longer listed.
    let out = sandbox.remove(&["rake", "rake"]);
    assert_eq!(out.status.code(), Some(4));

    let out = sandbox.remove(&[]);
    assert_eq!(out.status.code(), Some(15));
    assert_eq!(stderr(&out), "Please specify gems to remove.\n");
    assert_eq!(sandbox.try_read("Gemfile"), gemfile);
    assert_eq!(sandbox.try_read("Gemfile.lock"), lock);
}

#[test]
fn a_line_that_cannot_go_alone_writes_nothing() {
    let sandbox = Sandbox::new("shared-line");
    sandbox.gemfile(
        "source \"URL\"\n\
         gem \"rake\"; gem \"base64\"\n\
         gem \"tilt\" if gem(\"logger\")\n",
    );
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let gemfile = sandbox.try_read("Gemfile");
    let lock = sandbox.try_read("Gemfile.lock");
    let linked = sandbox.linked();
    let path = sandbox.project().join("Gemfile");

    // Sharing its line with another gem: left, as `bundle remove` says.
    let out = sandbox.remove(&["base64"]);
    assert_eq!(
        ok(&out, "spinel remove base64"),
        format!(
            "Removing gems from {}\n\
             base64 could not be removed.\n\
             No gems were removed from the gemfile.\n\
             Nothing to change\n",
            path.display()
        )
    );
    // Its line would take another gem with it.
    let out = sandbox.remove(&["tilt"]);
    assert_eq!(out.status.code(), Some(15));
    assert_eq!(
        stderr(&out),
        "Gems could not be removed. logger (>= 0) would also have been removed. \
         Bundler cannot continue.\n"
    );
    assert_eq!(sandbox.try_read("Gemfile"), gemfile);
    assert_eq!(sandbox.try_read("Gemfile.lock"), lock);
    assert_eq!(sandbox.linked(), linked);
}

#[test]
fn a_failed_install_writes_nothing() {
    let sandbox = Sandbox::new("failed");
    sandbox.gemfile("source \"URL\"\n\ngem \"rake\" # tasks\ngem \"base64\"\n");
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let gemfile = sandbox.try_read("Gemfile");
    let lock = sandbox.try_read("Gemfile.lock");

    // The store emptied and the index gone: rake cannot be linked again.
    fs::remove_dir_all(sandbox.root.join("home")).unwrap();
    sandbox.down.store(true, Ordering::SeqCst);
    let out = sandbox.remove(&["base64"]);
    assert!(!out.status.success(), "spinel remove succeeded");
    assert!(stderr(&out).contains("rake-13.4.2"), "{}", stderr(&out));
    assert_eq!(sandbox.try_read("Gemfile"), gemfile);
    assert_eq!(sandbox.try_read("Gemfile.lock"), lock);
}
