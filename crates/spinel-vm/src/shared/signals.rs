//! Signals that arrived and have not been handled yet (#44).
//!
//! A signal is the process's, not a Ractor's: the operating system delivers
//! it to whichever OS thread does not block it, at any instruction. So what
//! the OS handler can do is what CRuby's `sighandler` does — count the signal
//! and wake whoever waits — and nothing else: it touches no heap, takes no
//! lock and allocates nothing, since none of that is async-signal-safe. The
//! counts are read and cleared by the main Ractor's main thread, which runs
//! the Ruby handler at a safe point of its own (`interp::run`'s turn check,
//! or the scheduler's wait): never inside the OS handler.
//!
//! This is the one table here that is neither append-only nor made once, and
//! it is allowed for the same reason the others are: it holds no VM object.
//! It is a count per signal number and a pipe, the process state POSIX
//! signals are, and only the main Ractor takes from it.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

/// One more than the highest signal number on any platform Spinel builds
/// for: Linux's 64 real-time signals included.
const SLOTS: usize = 65;

/// How many times each signal arrived since it was last taken.
static PENDING: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];

/// Whether any count may be above zero: the loop's one load per turn check.
static ANY: AtomicBool = AtomicBool::new(false);

/// The pipe the handler writes a byte to, so a `poll(2)` on its read end
/// wakes whichever OS thread waits in it: its read end in the high half and
/// its write end in the low, or `NO_PIPE`. Made, nonblocking and
/// close-on-exec, the first time a handler is installed, and made again in a
/// child after `fork`, so parent and child never drain each other's.
static WAKE: AtomicI64 = AtomicI64::new(NO_PIPE);

const NO_PIPE: i64 = -1;

fn wake() -> Option<[libc::c_int; 2]> {
    let packed = WAKE.load(Ordering::SeqCst);
    (packed != NO_PIPE).then_some([(packed >> 32) as libc::c_int, packed as libc::c_int])
}

fn pack([read, write]: [libc::c_int; 2]) -> i64 {
    (i64::from(read) << 32) | i64::from(write as u32)
}

/// What a signal is set to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disposition {
    /// The operating system's default: `SIG_DFL`.
    Default,
    /// Nothing: `SIG_IGN`.
    Ignore,
    /// Counted here, for Ruby to handle at a safe point.
    Ruby,
}

extern "C" fn on_signal(signo: libc::c_int) {
    // Only atomics and `write(2)`, which are async-signal-safe. `errno` is
    // saved and put back, since the thread interrupted may be about to read
    // it.
    let saved = std::io::Error::last_os_error().raw_os_error();
    if let Some(slot) = usize::try_from(signo).ok().and_then(|i| PENDING.get(i)) {
        slot.fetch_add(1, Ordering::SeqCst);
        ANY.store(true, Ordering::SeqCst);
    }
    if let Some([_, write]) = wake() {
        let byte = 1u8;
        // SAFETY: one byte from a local; a full pipe already wakes its reader,
        // so a write that fails loses nothing.
        unsafe {
            libc::write(write, (&raw const byte).cast(), 1);
        }
    }
    if let Some(errno) = saved {
        set_errno(errno);
    }
}

#[cfg(target_os = "linux")]
fn set_errno(errno: libc::c_int) {
    // SAFETY: `__errno_location` answers this thread's errno.
    unsafe { *libc::__errno_location() = errno }
}

#[cfg(target_vendor = "apple")]
fn set_errno(errno: libc::c_int) {
    // SAFETY: `__error` answers this thread's errno.
    unsafe { *libc::__error() = errno }
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn set_errno(_errno: libc::c_int) {}

fn wake_pipe() -> Option<[libc::c_int; 2]> {
    if let Some(pipe) = wake() {
        return Some(pipe);
    }
    let fds = new_pipe()?;
    if WAKE
        .compare_exchange(NO_PIPE, pack(fds), Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // Another thread made one first: this pair is not needed.
        close_pipe(fds);
    }
    wake()
}

fn new_pipe() -> Option<[libc::c_int; 2]> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` is two writable ints for `pipe` to fill.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return None;
    }
    for &fd in &fds {
        // SAFETY: descriptors this call just made.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Some(fds)
}

fn close_pipe(fds: [libc::c_int; 2]) {
    // SAFETY: a pair `new_pipe` made, which nothing else closes.
    unsafe {
        libc::close(fds[0]);
        libc::close(fds[1]);
    }
}

/// Set what `signo` does. `SA_RESTART`, so a call the VM makes under its
/// lock — `waitpid`, `read` of a ready descriptor — is never cut short by a
/// signal it has no way to answer; the waits a program spends its time in
/// (`poll(2)`, which is never restarted) wake for one.
///
/// ponytail: a call that blocks in the kernel under the lock and is not a
/// `poll(2)` — `waitpid`, `accept`, `flock`, a write to a full pipe — meets a
/// signal only when it returns, where CRuby's blocking region is interrupted
/// and the call retried after the handler. Each is one wait-for-readiness or
/// one EINTR loop away.
pub fn install(signo: i32, disposition: Disposition) -> std::io::Result<()> {
    let handler = match disposition {
        Disposition::Default => libc::SIG_DFL,
        Disposition::Ignore => libc::SIG_IGN,
        Disposition::Ruby => {
            if wake_pipe().is_none() {
                return Err(std::io::Error::last_os_error());
            }
            on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t
        }
    };
    // SAFETY: `action` is a zeroed `sigaction` filled in here, and `sigaction`
    // reads it and nothing else.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&raw mut action.sa_mask);
        if libc::sigaction(signo, &raw const action, std::ptr::null_mut()) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// What `signo` was set to when the process started, before Spinel set it:
/// a signal a parent left ignored (`nohup`'s SIGHUP) stays ignored.
pub fn inherited_ignored(signo: i32) -> bool {
    // SAFETY: a null action asks only for the current one, written to `old`.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signo, std::ptr::null(), &raw mut old) == 0
            && old.sa_sigaction == libc::SIG_IGN
    }
}

/// Whether any signal is waiting to be handled.
pub fn any() -> bool {
    ANY.load(Ordering::SeqCst)
}

/// One arrival of the lowest-numbered signal waiting, as CRuby's
/// `rb_get_next_signal` takes it; the rest stay waiting, so a handler that
/// raises leaves them for the next safe point.
pub fn take() -> Option<i32> {
    if !ANY.swap(false, Ordering::SeqCst) {
        return None;
    }
    let mut taken = None;
    for (signo, slot) in PENDING.iter().enumerate() {
        if slot.load(Ordering::SeqCst) == 0 {
            continue;
        }
        if taken.is_some() {
            ANY.store(true, Ordering::SeqCst);
            break;
        }
        slot.fetch_sub(1, Ordering::SeqCst);
        taken = Some(signo as i32);
        if slot.load(Ordering::SeqCst) > 0 {
            ANY.store(true, Ordering::SeqCst);
            break;
        }
    }
    taken
}

/// The wake pipe's read end, for a `poll(2)` to wait on; none before any
/// handler is installed.
pub fn wake_fd() -> Option<libc::c_int> {
    wake().map(|[read, _]| read)
}

/// Empty the wake pipe after a `poll(2)` saw it ready.
pub fn drain() {
    if let Some([read, _]) = wake() {
        let mut bytes = [0u8; 64];
        // SAFETY: `bytes` is writable for its length; the descriptor is
        // nonblocking, so this ends when the pipe is empty.
        while unsafe { libc::read(read, bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
    }
}

/// Block every signal Ruby can handle on the calling OS thread, so the
/// operating system delivers them to the main thread's, which is the one
/// that runs their handlers and the one whose waits they should cut short.
/// What a fault raises (SEGV, BUS, ILL, FPE, TRAP, SYS) stays the faulting
/// thread's.
pub fn block_on_this_thread() {
    // SAFETY: `set` is a local `sigset_t` built here; `pthread_sigmask` reads it.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&raw mut set);
        for fault in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGTRAP,
            libc::SIGSYS,
            libc::SIGABRT,
        ] {
            libc::sigdelset(&raw mut set, fault);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
    }
}

/// Unblock every signal on the calling OS thread: in a child after `fork`,
/// whose only thread may be one that blocked them, and before `execve`,
/// since a mask survives it. Async-signal-safe.
pub fn unblock_on_this_thread() {
    // SAFETY: as in `block_on_this_thread`.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::pthread_sigmask(libc::SIG_SETMASK, &raw const set, std::ptr::null_mut());
    }
}

/// In a child after `fork`: what the parent had waiting is the parent's, and
/// so is its wake pipe; the child's handlers, inherited, wake a pipe of its
/// own.
pub fn forked() {
    ANY.store(false, Ordering::SeqCst);
    for slot in &PENDING {
        slot.store(0, Ordering::SeqCst);
    }
    if let Some(inherited) = wake() {
        let fresh = new_pipe().map_or(NO_PIPE, pack);
        WAKE.store(fresh, Ordering::SeqCst);
        close_pipe(inherited);
    }
    unblock_on_this_thread();
}
