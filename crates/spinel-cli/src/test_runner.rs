//! `spinel test` (docs/test-runner.md, "Execution"): the embedded
//! `shims/test_runner.rb` run on the VM as `spinel run` runs a program, with
//! the worker count it forks, after `shims/test_worker.rb`, as `-r` loads a
//! library. What it discovers, loads, forks and reports is the shims'; this
//! side only chooses how many workers and the reporter, and passes the paths
//! it was given.

use std::path::Path;
use std::process::ExitCode;

/// The parent and its workers, in Ruby.
const TEST_RUNNER: &str = include_str!("../../../shims/test_runner.rb");

/// The events a worker writes back, and the Minitest reporter and RSpec
/// formatter that write them.
const TEST_WORKER: &str = include_str!("../../../shims/test_worker.rb");

/// How `spinel test` reports the run on stdout (docs/test-runner.md,
/// "Output").
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reporter {
    /// A character per result, then each failure, a summary line and a
    /// command to re-run each failure.
    Progress,
    /// One JSON object per line, per event, for tools.
    Json,
    /// JUnit XML, for CI.
    Junit,
}

impl Reporter {
    /// As `shims/test_runner.rb` and `--reporter` take it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Reporter::Progress => "progress",
            Reporter::Json => "json",
            Reporter::Junit => "junit",
        }
    }
}

/// Run the suite under the current directory, or the `paths` given (each a
/// file or `FILE:LINE`), on `jobs` workers, or one per physical core,
/// reported by `reporter`, recording what failed in `failures` for
/// `--watch` when it is given.
pub(crate) fn run(
    jobs: Option<usize>,
    reporter: Reporter,
    failures: Option<&Path>,
    paths: &[String],
) -> ExitCode {
    let jobs = jobs.unwrap_or_else(physical_cores);
    let failures = failures.map_or_else(String::new, |path| path.to_string_lossy().into_owned());
    let mut args = vec![jobs.to_string(), reporter.name().to_owned(), failures];
    args.extend(paths.iter().cloned());
    let around = crate::Around {
        setup: true,
        library: Some(("<internal:test_worker>", TEST_WORKER)),
        ..crate::Around::default()
    };
    crate::run_program(
        Path::new("<internal:test_runner>"),
        TEST_RUNNER.as_bytes(),
        &args,
        false,
        around,
    )
}

/// The physical cores (docs/test-runner.md): a hyperthread's sibling shares
/// its core's execution units, so it does not get a worker of its own. Never
/// more than the CPUs this process may run on (a container's quota, an
/// affinity mask), and those when the platform does not say.
fn physical_cores() -> usize {
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    platform_physical_cores()
        .filter(|&n| n > 0)
        .map_or(available, |n| n.min(available))
}

/// Linux: each online CPU's `(physical_package_id, core_id)`, counted once.
#[cfg(target_os = "linux")]
fn platform_physical_cores() -> Option<usize> {
    let mut cores = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name.strip_prefix("cpu") else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // An offline CPU has no topology.
        let topology = entry.path().join("topology");
        let read = |file: &str| {
            std::fs::read_to_string(topology.join(file))
                .ok()
                .map(|text| text.trim().to_owned())
        };
        if let (Some(package), Some(core)) = (read("physical_package_id"), read("core_id")) {
            cores.insert((package, core));
        }
    }
    Some(cores.len())
}

/// macOS: `hw.physicalcpu`.
#[cfg(target_os = "macos")]
fn platform_physical_cores() -> Option<usize> {
    let mut count: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated and `count` is `size` bytes long.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.physicalcpu".as_ptr(),
            (&raw mut count).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0).then(|| usize::try_from(count).ok()).flatten()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_physical_cores() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn there_is_at_least_one_core() {
        let cores = super::physical_cores();
        assert!(cores >= 1);
        let logical = std::thread::available_parallelism().map_or(1, usize::from);
        assert!(cores <= logical, "{cores} physical of {logical} logical");
    }
}
