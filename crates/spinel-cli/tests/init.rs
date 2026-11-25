//! `spinel init` scaffolds a project and never clobbers one (#67, docs/cli.md).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// A fresh empty directory for one test, removed again when it drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("spinel-init-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn init_in(dir: &Path) -> Output {
    let out = Command::new(SPINEL)
        .arg("init")
        .current_dir(dir)
        .output()
        .expect("spinel runs");
    assert!(
        out.status.success(),
        "spinel init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The engine version the banner reports, which `.spinel-version` must name.
fn engine_version() -> String {
    let out = Command::new(SPINEL)
        .arg("--version")
        .output()
        .expect("runs");
    let banner = String::from_utf8(out.stdout).expect("utf-8");
    banner
        .strip_prefix("spinel ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("unexpected banner: {banner:?}"))
        .to_owned()
}

#[test]
fn init_in_an_empty_directory_writes_each_file() {
    let scratch = Scratch::new("empty");
    let dir = &scratch.0;
    init_in(dir);

    let gemfile = read(dir.join("Gemfile"));
    assert!(
        gemfile.contains("source \"https://rubygems.org\""),
        "{gemfile}"
    );

    assert_eq!(
        read(dir.join(".spinel-version")),
        format!("{}\n", engine_version())
    );

    assert!(dir.join("test").is_dir(), "test/ is created");

    let gitignore = read(dir.join(".gitignore"));
    assert_eq!(gitignore, ".spinel/\n.bundle/\n");

    let mut entries: Vec<String> = fs::read_dir(dir)
        .expect("readable")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        [".gitignore", ".spinel-version", "Gemfile", "test"]
    );
}

/// mise, rbenv, asdf and chruby read `.ruby-version` as a request for a CRuby,
/// so a `spinel-<version>` there sends `mise install` looking for a CRuby
/// named that, which fails (#902). A fresh project must not have one.
#[test]
fn init_writes_no_ruby_version() {
    let scratch = Scratch::new("no-ruby-version");
    let dir = &scratch.0;
    let out = init_in(dir);

    assert!(
        !dir.join(".ruby-version").exists(),
        "spinel init must not write .ruby-version"
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(!printed.contains(".ruby-version"), "{printed}");
}

#[test]
fn init_keeps_an_existing_gemfile() {
    let scratch = Scratch::new("gemfile");
    let dir = &scratch.0;
    let mine = "source \"https://example.org\"\ngem \"sinatra\"\n";
    fs::write(dir.join("Gemfile"), mine).expect("write");

    let out = init_in(dir);

    assert_eq!(read(dir.join("Gemfile")), mine, "the Gemfile is untouched");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed
            .lines()
            .any(|l| l.contains("exists") && l.ends_with("Gemfile")),
        "it says the Gemfile was kept: {printed}"
    );
    // The rest of the scaffold still lands.
    assert!(dir.join(".spinel-version").is_file());
    assert!(dir.join("test").is_dir());
    assert!(dir.join(".gitignore").is_file());
}

#[test]
fn init_keeps_existing_version_files_and_test_dir() {
    let scratch = Scratch::new("ruby-version");
    let dir = &scratch.0;
    fs::write(dir.join(".ruby-version"), "3.4.1\n").expect("write");
    fs::write(dir.join(".spinel-version"), "0.0.1\n").expect("write");
    fs::create_dir(dir.join("test")).expect("mkdir");
    fs::write(dir.join("test/a_test.rb"), "# mine\n").expect("write");

    init_in(dir);

    assert_eq!(read(dir.join(".ruby-version")), "3.4.1\n");
    assert_eq!(read(dir.join(".spinel-version")), "0.0.1\n");
    assert_eq!(read(dir.join("test/a_test.rb")), "# mine\n");
}

#[test]
fn init_appends_to_a_gitignore_without_the_entry() {
    let scratch = Scratch::new("gitignore");
    let dir = &scratch.0;
    // No trailing newline: the entry must still land on a line of its own.
    fs::write(dir.join(".gitignore"), "/tmp\n*.log").expect("write");

    init_in(dir);

    assert_eq!(
        read(dir.join(".gitignore")),
        "/tmp\n*.log\n.spinel/\n.bundle/\n"
    );
}

#[test]
fn init_leaves_a_gitignore_that_already_has_the_entry() {
    let scratch = Scratch::new("gitignore-has");
    let dir = &scratch.0;
    fs::write(dir.join(".gitignore"), "/.spinel\n*.log\n.bundle\n").expect("write");

    init_in(dir);

    assert_eq!(read(dir.join(".gitignore")), "/.spinel\n*.log\n.bundle\n");
}

/// `spinel install` writes `.bundle/config` (#900), so `init` ignores `.bundle/`
/// too, appended only when no line decides about it: a negation is the
/// project's choice to commit it, and stays the last word.
#[test]
fn init_appends_only_the_missing_bundle_entry_and_respects_a_negation() {
    let scratch = Scratch::new("gitignore-bundle");
    let dir = &scratch.0;
    fs::write(dir.join(".gitignore"), ".spinel/\n").expect("write");
    init_in(dir);
    assert_eq!(read(dir.join(".gitignore")), ".spinel/\n.bundle/\n");
    // A second run adds nothing.
    init_in(dir);
    assert_eq!(read(dir.join(".gitignore")), ".spinel/\n.bundle/\n");

    fs::write(dir.join(".gitignore"), "!.bundle/\n").expect("write");
    init_in(dir);
    assert_eq!(read(dir.join(".gitignore")), "!.bundle/\n.spinel/\n");
}

#[test]
fn init_twice_changes_nothing_the_second_time() {
    let scratch = Scratch::new("twice");
    let dir = &scratch.0;
    init_in(dir);
    let before: Vec<String> = ["Gemfile", ".spinel-version", ".gitignore"]
        .iter()
        .map(|f| read(dir.join(f)))
        .collect();

    let out = init_in(dir);

    let after: Vec<String> = ["Gemfile", ".spinel-version", ".gitignore"]
        .iter()
        .map(|f| read(dir.join(f)))
        .collect();
    assert_eq!(before, after);
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed
            .lines()
            .all(|l| l.trim_start().starts_with("exists")),
        "{printed}"
    );
}
