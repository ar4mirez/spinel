//! The require map (#66, docs/cli.md, "Bytecode cache and require map"):
//! `.spinel/require_map.bin` lists what the installed gems' `require_paths`
//! and the standard library held, and `require` passes over the load path
//! entries it lists that do not have a file instead of looking on disk. The
//! map is only ever a cache: a program run with it prints exactly what the
//! same program prints without it, whatever `$LOAD_PATH` it makes, whatever
//! `-I` it is given, and however the listed directories changed since the
//! map was written. `tests/install.rs` checks that `spinel install` writes
//! it; these make it with the hidden `spinel require-map`, which writes the
//! same map for directories given.
//!
//! Each test runs the fixture in a copy of its own, since some change it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// The fixture copied to a fresh directory of the test's own, made a
/// project as far as the setup shim looks, with no map yet.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("require_map")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/require_map"),
        &dir,
    );
    fs::create_dir_all(dir.join(".spinel/gem_home")).expect("the gem home is made");
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

fn map_path(dir: &Path) -> PathBuf {
    dir.join(".spinel/require_map.bin")
}

/// Write the project's map of alpha/lib, beta/lib and the standard library.
fn write_map(dir: &Path) {
    let out = spinel(
        dir,
        &[
            "require-map",
            ".spinel/require_map.bin",
            "alpha/lib",
            "beta/lib",
        ],
    );
    assert!(out.status.success(), "spinel require-map: {}", stderr(&out));
    assert!(map_path(dir).is_file());
}

fn spinel(dir: &Path, args: &[&str]) -> Output {
    Command::new(SPINEL)
        .args(args)
        .current_dir(dir)
        .env_remove("GEM_HOME")
        .env_remove("GEM_PATH")
        .env_remove("RUBYOPT")
        .env_remove("RUBYLIB")
        .output()
        .expect("spinel runs")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Status, stdout and stderr, for comparing two runs whole.
fn transcript(out: &Output) -> (Option<i32>, String, String) {
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr(out),
    )
}

/// Run `args` with the map in place, then with it moved aside, and answer
/// the first run's transcript after checking the two are the same.
fn same_with_and_without_map(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let with = transcript(&spinel(dir, args));
    let aside = dir.join("require_map.aside");
    fs::rename(map_path(dir), &aside).expect("the map moves aside");
    let without = transcript(&spinel(dir, args));
    fs::rename(&aside, map_path(dir)).expect("the map moves back");
    assert_eq!(with, without, "{args:?} with the map and without it");
    with
}

#[test]
fn the_map_answers_for_the_directories_it_lists() {
    let dir = project("probe");
    write_map(&dir);
    let out = spinel(
        &dir,
        &[
            "run",
            "probe.rb",
            "which.rb",
            "only_alpha.rb",
            "nested/deep.rb",
            "optparse.rb",
            "nothing_has_this.rb",
        ],
    );
    assert_eq!(
        transcript(&out),
        (
            Some(0),
            // `elsewhere` is `sitelibdir`, which no map lists.
            "which.rb: map beta/lib, map alpha/lib, disk elsewhere, none\n\
             only_alpha.rb: map alpha/lib, disk elsewhere, none\n\
             nested/deep.rb: map beta/lib, disk elsewhere, none\n\
             optparse.rb: disk elsewhere, map rubylibdir\n\
             nothing_has_this.rb: disk elsewhere, none\n"
                .to_owned(),
            String::new()
        )
    );
}

#[test]
fn a_program_that_edits_its_load_path_gets_what_it_asked_for() {
    let dir = project("edits");
    write_map(&dir);
    let (status, stdout, stderr) = same_with_and_without_map(&dir, &["run", "edits.rb"]);
    assert_eq!(stderr, "");
    assert_eq!(status, Some(0));
    assert_eq!(
        stdout,
        format!(
            "false\n[:rb, \"{}/beta/lib/gone.rb\"]\ncannot load such file -- nested/none\n\
             beta, alpha, local, beta, beta nested, only alpha\n",
            dir.display()
        )
    );
}

#[test]
fn dash_i_comes_first_with_the_map_as_without_it() {
    let dir = project("dash-i");
    write_map(&dir);
    let (status, stdout, stderr) =
        same_with_and_without_map(&dir, &["run", "-I", "alpha/lib", "dash_i.rb"]);
    assert_eq!((status, stderr.as_str()), (Some(0), ""));
    assert_eq!(stdout, "alpha, beta nested\n");
}

#[test]
fn a_stale_map_resolves_as_the_disk_does() {
    let dir = project("stale");
    write_map(&dir);
    // After the map: a file added to alpha/lib, one that now shadows
    // beta/lib's, one gone from beta/lib that alpha/lib still has, and one
    // gone from both places that had it.
    fs::write(dir.join("alpha/lib/added.rb"), "WHICH << \"alpha added\"\n").unwrap();
    fs::write(
        dir.join("alpha/lib/shadow.rb"),
        "WHICH << \"alpha shadow\"\n",
    )
    .unwrap();
    fs::remove_file(dir.join("beta/lib/gone.rb")).unwrap();
    fs::remove_file(dir.join("alpha/lib/only_alpha.rb")).unwrap();
    let (status, stdout, stderr) = same_with_and_without_map(&dir, &["run", "stale.rb"]);
    assert_eq!((status, stderr.as_str()), (Some(0), ""));
    assert_eq!(
        stdout,
        "cannot load such file -- only_alpha\nalpha added, alpha shadow, alpha gone\n"
    );
    // The map no longer answers for the directories that changed, or for
    // anything under them, and still does for the standard library.
    let out = spinel(&dir, &["run", "probe.rb", "nested/deep.rb", "optparse.rb"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "nested/deep.rb: disk beta/lib, disk alpha/lib, disk elsewhere, none\n\
         optparse.rb: disk beta/lib, disk alpha/lib, disk elsewhere, map rubylibdir\n"
    );
}

#[test]
fn a_directory_replaced_since_the_map_is_looked_at_on_disk() {
    let dir = project("replaced");
    write_map(&dir);
    // beta/lib/nested replaced by a directory of the same name and files.
    let nested = dir.join("beta/lib/nested");
    fs::rename(&nested, dir.join("nested.old")).unwrap();
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("deep.rb"), "WHICH << \"new nested\"\n").unwrap();
    fs::write(nested.join("extra.rb"), "WHICH << \"extra\"\n").unwrap();
    let out = spinel(
        &dir,
        &["run", "probe.rb", "nested/deep.rb", "nested/extra.rb"],
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "nested/deep.rb: disk beta/lib, disk elsewhere, none\n\
         nested/extra.rb: disk beta/lib, disk elsewhere, none\n"
    );
}

#[test]
fn a_damaged_map_is_not_used() {
    let dir = project("damaged");
    write_map(&dir);
    let good = transcript(&spinel(&dir, &["run", "edits.rb"]));
    let bytes = fs::read(map_path(&dir)).unwrap();
    // Truncated, a byte changed, and not a map at all.
    let damaged = [
        bytes[..bytes.len() / 2].to_vec(),
        {
            let mut b = bytes.clone();
            let last = b.len() - 1;
            b[last] ^= 0x20;
            b
        },
        b"not a require map".to_vec(),
    ];
    for bad in damaged {
        fs::write(map_path(&dir), &bad).unwrap();
        assert_eq!(transcript(&spinel(&dir, &["run", "edits.rb"])), good);
        let out = spinel(&dir, &["run", "probe.rb", "which.rb"]);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "which.rb: disk beta/lib, disk alpha/lib, disk elsewhere, disk rubylibdir, none\n"
        );
    }
}

#[test]
fn outside_a_project_the_map_is_not_read() {
    let dir = project("outside");
    write_map(&dir);
    fs::remove_dir_all(dir.join(".spinel/gem_home")).unwrap();
    let out = spinel(&dir, &["run", "probe.rb", "which.rb"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "which.rb: disk beta/lib, disk alpha/lib, disk elsewhere, disk rubylibdir, none\n"
    );
}
