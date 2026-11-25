//! `spinel run -S <exe>` (#896, docs/cli.md, "What `spinel run` sets up"):
//! `-S` looks on the `PATH` `spinel run` makes, with the project's
//! `.spinel/gem_home/bin/` first, so a gem's executable runs by name as
//! `bundle exec ruby -S <exe>` runs it on CRuby. The gem is
//! `tests/fixtures/run_search_path/greeter/`, a path gem with an executable,
//! installed with `spinel install`; nothing touches the network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

/// What the test gives `spinel` as PATH: nothing from the machine running
/// the tests that could hold a `greeter`.
const PATH: &str = "/usr/bin:/bin";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run_search_path")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A project whose Gemfile takes `greeter` from a path beside it, with a
/// Spinel home of its own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("run_search_path")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("project/app")).unwrap();
        copy_dir(&fixtures().join("greeter"), &root.join("greeter"));
        fs::write(
            root.join("project/Gemfile"),
            "gem \"greeter\", path: \"../greeter\"\n",
        )
        .unwrap();
        Sandbox { root }
    }

    fn project(&self) -> PathBuf {
        self.root.join("project")
    }

    fn spinel_in(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(SPINEL);
        command
            .current_dir(dir)
            .env("SPINEL_HOME", self.root.join("home"))
            .env("PATH", PATH)
            .args(args);
        for var in [
            "GEM_HOME",
            "GEM_PATH",
            "RUBYPATH",
            "RUBYOPT",
            "RUBYLIB",
            "BUNDLE_GEMFILE",
            "BUNDLE_PATH",
            "BUNDLE_APP_CONFIG",
        ] {
            command.env_remove(var);
        }
        command.output().expect("spinel runs")
    }

    fn install(&self) {
        let out = self.spinel_in(&self.project(), &["install"]);
        assert_eq!(ok(&out, "spinel install"), "+ greeter 0.1.0\n");
        assert!(
            self.project()
                .join(".spinel/gem_home/bin/greeter")
                .is_file()
        );
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

#[test]
fn spinel_run_dash_s_finds_the_project_gem_home_stub() {
    let sandbox = Sandbox::new("stub");
    sandbox.install();
    // The stub is found by name, and the words after it are its own.
    assert_eq!(
        ok(
            &sandbox.spinel_in(&sandbox.project(), &["run", "-S", "greeter", "--version"]),
            "spinel run -S greeter --version"
        ),
        "greeter 0.1.0 --version\n"
    );
    // From a subdirectory, as the setup shim finds the gem home upward.
    assert_eq!(
        ok(
            &sandbox.spinel_in(
                &sandbox.project().join("app"),
                &["run", "-S", "greeter", "a", "b"]
            ),
            "spinel run -S greeter in a subdirectory"
        ),
        "greeter 0.1.0 a b\n"
    );
}

#[test]
fn ruby_command_line_dash_s_looks_for_no_project() {
    // `ruby -S` outside `bundle exec` sees only the PATH it was given.
    let sandbox = Sandbox::new("ruby");
    sandbox.install();
    let out = sandbox.spinel_in(&sandbox.project(), &["-S", "greeter"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "spinel: No such file or directory -- greeter (LoadError)\n"
    );
}
