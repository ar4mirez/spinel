//! The bytecode cache (#40, docs/cli.md, "Bytecode cache and require map"):
//! inside a project, `require` keeps each file's compiled bytecode in
//! `.spinel/bytecode/<sha256 of source>.sbc` and reads it back on the next
//! run instead of parsing. A run that reads the cache prints exactly what a
//! run without one prints — warnings, `__FILE__`, `__LINE__`, backtraces —
//! and a cache file that is corrupt, truncated, written by another build or
//! for another source is parsed past, never trusted.
//!
//! Each test runs the fixture in a copy of its own, since the cache writes
//! into the project.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// The fixture copied to a fresh directory of the test's own, as `Dir.pwd`
/// spells it. Not yet a project: [`make_project`] makes it one.
fn copy_fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spinel-bytecode-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bytecode_cache"),
        &dir,
    );
    fs::canonicalize(&dir).expect("the copy exists")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("a directory is made");
    for entry in fs::read_dir(from).expect("the fixture reads") {
        let entry = entry.expect("a directory entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a file copies");
        }
    }
}

/// What `spinel install` leaves, as far as the setup shim looks.
fn make_project(dir: &Path) {
    fs::create_dir_all(dir.join(".spinel/gem_home")).expect("the gem home is made");
}

fn spinel(dir: &Path, args: &[&str]) -> Output {
    Command::new(SPINEL)
        .args(args)
        .current_dir(dir)
        .env_remove("GEM_HOME")
        .env_remove("GEM_PATH")
        .env_remove("RUBYOPT")
        .output()
        .expect("spinel runs")
}

/// Status, stdout and stderr, for comparing two runs whole.
fn transcript(out: &Output) -> (Option<i32>, String, String) {
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn cache_dir(dir: &Path) -> PathBuf {
    dir.join(".spinel/bytecode")
}

/// The cache files, by name, and nothing else in the directory — no
/// temporary file a write left behind.
fn cache_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(cache_dir(dir)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The cache file `source` is kept in.
fn cache_file(dir: &Path, source: &Path) -> PathBuf {
    let digest = Sha256::digest(fs::read(source).expect("the source reads"));
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    cache_dir(dir).join(format!("{hex}.sbc"))
}

/// The cache files every program in a project starts with: the stdlib
/// files the gem prelude requires (#319), RubyGems and `did_you_mean`,
/// cached as any required file is. Found by running an empty program in a
/// project of its own.
fn boot_files() -> Vec<String> {
    // One directory per call: the tests run at once.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "spinel-bytecode-boot-{}-{call}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    make_project(&dir);
    fs::write(dir.join("empty.rb"), "").expect("the program is written");
    let out = spinel(&dir, &["run", "empty.rb"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let names = cache_files(&dir);
    let _ = fs::remove_dir_all(&dir);
    names
}

/// The cache file for each of the fixture's three required files, and the
/// [`boot_files`].
fn expected_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = ["lib/greeting.rb", "lib/failing.rb", "lib/conditional.rb"]
        .iter()
        .map(|file| {
            cache_file(dir, &dir.join(file))
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.extend(boot_files());
    names.sort();
    names
}

const LONG_AGO: Duration = Duration::from_secs(1_000_000_000);

/// Date every cache file far in the past, so a run that rewrites one shows.
fn backdate(dir: &Path) {
    for name in cache_files(dir) {
        let file = fs::File::options()
            .write(true)
            .open(cache_dir(dir).join(name))
            .expect("a cache file opens");
        file.set_modified(SystemTime::UNIX_EPOCH + LONG_AGO)
            .expect("its time is set");
    }
}

/// Whether every cache file still has the time [`backdate`] gave it: the
/// run read them all and wrote none.
fn untouched(dir: &Path) -> bool {
    cache_files(dir)
        .iter()
        .all(|name| backdated(&cache_dir(dir).join(name)))
}

fn backdated(file: &Path) -> bool {
    let modified = fs::metadata(file)
        .and_then(|meta| meta.modified())
        .expect("a cache file has a time");
    modified == SystemTime::UNIX_EPOCH + LONG_AGO
}

#[test]
fn a_second_run_reads_the_cache_and_prints_what_parsing_printed() {
    let dir = copy_fixture("hit");
    // Outside a project: the baseline, and no cache.
    let baseline = transcript(&spinel(&dir, &["run", "main.rb"]));
    assert_eq!(baseline.0, Some(0), "stderr: {}", baseline.2);
    assert!(baseline.1.contains("Hello There, cache! frozen=true\n"));
    assert!(baseline.1.contains("[\"greeting.rb\", 10, \"lib\"]\n"));
    assert!(baseline.1.contains(&format!(
        "{}/lib/failing.rb:3:in 'Failing.call'\n",
        dir.display()
    )));
    assert!(
        baseline
            .2
            .contains("conditional.rb:4: warning: found '= literal'")
    );
    assert!(
        !dir.join(".spinel").exists(),
        "nothing is written outside a project"
    );

    make_project(&dir);
    let first = transcript(&spinel(&dir, &["run", "main.rb"]));
    assert_eq!(first, baseline);
    assert_eq!(cache_files(&dir), expected_files(&dir));

    backdate(&dir);
    let second = transcript(&spinel(&dir, &["main.rb"]));
    assert_eq!(second, baseline);
    assert!(
        untouched(&dir),
        "the second run read the cache rather than parsing"
    );
    assert_eq!(cache_files(&dir), expected_files(&dir));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_truncated_or_stale_cache_file_is_parsed_past_and_replaced() {
    let dir = copy_fixture("corrupt");
    let baseline = transcript(&spinel(&dir, &["run", "main.rb"]));
    make_project(&dir);
    assert_eq!(transcript(&spinel(&dir, &["run", "main.rb"])), baseline);
    let greeting = cache_file(&dir, &dir.join("lib/greeting.rb"));
    let good = fs::read(&greeting).expect("the cache file was written");

    let mut flipped = good.clone();
    *flipped.last_mut().expect("not empty") ^= 0x20;
    let mut other_build = good.clone();
    // The compiler fingerprint follows the 8-byte magic and 4-byte format.
    other_build[12] ^= 0x01;
    let damaged: [(&str, Vec<u8>); 4] = [
        ("a flipped payload byte", flipped),
        ("half the file", good[..good.len() / 2].to_vec()),
        ("an empty file", Vec::new()),
        ("another build's fingerprint", other_build),
    ];
    for (what, bytes) in damaged {
        fs::write(&greeting, &bytes).expect("the cache file is damaged");
        let run = transcript(&spinel(&dir, &["run", "main.rb"]));
        assert_eq!(run, baseline, "with {what}");
        assert_eq!(
            fs::read(&greeting).expect("rewritten"),
            good,
            "{what} was replaced"
        );
    }
    assert_eq!(cache_files(&dir), expected_files(&dir));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_source_is_compiled_again_under_its_new_hash() {
    let dir = copy_fixture("changed");
    make_project(&dir);
    let before = transcript(&spinel(&dir, &["run", "main.rb"]));
    assert_eq!(before.0, Some(0), "stderr: {}", before.2);
    let source = dir.join("lib/greeting.rb");
    let old = cache_file(&dir, &source);
    let text = fs::read_to_string(&source).expect("the source reads");
    fs::write(&source, text.replace("hello there", "goodbye now")).expect("the source changes");
    let new = cache_file(&dir, &source);
    assert_ne!(old, new);
    backdate(&dir);

    let after = transcript(&spinel(&dir, &["run", "main.rb"]));
    assert_eq!(after.0, Some(0), "stderr: {}", after.2);
    assert_eq!(after.1, before.1.replace("Hello There", "Goodbye Now"));
    assert!(new.exists(), "the new source is cached under its own hash");
    assert!(!backdated(&new));
    // The files that did not change were read, not rewritten.
    for file in ["lib/failing.rb", "lib/conditional.rb"] {
        assert!(backdated(&cache_file(&dir, &dir.join(file))), "{file}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn processes_running_at_once_share_one_cache() {
    let dir = copy_fixture("processes");
    let baseline = transcript(&spinel(&dir, &["run", "main.rb"]));
    make_project(&dir);
    // Several processes find the cache empty at once, and each writes it.
    let children: Vec<_> = (0..6)
        .map(|_| {
            Command::new(SPINEL)
                .args(["run", "main.rb"])
                .current_dir(&dir)
                .env_remove("GEM_HOME")
                .env_remove("GEM_PATH")
                .env_remove("RUBYOPT")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spinel starts")
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().expect("spinel finishes");
        assert_eq!(transcript(&out), baseline);
    }
    // Every write landed whole, and no temporary file was left.
    assert_eq!(cache_files(&dir), expected_files(&dir));
    // A process that did not write them reads them.
    backdate(&dir);
    assert_eq!(transcript(&spinel(&dir, &["run", "main.rb"])), baseline);
    assert!(untouched(&dir));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_syntax_error_reads_the_same_and_is_never_cached() {
    let dir = copy_fixture("syntax");
    let baseline = transcript(&spinel(&dir, &["run", "requires_broken.rb"]));
    assert_eq!(baseline.0, Some(1));
    assert!(baseline.2.contains("broken.rb:2: unexpected end-of-input"));
    make_project(&dir);
    for _ in 0..2 {
        assert_eq!(
            transcript(&spinel(&dir, &["run", "requires_broken.rb"])),
            baseline
        );
    }
    assert_eq!(cache_files(&dir), boot_files());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ruby_command_lines_and_dump_bytecode_write_no_cache() {
    let dir = copy_fixture("ruby-lines");
    make_project(&dir);
    let program = "require './lib/greeting'; puts Greeting.hello('e')";
    let out = spinel(&dir, &["-e", program]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Hello There, e! frozen=true\n"
    );
    let mut child = Command::new(SPINEL)
        .arg("-")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spinel starts");
    std::io::Write::write_all(
        &mut child.stdin.take().expect("a stdin"),
        program.as_bytes(),
    )
    .expect("the program is written");
    let out = child.wait_with_output().expect("spinel finishes");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Hello There, e! frozen=true\n"
    );
    let dump = transcript(&spinel(&dir, &["run", "--dump-bytecode", "main.rb"]));
    assert_eq!(dump.0, Some(0));
    assert!(dump.1.starts_with("Iseq {"), "{}", dump.1);
    assert!(!cache_dir(&dir).exists(), "{:?}", cache_files(&dir));
    // With a cache in place, the dump is the same.
    assert_eq!(spinel(&dir, &["run", "main.rb"]).status.code(), Some(0));
    assert!(cache_dir(&dir).exists());
    assert_eq!(
        transcript(&spinel(&dir, &["run", "--dump-bytecode", "main.rb"])),
        dump
    );
    let _ = fs::remove_dir_all(&dir);
}
