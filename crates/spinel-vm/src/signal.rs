//! The signal table behind `Signal.list` and `SignalException` (#29).
//!
//! The same shape as `crate::errno`, for the same reason: the *names* are
//! CRuby's and fixed, the *numbers* are the platform's and come from the `libc`
//! crate for the target being built. `SIGIO` is 29 on Linux and 23 on macOS
//! without a committed table saying so.
//!
//! The order is CRuby's `siglist` in `signal.c`, and it matters twice:
//! `Signal.list` is a Hash that iterates in it, and `Signal.signame(6)` is
//! "ABRT" rather than "IOT" because `ABRT` comes first. Measured against
//! ruby 4.0.7 on Linux, `Signal.list` agrees entry for entry.

/// CRuby's `siglist`, as `(name, number)` for every name this platform has.
#[cfg(target_os = "linux")]
const LIST: &[(&str, i32)] = &[
    ("EXIT", 0),
    ("HUP", libc::SIGHUP),
    ("INT", libc::SIGINT),
    ("QUIT", libc::SIGQUIT),
    ("ILL", libc::SIGILL),
    ("TRAP", libc::SIGTRAP),
    ("ABRT", libc::SIGABRT),
    ("IOT", libc::SIGIOT),
    ("FPE", libc::SIGFPE),
    ("KILL", libc::SIGKILL),
    ("BUS", libc::SIGBUS),
    ("SEGV", libc::SIGSEGV),
    ("SYS", libc::SIGSYS),
    ("PIPE", libc::SIGPIPE),
    ("ALRM", libc::SIGALRM),
    ("TERM", libc::SIGTERM),
    ("URG", libc::SIGURG),
    ("STOP", libc::SIGSTOP),
    ("TSTP", libc::SIGTSTP),
    ("CONT", libc::SIGCONT),
    ("CHLD", libc::SIGCHLD),
    // glibc's `<signal.h>` defines `SIGCLD` as `SIGCHLD`; the `libc` crate
    // does not export the alias, so it is spelled here.
    ("CLD", libc::SIGCHLD),
    ("TTIN", libc::SIGTTIN),
    ("TTOU", libc::SIGTTOU),
    ("IO", libc::SIGIO),
    ("XCPU", libc::SIGXCPU),
    ("XFSZ", libc::SIGXFSZ),
    ("VTALRM", libc::SIGVTALRM),
    ("PROF", libc::SIGPROF),
    ("WINCH", libc::SIGWINCH),
    ("USR1", libc::SIGUSR1),
    ("USR2", libc::SIGUSR2),
    ("PWR", libc::SIGPWR),
    ("POLL", libc::SIGPOLL),
];

#[cfg(target_vendor = "apple")]
const LIST: &[(&str, i32)] = &[
    ("EXIT", 0),
    ("HUP", libc::SIGHUP),
    ("INT", libc::SIGINT),
    ("QUIT", libc::SIGQUIT),
    ("ILL", libc::SIGILL),
    ("TRAP", libc::SIGTRAP),
    ("ABRT", libc::SIGABRT),
    ("IOT", libc::SIGIOT),
    ("EMT", libc::SIGEMT),
    ("FPE", libc::SIGFPE),
    ("KILL", libc::SIGKILL),
    ("BUS", libc::SIGBUS),
    ("SEGV", libc::SIGSEGV),
    ("SYS", libc::SIGSYS),
    ("PIPE", libc::SIGPIPE),
    ("ALRM", libc::SIGALRM),
    ("TERM", libc::SIGTERM),
    ("URG", libc::SIGURG),
    ("STOP", libc::SIGSTOP),
    ("TSTP", libc::SIGTSTP),
    ("CONT", libc::SIGCONT),
    ("CHLD", libc::SIGCHLD),
    ("TTIN", libc::SIGTTIN),
    ("TTOU", libc::SIGTTOU),
    ("IO", libc::SIGIO),
    ("XCPU", libc::SIGXCPU),
    ("XFSZ", libc::SIGXFSZ),
    ("VTALRM", libc::SIGVTALRM),
    ("PROF", libc::SIGPROF),
    ("WINCH", libc::SIGWINCH),
    ("USR1", libc::SIGUSR1),
    ("USR2", libc::SIGUSR2),
    ("INFO", libc::SIGINFO),
];

// ponytail: Unix only, like `crate::errno`. Windows (#139) has six signals and
// its own `siglist` subset.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
const LIST: &[(&str, i32)] = &[("EXIT", 0)];

/// `NSIG`, which the `libc` crate does not export for either target. CRuby
/// accepts a signal number up to and including it — `SignalException.new(65)`
/// is "SIG65" on Linux and 66 is an ArgumentError, measured — so it is the
/// bound `SignalException#initialize` checks.
#[cfg(target_os = "linux")]
pub const LIMIT: i32 = 65;
#[cfg(target_vendor = "apple")]
pub const LIMIT: i32 = 32;
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub const LIMIT: i32 = 32;

/// Every signal name this platform has, without its `SIG` prefix, in CRuby's
/// order.
pub fn list() -> impl Iterator<Item = (&'static str, i32)> {
    LIST.iter().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_name_for_a_number_is_crubys() {
        let abrt = list()
            .find(|&(_, n)| n == libc::SIGABRT)
            .map(|(name, _)| name);
        assert_eq!(abrt, Some("ABRT"));
    }

    #[test]
    fn every_number_is_below_the_limit() {
        assert!(list().all(|(_, n)| (0..LIMIT).contains(&n)));
    }
}
