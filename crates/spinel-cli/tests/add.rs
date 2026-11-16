//! `spinel add` (#85, docs/package-manager.md, "9. `spinel add`") against
//! rubygems.org's own files served on 127.0.0.1: `/versions` and `/info`
//! from the resolver's snapshot (`fixtures/resolver/index/`: Sinatra and
//! everything it reaches), the `.gem`s from the install snapshot
//! (`fixtures/install/gems/`: Sinatra 4.2.1's set and rake). Each Gemfile
//! here is written by the test, so every byte of it outside the added line
//! can be checked. The lines expected are those `bundle add` 4.0.20 writes
//! for the same Gemfile and index, but where a `group` block takes them.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The index and the gems, on a port of its own. A gem the install
/// snapshot does not hold (`json`) is listed by the index and then 404s.
fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    url
}

fn serve(stream: TcpStream) {
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
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("add")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project")).unwrap();
        Sandbox {
            root,
            url: start_server(),
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

    fn add(&self, args: &[&str]) -> Output {
        let mut all = vec!["add"];
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

/// The lockfile's text from `section` to the blank line after it.
fn section(lock: &str, name: &str) -> String {
    lock.lines()
        .skip_while(|l| *l != name)
        .take_while(|l| !l.is_empty())
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn add_sinatra_appends_its_line_and_installs_it() {
    let sandbox = Sandbox::new("sinatra");
    // Comments, single quotes, odd spacing and no newline at the end, all
    // of which stay as they are.
    let before = sandbox.gemfile(
        "# frozen_string_literal: true\n\
         source  'URL'   # the index\n\
         \n\
         gem 'rake',    '~> 13.0'   # tasks",
    );
    let out = sandbox.add(&["sinatra"]);
    assert_eq!(
        ok(&out, "spinel add sinatra"),
        "+ base64 0.3.0\n+ logger 1.7.0\n+ mustermann 3.1.1\n+ rack 3.2.8\n\
         + rack-protection 4.2.1\n+ rack-session 2.1.2\n+ rake 13.4.2\n\
         + sinatra 4.2.1\n+ tilt 2.9.0\n"
    );
    // `bundle add sinatra` on the same Gemfile and index: a newline, which
    // ends the last line, then the line, `~>` the version resolved.
    assert_eq!(
        sandbox.read("Gemfile"),
        format!("{before}\ngem \"sinatra\", \"~> 4.2\"\n")
    );
    let lock = sandbox.read("Gemfile.lock");
    assert_eq!(
        section(&lock, "DEPENDENCIES"),
        "DEPENDENCIES\n  rake (~> 13.0)\n  sinatra (~> 4.2)\n"
    );
    assert!(sandbox.linked().contains(&"sinatra-4.2.1".to_owned()));

    // Installed as `spinel install` would install that Gemfile: a second
    // install changes nothing.
    let out = sandbox.spinel(&["install", "--frozen"]);
    assert_eq!(ok(&out, "spinel install --frozen"), "");
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}

#[test]
fn a_requirement_after_at_is_written_as_given() {
    let sandbox = Sandbox::new("requirement");
    let before = sandbox.gemfile("source \"URL\"\n");
    let out = sandbox.add(&["rake@13.4.2", "tilt@>= 2, < 3", "base64"]);
    assert_eq!(
        ok(&out, "spinel add"),
        "+ base64 0.3.0\n+ rake 13.4.2\n+ tilt 2.9.0\n"
    );
    // `as_list`, each one `dump`ed; a gem without one gets `~>`, below 1.0
    // to three segments.
    assert_eq!(
        sandbox.read("Gemfile"),
        format!(
            "{before}\n\
             gem \"rake\", \"= 13.4.2\"\n\
             gem \"tilt\", \">= 2\", \"< 3\"\n\
             gem \"base64\", \"~> 0.3.0\"\n"
        )
    );
    assert_eq!(
        section(&sandbox.read("Gemfile.lock"), "DEPENDENCIES"),
        "DEPENDENCIES\n  base64 (~> 0.3.0)\n  rake (= 13.4.2)\n  tilt (>= 2, < 3)\n"
    );

    let out = sandbox.add(&["tilt@~>x"]);
    assert_eq!(out.status.code(), Some(15));
    assert_eq!(stderr(&out), "Invalid gem requirement pattern '~>x'\n");
}

#[test]
fn a_group_goes_into_the_block_that_names_it() {
    let sandbox = Sandbox::new("group-block");
    sandbox.gemfile(
        "source \"URL\"\n\
         \n\
         group :development, :test do\n\
         \x20 gem \"rake\" # tasks\n\
         end\n\
         \n\
         group :test do\n\
         \x20   # nothing yet\n\
         end\n\
         # the end\n",
    );
    let out = sandbox.add(&["base64", "--group", "test"]);
    assert_eq!(
        ok(&out, "spinel add --group test"),
        "+ base64 0.3.0\n+ rake 13.4.2\n"
    );
    let out = sandbox.add(&["logger", "--group", "test, development"]);
    assert_eq!(
        ok(&out, "spinel add --group test,development"),
        "+ logger 1.7.0\n"
    );
    // At the end of each block, as indented as its last line or two spaces
    // in from the `group`, and without a `group:` the block already says.
    assert_eq!(
        sandbox.read("Gemfile"),
        "source \"URL\"\n\
             \n\
             group :development, :test do\n\
             \x20 gem \"rake\" # tasks\n\
             \x20 gem \"logger\", \"~> 1.7\"\n\
             end\n\
             \n\
             group :test do\n\
             \x20   # nothing yet\n\
             \x20 gem \"base64\", \"~> 0.3.0\"\n\
             end\n\
             # the end\n"
            .replace("URL", &sandbox.url)
    );
}

#[test]
fn a_group_no_block_names_is_an_option_as_bundle_add_writes_it() {
    let sandbox = Sandbox::new("group-option");
    let before = sandbox.gemfile("source \"URL\"\n\ngroup :test do\n  gem \"rake\"\nend\n");
    ok(
        &sandbox.add(&["base64", "--group", "dev"]),
        "spinel add --group dev",
    );
    ok(
        &sandbox.add(&["logger", "-g", "dev,test"]),
        "spinel add -g dev,test",
    );
    assert_eq!(
        sandbox.read("Gemfile"),
        format!(
            "{before}\ngem \"base64\", \"~> 0.3.0\", group: :dev\n\
             \ngem \"logger\", \"~> 1.7\", groups: [:dev, :test]\n"
        )
    );
}

#[test]
fn a_failed_add_writes_nothing() {
    let sandbox = Sandbox::new("failed");
    sandbox.gemfile("source \"URL\"\n\ngem \"rake\" # tasks\n");
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let gemfile = sandbox.try_read("Gemfile");
    let lock = sandbox.try_read("Gemfile.lock");
    let linked = sandbox.linked();
    let unchanged = |out: &Output, what: &str| {
        assert!(!out.status.success(), "{what} succeeded");
        assert_eq!(sandbox.try_read("Gemfile"), gemfile, "{what}");
        assert_eq!(sandbox.try_read("Gemfile.lock"), lock, "{what}");
        assert_eq!(sandbox.linked(), linked, "{what}");
    };

    // No version meets the requirement.
    let out = sandbox.add(&["base64@< 0.1"]);
    unchanged(&out, "an unmet requirement");
    assert!(stderr(&out).contains("base64"), "{}", stderr(&out));
    // No such gem.
    let out = sandbox.add(&["no-such-gem"]);
    unchanged(&out, "an unknown gem");
    assert!(stderr(&out).contains("no-such-gem"), "{}", stderr(&out));
    // Resolved, then its `.gem` cannot be fetched.
    let out = sandbox.add(&["json"]);
    unchanged(&out, "a gem that does not fetch");
    assert!(stderr(&out).contains("json-"), "{}", stderr(&out));
    // A git repository that is not there (tests/git_path.rs adds one that
    // is).
    let missing = sandbox.root.join("no-such-repo");
    let out = sandbox.add(&["tilt", "--git", &format!("file://{}", missing.display())]);
    unchanged(&out, "--git");
    assert!(
        stderr(&out).starts_with("Git error: command `git clone --bare"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_gem_already_in_the_gemfile() {
    let sandbox = Sandbox::new("present");
    sandbox.gemfile("source \"URL\"\ngem \"rake\"\ngem \"tilt\", \"~> 2.0\"\n");
    ok(&sandbox.spinel(&["install"]), "spinel install");
    let gemfile = sandbox.read("Gemfile");
    let lock = sandbox.read("Gemfile.lock");

    // The same requirement: nothing to add, as `bundle add` drops it, in
    // any group.
    for args in [&["rake"][..], &["rake", "--group", "dev"], &["tilt@~> 2.0"]] {
        let out = sandbox.add(args);
        assert_eq!(ok(&out, &format!("spinel add {args:?}")), "");
        assert_eq!(sandbox.read("Gemfile"), gemfile);
        assert_eq!(sandbox.read("Gemfile.lock"), lock);
    }

    // Another requirement: Bundler's error for the `injected gems`.
    let out = sandbox.add(&["tilt"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        format!(
            "There was an error parsing `injected gems`: You cannot specify the same gem twice \
             with different version requirements.\n\
             You specified: tilt (~> 2.0) and tilt (>= 0). Gem already added\n  \
             from {}:1\n",
            sandbox.project().join("injected gems").display()
        )
    );
    let out = sandbox.add(&["rake@13.4.2"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "You specified: rake (>= 0) and rake (= 13.4.2). If you want to update the gem \
             version, run `spinel update rake`\n"
        ),
        "{}",
        stderr(&out)
    );
    assert_eq!(sandbox.read("Gemfile"), gemfile);
    assert_eq!(sandbox.read("Gemfile.lock"), lock);
}
