//! `ARGF` on the `spinel` binary (#41): `-n` and `-p` read the files named
//! on the command line one after another through it, with `$.`, `$FILENAME`
//! and `ARGF.file` following along, and `-` standing for standard input.
//!
//! `-i[ext]` edits those files in place, each one's output going back into
//! it and the original kept as a backup when there is an extension.
//!
//! Each expectation was measured on ruby 4.0.9 with the same arguments and
//! input, in `tests/fixtures/argf`: `a.txt` has two lines and `b.txt` three.
//! The in-place tests edit copies of them in a directory of their own.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/argf")
}

fn out_with(args: &[&str], input: &str) -> String {
    let mut child = Command::new(SPINEL)
        .args(args)
        .current_dir(fixtures())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("stdin takes the input");
    let output = child.wait_with_output().expect("spinel finishes");
    assert!(
        output.status.success(),
        "spinel {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

fn out(args: &[&str]) -> String {
    out_with(args, "")
}

/// A fresh directory holding copies of the fixtures, for a test to edit.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spinel-argf-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("the scratch directory is made");
    for file in ["a.txt", "b.txt"] {
        fs::copy(fixtures().join(file), dir.join(file)).expect("the fixture is copied");
    }
    dir
}

fn run_in(dir: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(SPINEL)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spinel runs");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("stdin takes the input");
    child.wait_with_output().expect("spinel finishes")
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} is read: {e}", path.display()))
}

#[test]
fn dash_n_reads_every_file_in_turn() {
    assert_eq!(
        out(&[
            "-ne",
            "print \"#{$FILENAME}:#{ARGF.file.lineno}:#{$.}:#{$_}\"",
            "a.txt",
            "b.txt",
        ]),
        "a.txt:1:1:alpha\n\
         a.txt:2:2:beta\n\
         b.txt:1:3:gamma\n\
         b.txt:2:4:delta\n\
         b.txt:3:5:epsilon\n"
    );
}

#[test]
fn dash_p_reads_standard_input_where_a_dash_is() {
    assert_eq!(
        out_with(
            &[
                "-pe",
                "$_ = \"#{$.} #{$FILENAME} #{$_}\"",
                "a.txt",
                "-",
                "b.txt"
            ],
            "in\n"
        ),
        "1 a.txt alpha\n\
         2 a.txt beta\n\
         3 - in\n\
         4 b.txt gamma\n\
         5 b.txt delta\n\
         6 b.txt epsilon\n"
    );
}

#[test]
fn skip_moves_on_to_the_next_file() {
    assert_eq!(
        out(&[
            "-ne",
            "print; ARGF.skip if ARGF.file.lineno == 1",
            "a.txt",
            "b.txt"
        ]),
        "alpha\ngamma\n"
    );
}

#[test]
fn reading_runs_across_the_files() {
    assert_eq!(
        out(&[
            "-e",
            "p ARGF.read(8), ARGF.lineno, ARGF.readlines, ARGF.lineno, $., ARGF.read, ARGF.eof?",
            "a.txt",
            "b.txt",
        ]),
        "\"alpha\\nbe\"\n0\n[\"ta\\n\", \"gamma\\n\", \"delta\\n\", \"epsilon\\n\"]\n4\n4\n\"\"\ntrue\n"
    );
    assert_eq!(
        out(&["-e", "p readlines, $.; p gets", "a.txt", "b.txt"]),
        "[\"alpha\\n\", \"beta\\n\", \"gamma\\n\", \"delta\\n\", \"epsilon\\n\"]\n5\nnil\n"
    );
}

#[test]
fn rewind_and_close_reset_the_line_number() {
    assert_eq!(
        out(&[
            "-e",
            "p gets; ARGF.rewind; p [ARGF.lineno, $.]; ARGF.close; p [ARGF.lineno, $., ARGF.filename]; p ARGF.readline",
            "a.txt",
            "b.txt",
        ]),
        "\"alpha\\n\"\n[0, 1]\n[0, 1, \"b.txt\"]\n\"gamma\\n\"\n"
    );
}

#[test]
fn dash_i_edits_in_place_and_keeps_backups() {
    let dir = scratch("backup");
    let output = run_in(&dir, &["-i.bak", "-pe", "$_.upcase!", "a.txt", "b.txt"], "");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Everything printed went into the files.
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(read(dir.join("a.txt")), "ALPHA\nBETA\n");
    assert_eq!(read(dir.join("b.txt")), "GAMMA\nDELTA\nEPSILON\n");
    assert_eq!(read(dir.join("a.txt.bak")), "alpha\nbeta\n");
    assert_eq!(read(dir.join("b.txt.bak")), "gamma\ndelta\nepsilon\n");
    fs::remove_dir_all(&dir).expect("the scratch directory is removed");
}

#[test]
fn dash_i_without_an_extension_keeps_the_mode_and_skips_standard_input() {
    let dir = scratch("plain");
    fs::set_permissions(dir.join("b.txt"), fs::Permissions::from_mode(0o640))
        .expect("the mode is set");
    let output = run_in(
        &dir,
        &["-i", "-ne", "print $., \" \", $_", "-", "b.txt"],
        "in\n",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "-e:1: warning: Can't do inplace edit for stdio; skipping\n"
    );
    assert_eq!(read(dir.join("b.txt")), "1 gamma\n2 delta\n3 epsilon\n");
    let mode = fs::metadata(dir.join("b.txt"))
        .expect("b.txt is there")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o640);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("the directory is read")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt"]);
    fs::remove_dir_all(&dir).expect("the scratch directory is removed");
}
