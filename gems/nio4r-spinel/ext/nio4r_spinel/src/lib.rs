//! The native half of the nio4r-spinel gem: the OS readiness calls upstream
//! nio4r's extension has libev make, as singleton methods of `NIO::Spinel`,
//! one system call each. Everything the C extension decides on top of them
//! — which monitors an event goes to, what a selector answers, the
//! exceptions and their messages — is Ruby, in `lib/nio4r_ext.rb`.
//!
//! Each call is made with a zero timeout and never blocks: a method holds
//! its Ractor's lock for its whole call (docs/engine.md, "Threads"), so a
//! selector that has to wait waits in Ruby, in `IO.select` on the readiness
//! queue's descriptor (an epoll or kqueue descriptor is readable while an
//! event is waiting), which the scheduler waits on outside the lock, as
//! libev's patched `epoll_wait` waits outside CRuby's.
//!
//! Interests and events are libev's masks: [`READ`] and [`WRITE`]. A call
//! that fails answers `-errno`, which the Ruby side raises as the
//! `SystemCallError` the C extension's `rb_sys_fail` would.

use spinel_ext::{Context, Error, Result, Value};

spinel_ext::extension!(init);

/// libev's `EV_READ`.
const READ: i64 = 1;
/// libev's `EV_WRITE`.
const WRITE: i64 = 2;
/// The bit `poll_ready` adds for a descriptor that is not open
/// (`POLLNVAL`), which libev's poll backend kills.
const INVALID: i64 = 4;

/// The most events one `queue_wait` reads.
const MAX_EVENTS: usize = 64;

/// One method of `NIO::Spinel`: `$cx`, and the arguments as `$args`.
macro_rules! methods {
    ($($(#[$meta:meta])* fn $name:ident($cx:ident, $args:ident) $body:block)*) => {
        $(
            $(#[$meta])*
            fn $name<'cx>(
                $cx: &mut Context<'cx>,
                _self: Value<'cx>,
                $args: &[Value<'cx>],
            ) -> Result<Value<'cx>> $body
        )*
    };
}

fn init(cx: &mut Context<'_>) -> Result<()> {
    let nio = cx.define_module("NIO")?;
    let spinel = cx.define_module_under(nio, "Spinel")?;
    let table: &[(&str, i32, spinel_ext::Method)] = &[
        ("queue_backend", 0, queue_backend),
        ("char_signed?", 0, char_signed),
        ("queue_new", 0, queue_new),
        ("close", 1, close),
        ("queue_set", 4, queue_set),
        ("queue_wait", 1, queue_wait),
        ("poll_ready", 1, poll_ready),
        ("set_nonblock", 1, set_nonblock),
        ("read", 2, read),
        ("write", 2, write),
    ];
    for &(name, arity, method) in table {
        cx.define_singleton_method(spinel, name, arity, method)?;
    }
    Ok(())
}

fn errno() -> i64 {
    -i64::from(
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO),
    )
}

/// A descriptor argument, as a C `int`.
fn fd<'cx>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<libc::c_int> {
    let fd = cx.to_i64(value)?;
    libc::c_int::try_from(fd).map_err(|_| {
        Error::new(
            "RangeError",
            format!("integer {fd} too big to convert to 'int'"),
        )
    })
}

/// `(fd, mask)` pairs as the Ruby side unpacks them: `"l<l<"` each.
fn pack(pairs: impl IntoIterator<Item = (i32, i64)>) -> Vec<u8> {
    let mut out = Vec::new();
    for (fd, mask) in pairs {
        out.extend_from_slice(&fd.to_le_bytes());
        out.extend_from_slice(&(mask as i32).to_le_bytes());
    }
    out
}

methods! {
    /// The readiness queue this platform has: `"epoll"` or `"kqueue"`.
    fn queue_backend(cx, _args) {
        Ok(cx.string(if cfg!(any(target_os = "linux", target_os = "android")) { "epoll" } else { "kqueue" }))
    }

    /// Whether a C `char` is signed here, as it is on x86-64 and on Apple's
    /// arm64 and is not on Linux's: what upstream's `NIO::ByteBuffer#[]`
    /// and `#each` answer for a byte past 127 depends on it.
    fn char_signed(cx, _args) {
        Ok(cx.boolean(libc::c_char::MIN != 0))
    }

    /// A new readiness queue, close-on-exec: its descriptor, or `-errno`.
    fn queue_new(cx, _args) {
        Ok(cx.integer(queue::new()))
    }

    /// `close(fd)`: 0, or `-errno`.
    fn close(cx, args) {
        let fd = fd(cx, args[0])?;
        // SAFETY: closes a descriptor the caller owns, nothing else.
        let status = unsafe { libc::close(fd) };
        Ok(cx.integer(if status == 0 { 0 } else { errno() }))
    }

    /// `queue_set(queue, fd, old, new)`: watch `fd` for the interests `new`
    /// where it was watched for `old` (0 when it was not, 0 for none): 0,
    /// or `-errno`.
    fn queue_set(cx, args) {
        let (queue, target) = (fd(cx, args[0])?, fd(cx, args[1])?);
        let (old, new) = (cx.to_i64(args[2])?, cx.to_i64(args[3])?);
        Ok(cx.integer(queue::set(queue, target, old, new)))
    }

    /// The events waiting on `queue`, without waiting: `(fd, mask)` pairs,
    /// libev's mask of what the descriptor is ready for, or `-errno`.
    fn queue_wait(cx, args) {
        let queue = fd(cx, args[0])?;
        Ok(match queue::wait(queue) {
            Ok(events) => cx.string_from_bytes(&pack(events)),
            Err(errno) => cx.integer(errno),
        })
    }

    /// `poll(2)` with no timeout over `(fd, mask)` pairs: a pair for each
    /// descriptor with an event, as libev's poll backend reads `revents`,
    /// [`INVALID`] added for one that is not open; or `-errno`.
    fn poll_ready(cx, args) {
        let bytes = cx.to_bytes(args[0])?;
        let mut fds: Vec<libc::pollfd> = bytes
            .chunks_exact(8)
            .map(|pair| {
                let fd = i32::from_le_bytes(pair[..4].try_into().unwrap());
                let mask = i64::from(i32::from_le_bytes(pair[4..].try_into().unwrap()));
                let mut events = 0;
                if mask & READ != 0 {
                    events |= libc::POLLIN;
                }
                if mask & WRITE != 0 {
                    events |= libc::POLLOUT;
                }
                libc::pollfd { fd, events, revents: 0 }
            })
            .collect();
        // SAFETY: `fds` is a live array of `fds.len()` pollfds.
        let count = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 0) };
        if count < 0 {
            return Ok(cx.integer(errno()));
        }
        let ready = fds.iter().filter(|p| p.revents != 0).map(|p| {
            let revents = p.revents;
            let mut mask = 0;
            if revents & (libc::POLLOUT | libc::POLLERR | libc::POLLHUP) != 0 {
                mask |= WRITE;
            }
            if revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0 {
                mask |= READ;
            }
            if revents & libc::POLLNVAL != 0 {
                mask |= INVALID;
            }
            (p.fd, mask)
        });
        Ok(cx.string_from_bytes(&pack(ready)))
    }

    /// `rb_io_set_nonblock`'s `fcntl`: 0, or `-errno`.
    fn set_nonblock(cx, args) {
        let fd = fd(cx, args[0])?;
        // SAFETY: F_GETFL and F_SETFL on a descriptor change only its flags.
        let status = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || flags & libc::O_NONBLOCK != 0 {
                flags
            } else {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK)
            }
        };
        Ok(cx.integer(if status < 0 { errno() } else { 0 }))
    }

    /// `read(fd, n)`: the bytes read, empty at the end of the stream, or
    /// `-errno`.
    fn read(cx, args) {
        let fd = fd(cx, args[0])?;
        let len = usize::try_from(cx.to_i64(args[1])?).unwrap_or(0);
        let mut buffer = vec![0u8; len];
        // SAFETY: reads at most `len` bytes into a buffer of `len`.
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), len) };
        if count < 0 {
            return Ok(cx.integer(errno()));
        }
        buffer.truncate(count as usize);
        Ok(cx.string_from_bytes(&buffer))
    }

    /// `write(fd, bytes)`: how many were written, or `-errno`.
    fn write(cx, args) {
        let fd = fd(cx, args[0])?;
        let bytes = cx.to_bytes(args[1])?;
        // SAFETY: writes from a live buffer of `bytes.len()` bytes.
        let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        Ok(cx.integer(if count < 0 { errno() } else { count as i64 }))
    }
}

/// epoll, level-triggered as libev's epoll backend uses it.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod queue {
    use super::{MAX_EVENTS, READ, WRITE, errno};

    pub fn new() -> i64 {
        // SAFETY: makes a descriptor, nothing else.
        let fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if fd < 0 { errno() } else { i64::from(fd) }
    }

    fn events(mask: i64) -> u32 {
        let mut events = 0;
        if mask & READ != 0 {
            events |= libc::EPOLLIN as u32;
        }
        if mask & WRITE != 0 {
            events |= libc::EPOLLOUT as u32;
        }
        events
    }

    fn ctl(queue: i32, op: i32, fd: i32, mask: i64) -> i64 {
        let mut event = libc::epoll_event {
            events: events(mask),
            u64: fd as u64,
        };
        // SAFETY: `event` is live for the call.
        if unsafe { libc::epoll_ctl(queue, op, fd, &mut event) } == 0 {
            0
        } else {
            errno()
        }
    }

    pub fn set(queue: i32, fd: i32, old: i64, new: i64) -> i64 {
        if new == 0 {
            return if old == 0 {
                0
            } else {
                ctl(queue, libc::EPOLL_CTL_DEL, fd, 0)
            };
        }
        // What epoll holds can differ from what was asked: a descriptor
        // closed while it was watched leaves the set, and one opened in its
        // place is not in it. libev retries the other way, and so does this.
        let (first, second) = if old == 0 {
            (libc::EPOLL_CTL_ADD, libc::EPOLL_CTL_MOD)
        } else {
            (libc::EPOLL_CTL_MOD, libc::EPOLL_CTL_ADD)
        };
        match ctl(queue, first, fd, new) {
            status if status == -i64::from(libc::EEXIST) || status == -i64::from(libc::ENOENT) => {
                ctl(queue, second, fd, new)
            }
            status => status,
        }
    }

    pub fn wait(queue: i32) -> Result<Vec<(i32, i64)>, i64> {
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; MAX_EVENTS];
        // SAFETY: `events` holds MAX_EVENTS events.
        let count = unsafe { libc::epoll_wait(queue, events.as_mut_ptr(), MAX_EVENTS as i32, 0) };
        if count < 0 {
            return if errno() == -i64::from(libc::EINTR) {
                Ok(Vec::new())
            } else {
                Err(errno())
            };
        }
        Ok(events[..count as usize]
            .iter()
            .map(|event| {
                let got = event.events;
                let (out, err) = (
                    libc::EPOLLOUT as u32,
                    (libc::EPOLLERR | libc::EPOLLHUP) as u32,
                );
                let mut mask = 0;
                // ev_epoll.c's reading of an event.
                if got & (out | err) != 0 {
                    mask |= WRITE;
                }
                if got & (libc::EPOLLIN as u32 | err) != 0 {
                    mask |= READ;
                }
                (event.u64 as i32, mask)
            })
            .collect())
    }
}

/// kqueue, a filter for each interest, as libev's kqueue backend uses it.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod queue {
    use super::{MAX_EVENTS, READ, WRITE, errno};
    use std::ptr;

    pub fn new() -> i64 {
        // SAFETY: makes a descriptor and sets its close-on-exec flag.
        unsafe {
            let fd = libc::kqueue();
            if fd < 0 {
                return errno();
            }
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            i64::from(fd)
        }
    }

    fn change(fd: i32, filter: i16, flags: u16) -> libc::kevent {
        // SAFETY: a kevent is plain data, valid zeroed.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        event.ident = fd as libc::uintptr_t;
        event.filter = filter as _;
        event.flags = flags as _;
        event
    }

    pub fn set(queue: i32, fd: i32, old: i64, new: i64) -> i64 {
        let mut changes = Vec::new();
        for (bit, filter) in [(READ, libc::EVFILT_READ), (WRITE, libc::EVFILT_WRITE)] {
            match (old & bit != 0, new & bit != 0) {
                (false, true) => changes.push(change(fd, filter, libc::EV_ADD | libc::EV_ENABLE)),
                (true, false) => changes.push(change(fd, filter, libc::EV_DELETE)),
                // Added again, as libev re-adds a changed descriptor, in
                // case a descriptor closed while watched took the filter
                // with it.
                (true, true) => changes.push(change(fd, filter, libc::EV_ADD | libc::EV_ENABLE)),
                (false, false) => {}
            }
        }
        if changes.is_empty() {
            return 0;
        }
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut status = 0;
        for event in &changes {
            // SAFETY: one change, no events read back.
            let done = unsafe { libc::kevent(queue, event, 1, ptr::null_mut(), 0, &timeout) };
            // A filter deleted from a descriptor closed meanwhile is gone
            // already.
            if done < 0 {
                let failed = errno();
                let gone = [libc::ENOENT, libc::EBADF].map(|e| -i64::from(e));
                if event.flags & libc::EV_DELETE == 0 || !gone.contains(&failed) {
                    status = failed;
                }
            }
        }
        status
    }

    pub fn wait(queue: i32) -> Result<Vec<(i32, i64)>, i64> {
        // SAFETY: kevents are plain data, valid zeroed.
        let mut events: [libc::kevent; MAX_EVENTS] = unsafe { std::mem::zeroed() };
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `events` holds MAX_EVENTS events.
        let count = unsafe {
            libc::kevent(
                queue,
                ptr::null(),
                0,
                events.as_mut_ptr(),
                MAX_EVENTS as i32,
                &timeout,
            )
        };
        if count < 0 {
            return if errno() == -i64::from(libc::EINTR) {
                Ok(Vec::new())
            } else {
                Err(errno())
            };
        }
        let mut ready: Vec<(i32, i64)> = Vec::new();
        for event in &events[..count as usize] {
            let fd = event.ident as i32;
            // ev_kqueue.c: an error is both events, which the watchers then
            // fail on; a filter's event is its own.
            let mask = if event.flags & libc::EV_ERROR != 0 {
                READ | WRITE
            } else if event.filter == libc::EVFILT_READ as _ {
                READ
            } else if event.filter == libc::EVFILT_WRITE as _ {
                WRITE
            } else {
                0
            };
            match ready.iter_mut().find(|(seen, _)| *seen == fd) {
                Some((_, seen)) => *seen |= mask,
                None => ready.push((fd, mask)),
            }
        }
        Ok(ready)
    }
}
