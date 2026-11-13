//! The setup shim, `shims/setup.rb` (#64, docs/cli.md, "What `spinel run`
//! sets up"): `spinel run` in a project's subdirectory finds the project's
//! `.spinel/gem_home/`, a child process finds its `bin/` first on PATH, and
//! outside a project nothing changes and nothing fails.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// What the test gives `spinel` as PATH, so a child finds `sh` and
/// `printenv` and nothing from the machine running the tests.
const PATH: &str = "/usr/bin:/bin";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/setup")
}

/// The fixture project, as `Dir.pwd` spells it: symlinks resolved.
fn project() -> PathBuf {
    fs::canonicalize(fixtures().join("project")).expect("the fixture project exists")
}

fn gem_home() -> String {
    project()
        .join(".spinel/gem_home")
        .to_string_lossy()
        .into_owned()
}

/// `spinel` in `dir`, with GEM_HOME and GEM_PATH unset and PATH as `path`.
fn spinel_in(dir: &Path, path: &str, args: &[&str]) -> Output {
    Command::new(SPINEL)
        .args(args)
        .current_dir(dir)
        .env_remove("GEM_HOME")
        .env_remove("GEM_PATH")
        .env("PATH", path)
        .output()
        .expect("spinel runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The report `app/models/report.rb` prints for a run in the project.
fn in_project_report(child_path: &str) -> String {
    let gem_home = gem_home();
    format!(
        "GEM_HOME={gem_home:?}\nGEM_PATH={gem_home:?}\nchild PATH={child_path}\nran the gem home's executable\n"
    )
}

#[test]
fn spinel_run_in_a_subdirectory_finds_the_project_gem_home() {
    let dir = project().join("app/models");
    let out = spinel_in(&dir, PATH, &["run", "report.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        in_project_report(&format!("{}/bin:{PATH}", gem_home()))
    );
}

#[test]
fn spinel_with_a_bare_file_sets_up_as_spinel_run_does() {
    let dir = project().join("app/models");
    let out = spinel_in(&dir, PATH, &["report.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        in_project_report(&format!("{}/bin:{PATH}", gem_home()))
    );
}

#[test]
fn the_gem_home_bin_is_on_path_once_however_often_spinel_nests() {
    // As a `spinel` started from inside `spinel run` would find PATH.
    let bin = format!("{}/bin", gem_home());
    let given = format!("/usr/bin:{bin}:/bin:{bin}");
    let out = spinel_in(&project(), &given, &["run", "app/models/report.rb"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), in_project_report(&format!("{bin}:{PATH}")));
}

#[test]
fn a_loaded_gem_is_told_the_gem_home() {
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shims/setup.rb");
    let shim = fs::canonicalize(shim).expect("shims/setup.rb exists");
    let out = spinel_in(
        &project().join("app"),
        PATH,
        &["run", "models/with_gem.rb", &shim.to_string_lossy()],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let gem_home = gem_home();
    assert_eq!(
        stdout(&out),
        format!("{{\"GEM_HOME\" => {gem_home:?}, \"GEM_PATH\" => {gem_home:?}}}\n")
    );
}

#[test]
fn outside_a_project_nothing_changes_and_nothing_fails() {
    // A directory of its own under the system's temporary one, which no
    // project encloses.
    let dir = std::env::temp_dir().join(format!("spinel-setup-outside-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the temporary directory is writable");
    let report = fixtures().join("outside/report.rb");
    let out = spinel_in(&dir, PATH, &["run", &report.to_string_lossy()]);
    let _ = fs::remove_dir_all(&dir);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stderr(&out), "");
    assert_eq!(
        stdout(&out),
        format!("GEM_HOME=nil\nGEM_PATH=nil\nPATH={PATH}\n")
    );
}

#[test]
fn ruby_command_line_looks_for_no_project() {
    let dir = project().join("app/models");
    let out = spinel_in(&dir, PATH, &["-e", "p ENV[\"GEM_HOME\"], ENV[\"PATH\"]"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), format!("nil\n{PATH:?}\n"));
}
