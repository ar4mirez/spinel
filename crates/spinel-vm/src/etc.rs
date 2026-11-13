//! The C library calls `lib/etc.rb` is Ruby over (#111): the user and group
//! databases, `sysconf(3)`, `confstr(3)`, `fpathconf(3)`, `uname(2)` and the
//! processor count, as CRuby's `ext/etc/etc.c` makes them.
//!
//! Entries cross to Ruby as the bytes of each field; `lib/etc.rb` builds the
//! `Etc::Passwd` and `Etc::Group` structs and gives the strings an encoding.
//! A failed call answers its `io::Error`, which the Ruby side raises.

use std::ffi::{CStr, CString, c_char};
use std::io;

/// A `struct passwd`, field by field: name, password, uid, gid, gecos, home
/// directory and shell. A null string field reads as empty, as CRuby's does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passwd {
    pub name: Vec<u8>,
    pub passwd: Vec<u8>,
    pub uid: u32,
    pub gid: u32,
    pub gecos: Vec<u8>,
    pub dir: Vec<u8>,
    pub shell: Vec<u8>,
}

/// A `struct group`: name, password, gid and the member names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: Vec<u8>,
    pub passwd: Vec<u8>,
    pub gid: u32,
    pub mem: Vec<Vec<u8>>,
}

/// The bytes of a C string, empty for a null pointer.
///
/// # Safety
///
/// `text` is null or points at a NUL-terminated string that stays valid for
/// the call.
unsafe fn c_bytes(text: *const c_char) -> Vec<u8> {
    if text.is_null() {
        Vec::new()
    } else {
        // SAFETY: the caller's contract.
        unsafe { CStr::from_ptr(text) }.to_bytes().to_vec()
    }
}

/// A copy of `entry`.
///
/// # Safety
///
/// Every string pointer in `entry` is null or a valid C string.
unsafe fn passwd_from(entry: &libc::passwd) -> Passwd {
    // SAFETY: the caller's contract, for each field.
    unsafe {
        Passwd {
            name: c_bytes(entry.pw_name),
            passwd: c_bytes(entry.pw_passwd),
            uid: entry.pw_uid,
            gid: entry.pw_gid,
            gecos: c_bytes(entry.pw_gecos),
            dir: c_bytes(entry.pw_dir),
            shell: c_bytes(entry.pw_shell),
        }
    }
}

/// A copy of `entry`.
///
/// # Safety
///
/// Every string pointer in `entry` is null or a valid C string, and
/// `gr_mem` is null or a null-terminated array of them.
unsafe fn group_from(entry: &libc::group) -> Group {
    let mut mem = Vec::new();
    let mut member = entry.gr_mem;
    // SAFETY: the caller's contract: the walk stops at the null that ends
    // the array, and each element before it is a C string.
    unsafe {
        while !member.is_null() && !(*member).is_null() {
            mem.push(c_bytes(*member));
            member = member.add(1);
        }
        Group {
            name: c_bytes(entry.gr_name),
            passwd: c_bytes(entry.gr_passwd),
            gid: entry.gr_gid,
            mem,
        }
    }
}

/// Call one of the reentrant lookups, `getpwuid_r` and the rest, growing its
/// buffer while it answers `ERANGE`. `None` when there is no such entry, and
/// also when the lookup fails: CRuby's `etc` reports both as "can't find".
fn lookup<E: Copy>(
    mut call: impl FnMut(&mut E, &mut [c_char], &mut *mut E) -> libc::c_int,
) -> Option<E> {
    // Big enough for most entries; a group with many members can need more.
    let mut buffer = vec![0 as c_char; 1024];
    loop {
        // SAFETY: `passwd` and `group` are plain C structs, for which all
        // zeroes is a valid value; the call fills it in.
        let mut entry: E = unsafe { std::mem::zeroed() };
        let mut found: *mut E = std::ptr::null_mut();
        match call(&mut entry, &mut buffer, &mut found) {
            0 => return (!found.is_null()).then_some(entry),
            libc::ERANGE if buffer.len() < 1 << 24 => {
                let len = buffer.len() * 2;
                buffer.resize(len, 0);
            }
            _ => return None,
        }
    }
}

/// `getpwuid_r(3)`.
pub fn passwd_by_uid(uid: u32) -> Option<Passwd> {
    let mut answer = None;
    lookup::<libc::passwd>(|entry, buffer, found| {
        // SAFETY: each pointer is valid and writable for the call, and
        // `buffer.len()` is its length.
        let status =
            unsafe { libc::getpwuid_r(uid, entry, buffer.as_mut_ptr(), buffer.len(), found) };
        if status == 0 && !found.is_null() {
            // SAFETY: on success the entry's strings point into `buffer`.
            answer = Some(unsafe { passwd_from(entry) });
        }
        status
    });
    answer
}

/// `getpwnam_r(3)`. A name with a NUL in it names no one.
pub fn passwd_by_name(name: &[u8]) -> Option<Passwd> {
    let Ok(name) = CString::new(name) else {
        return None;
    };
    let mut answer = None;
    lookup::<libc::passwd>(|entry, buffer, found| {
        // SAFETY: as in `passwd_by_uid`; `name` is a C string.
        let status = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                found,
            )
        };
        if status == 0 && !found.is_null() {
            // SAFETY: on success the entry's strings point into `buffer`.
            answer = Some(unsafe { passwd_from(entry) });
        }
        status
    });
    answer
}

/// `getgrgid_r(3)`.
pub fn group_by_gid(gid: u32) -> Option<Group> {
    let mut answer = None;
    lookup::<libc::group>(|entry, buffer, found| {
        // SAFETY: as in `passwd_by_uid`.
        let status =
            unsafe { libc::getgrgid_r(gid, entry, buffer.as_mut_ptr(), buffer.len(), found) };
        if status == 0 && !found.is_null() {
            // SAFETY: on success the entry's strings and member array point
            // into `buffer`.
            answer = Some(unsafe { group_from(entry) });
        }
        status
    });
    answer
}

/// `getgrnam_r(3)`. A name with a NUL in it names no group.
pub fn group_by_name(name: &[u8]) -> Option<Group> {
    let Ok(name) = CString::new(name) else {
        return None;
    };
    let mut answer = None;
    lookup::<libc::group>(|entry, buffer, found| {
        // SAFETY: as in `group_by_gid`; `name` is a C string.
        let status = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                found,
            )
        };
        if status == 0 && !found.is_null() {
            // SAFETY: as in `group_by_gid`.
            answer = Some(unsafe { group_from(entry) });
        }
        status
    });
    answer
}

/// What to do with the user or group database's cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// `setpwent` or `setgrent`: back to the first entry.
    Rewind,
    /// `getpwent` or `getgrent`: the next entry, `None` past the last.
    Next,
    /// `endpwent` or `endgrent`: close the database.
    Close,
}

/// `setpwent(3)`, `getpwent(3)` and `endpwent(3)`. The cursor is the C
/// library's, one per process, as it is for CRuby.
pub fn pwent(cursor: Cursor) -> Option<Passwd> {
    // SAFETY: none of the three takes an argument; `getpwent`'s answer is
    // null or an entry that stays valid until the next call, and is copied
    // before then.
    unsafe {
        match cursor {
            Cursor::Rewind => libc::setpwent(),
            Cursor::Close => libc::endpwent(),
            Cursor::Next => {
                let entry = libc::getpwent();
                return entry.as_ref().map(|entry| passwd_from(entry));
            }
        }
    }
    None
}

/// `setgrent(3)`, `getgrent(3)` and `endgrent(3)`, as `pwent`.
pub fn grent(cursor: Cursor) -> Option<Group> {
    // SAFETY: as in `pwent`.
    unsafe {
        match cursor {
            Cursor::Rewind => libc::setgrent(),
            Cursor::Close => libc::endgrent(),
            Cursor::Next => {
                let entry = libc::getgrent();
                return entry.as_ref().map(|entry| group_from(entry));
            }
        }
    }
    None
}

/// `getlogin(3)`, `None` when the process has no controlling terminal's
/// login to report.
pub fn login() -> Option<Vec<u8>> {
    // SAFETY: no arguments; the answer is null or a C string valid until the
    // next call, copied at once.
    let name = unsafe { libc::getlogin() };
    // SAFETY: as above.
    (!name.is_null()).then(|| unsafe { c_bytes(name) })
}

/// Reset `errno`, so a `-1` that is not an error can be told from one.
fn clear_errno() {
    // SAFETY: the location is this thread's `errno`, always writable.
    unsafe {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            *libc::__errno_location() = 0;
        }
        #[cfg(target_vendor = "apple")]
        {
            *libc::__error() = 0;
        }
    }
}

/// A `-1` that may be "no limit" (`Ok(None)`) or an error, by `errno`.
// `c_long` is `i64` on the 64-bit targets CI builds and narrower elsewhere,
// so the conversion is kept.
#[allow(clippy::useless_conversion)]
fn limit(answer: libc::c_long) -> io::Result<Option<i64>> {
    if answer != -1 {
        return Ok(Some(i64::from(answer)));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(0) | None => Ok(None),
        Some(_) => Err(error),
    }
}

/// `sysconf(3)`: the value, `None` for a variable with no limit.
pub fn sysconf(name: libc::c_int) -> io::Result<Option<i64>> {
    clear_errno();
    // SAFETY: takes an integer; an unknown one is EINVAL.
    limit(unsafe { libc::sysconf(name) })
}

/// `fpathconf(3)`, as `sysconf`.
pub fn fpathconf(fd: libc::c_int, name: libc::c_int) -> io::Result<Option<i64>> {
    clear_errno();
    // SAFETY: takes integers; a bad descriptor is EBADF.
    limit(unsafe { libc::fpathconf(fd, name) })
}

/// `confstr(3)`: the value, `None` for a variable with none.
pub fn confstr(name: libc::c_int) -> io::Result<Option<Vec<u8>>> {
    clear_errno();
    // SAFETY: a null buffer of length zero asks only for the length.
    let len = unsafe { libc::confstr(name, std::ptr::null_mut(), 0) };
    if len == 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(0) | None => Ok(None),
            Some(_) => Err(error),
        };
    }
    let mut buffer = vec![0 as c_char; len];
    // SAFETY: `buffer` has room for the `len` bytes asked for, the NUL
    // included.
    unsafe { libc::confstr(name, buffer.as_mut_ptr(), buffer.len()) };
    // SAFETY: `confstr` NUL-terminated what it wrote within `buffer`.
    Ok(Some(unsafe { c_bytes(buffer.as_ptr()) }))
}

/// `uname(2)`: sysname, nodename, release, version and machine.
pub fn uname() -> io::Result<[Vec<u8>; 5]> {
    // SAFETY: `utsname` is arrays of `c_char`, valid all zeroes.
    let mut names: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `names` is valid and writable for the call.
    if unsafe { libc::uname(&raw mut names) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let field = |chars: &[c_char]| -> Vec<u8> {
        chars
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect()
    };
    Ok([
        field(&names.sysname),
        field(&names.nodename),
        field(&names.release),
        field(&names.version),
        field(&names.machine),
    ])
}

/// The processors this process may run on: its CPU affinity mask, where
/// there is one, as CRuby counts them, or else those online.
pub fn nprocessors() -> io::Result<i64> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `cpu_set_t` is a bitmask, valid all zeroes; `set` is
        // valid and writable for the call and its size is passed.
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::cpu_set_t>();
        // SAFETY: as above; pid 0 is this thread.
        if unsafe { libc::sched_getaffinity(0, size, &raw mut set) } == 0 {
            // SAFETY: `set` was filled in.
            let count = unsafe { libc::CPU_COUNT(&set) };
            if count > 0 {
                return Ok(i64::from(count));
            }
        }
    }
    match sysconf(libc::_SC_NPROCESSORS_ONLN)? {
        Some(count) => Ok(count),
        None => Err(io::Error::from_raw_os_error(libc::EINVAL)),
    }
}

/// `Etc`'s `SC_*`, `CS_*` and `PC_*` constants: the ones CRuby 4.0 defines
/// on this platform, by name without the leading underscore.
pub fn constants() -> Vec<(&'static str, i64)> {
    macro_rules! table {
        ($($name:ident),* $(,)?) => {
            vec![$((&stringify!($name)[1..], i64::from(libc::$name))),*]
        };
    }
    // The `mut` is used only where the Linux-only names are added.
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut names = table![
        _SC_AIO_LISTIO_MAX,
        _SC_AIO_MAX,
        _SC_AIO_PRIO_DELTA_MAX,
        _SC_ARG_MAX,
        _SC_ATEXIT_MAX,
        _SC_BC_BASE_MAX,
        _SC_BC_DIM_MAX,
        _SC_BC_SCALE_MAX,
        _SC_BC_STRING_MAX,
        _SC_CHILD_MAX,
        _SC_CLK_TCK,
        _SC_COLL_WEIGHTS_MAX,
        _SC_DELAYTIMER_MAX,
        _SC_EXPR_NEST_MAX,
        _SC_HOST_NAME_MAX,
        _SC_IOV_MAX,
        _SC_LINE_MAX,
        _SC_LOGIN_NAME_MAX,
        _SC_NGROUPS_MAX,
        _SC_GETGR_R_SIZE_MAX,
        _SC_GETPW_R_SIZE_MAX,
        _SC_MQ_OPEN_MAX,
        _SC_MQ_PRIO_MAX,
        _SC_OPEN_MAX,
        _SC_ADVISORY_INFO,
        _SC_BARRIERS,
        _SC_ASYNCHRONOUS_IO,
        _SC_CLOCK_SELECTION,
        _SC_CPUTIME,
        _SC_FSYNC,
        _SC_IPV6,
        _SC_JOB_CONTROL,
        _SC_MAPPED_FILES,
        _SC_MEMLOCK,
        _SC_MEMLOCK_RANGE,
        _SC_MEMORY_PROTECTION,
        _SC_MESSAGE_PASSING,
        _SC_MONOTONIC_CLOCK,
        _SC_PRIORITIZED_IO,
        _SC_PRIORITY_SCHEDULING,
        _SC_RAW_SOCKETS,
        _SC_READER_WRITER_LOCKS,
        _SC_REALTIME_SIGNALS,
        _SC_REGEXP,
        _SC_SAVED_IDS,
        _SC_SEMAPHORES,
        _SC_SHARED_MEMORY_OBJECTS,
        _SC_SHELL,
        _SC_SPAWN,
        _SC_SPIN_LOCKS,
        _SC_SPORADIC_SERVER,
        _SC_SS_REPL_MAX,
        _SC_SYNCHRONIZED_IO,
        _SC_THREAD_ATTR_STACKADDR,
        _SC_THREAD_ATTR_STACKSIZE,
        _SC_THREAD_CPUTIME,
        _SC_THREAD_PRIO_INHERIT,
        _SC_THREAD_PRIO_PROTECT,
        _SC_THREAD_PRIORITY_SCHEDULING,
        _SC_THREAD_PROCESS_SHARED,
        _SC_THREAD_SAFE_FUNCTIONS,
        _SC_THREAD_SPORADIC_SERVER,
        _SC_THREADS,
        _SC_TIMEOUTS,
        _SC_TIMERS,
        _SC_TRACE,
        _SC_TRACE_EVENT_FILTER,
        _SC_TRACE_EVENT_NAME_MAX,
        _SC_TRACE_INHERIT,
        _SC_TRACE_LOG,
        _SC_TRACE_NAME_MAX,
        _SC_TRACE_SYS_MAX,
        _SC_TRACE_USER_EVENT_MAX,
        _SC_TYPED_MEMORY_OBJECTS,
        _SC_VERSION,
        _SC_V6_ILP32_OFF32,
        _SC_V6_ILP32_OFFBIG,
        _SC_V6_LP64_OFF64,
        _SC_V6_LPBIG_OFFBIG,
        _SC_2_C_BIND,
        _SC_2_C_DEV,
        _SC_2_CHAR_TERM,
        _SC_2_FORT_DEV,
        _SC_2_FORT_RUN,
        _SC_2_LOCALEDEF,
        _SC_2_PBS,
        _SC_2_PBS_ACCOUNTING,
        _SC_2_PBS_CHECKPOINT,
        _SC_2_PBS_LOCATE,
        _SC_2_PBS_MESSAGE,
        _SC_2_PBS_TRACK,
        _SC_2_SW_DEV,
        _SC_2_UPE,
        _SC_2_VERSION,
        _SC_PAGE_SIZE,
        _SC_PAGESIZE,
        _SC_THREAD_DESTRUCTOR_ITERATIONS,
        _SC_THREAD_KEYS_MAX,
        _SC_THREAD_STACK_MIN,
        _SC_THREAD_THREADS_MAX,
        _SC_RE_DUP_MAX,
        _SC_RTSIG_MAX,
        _SC_SEM_NSEMS_MAX,
        _SC_SEM_VALUE_MAX,
        _SC_SIGQUEUE_MAX,
        _SC_STREAM_MAX,
        _SC_SYMLOOP_MAX,
        _SC_TIMER_MAX,
        _SC_TTY_NAME_MAX,
        _SC_TZNAME_MAX,
        _SC_XOPEN_CRYPT,
        _SC_XOPEN_ENH_I18N,
        _SC_XOPEN_REALTIME,
        _SC_XOPEN_REALTIME_THREADS,
        _SC_XOPEN_SHM,
        _SC_XOPEN_STREAMS,
        _SC_XOPEN_UNIX,
        _SC_XOPEN_VERSION,
        _SC_PHYS_PAGES,
        _SC_NPROCESSORS_CONF,
        _SC_NPROCESSORS_ONLN,
        _CS_PATH,
        _PC_FILESIZEBITS,
        _PC_LINK_MAX,
        _PC_MAX_CANON,
        _PC_MAX_INPUT,
        _PC_NAME_MAX,
        _PC_PATH_MAX,
        _PC_PIPE_BUF,
        _PC_2_SYMLINKS,
        _PC_ALLOC_SIZE_MIN,
        _PC_REC_INCR_XFER_SIZE,
        _PC_REC_MAX_XFER_SIZE,
        _PC_REC_MIN_XFER_SIZE,
        _PC_REC_XFER_ALIGN,
        _PC_SYMLINK_MAX,
        _PC_CHOWN_RESTRICTED,
        _PC_NO_TRUNC,
        _PC_VDISABLE,
        _PC_ASYNC_IO,
        _PC_PRIO_IO,
        _PC_SYNC_IO,
    ];
    #[cfg(target_os = "linux")]
    names.extend(table![
        _SC_THREAD_ROBUST_PRIO_INHERIT,
        _SC_THREAD_ROBUST_PRIO_PROTECT,
        _SC_V7_ILP32_OFF32,
        _SC_V7_ILP32_OFFBIG,
        _SC_V7_LP64_OFF64,
        _SC_V7_LPBIG_OFFBIG,
        _SC_AVPHYS_PAGES,
        _CS_POSIX_V6_ILP32_OFF32_CFLAGS,
        _CS_POSIX_V6_ILP32_OFF32_LDFLAGS,
        _CS_POSIX_V6_ILP32_OFF32_LIBS,
        _CS_POSIX_V6_ILP32_OFFBIG_CFLAGS,
        _CS_POSIX_V6_ILP32_OFFBIG_LDFLAGS,
        _CS_POSIX_V6_ILP32_OFFBIG_LIBS,
        _CS_POSIX_V6_LP64_OFF64_CFLAGS,
        _CS_POSIX_V6_LP64_OFF64_LDFLAGS,
        _CS_POSIX_V6_LP64_OFF64_LIBS,
        _CS_POSIX_V6_LPBIG_OFFBIG_CFLAGS,
        _CS_POSIX_V6_LPBIG_OFFBIG_LDFLAGS,
        _CS_POSIX_V6_LPBIG_OFFBIG_LIBS,
        _CS_POSIX_V6_WIDTH_RESTRICTED_ENVS,
        _CS_POSIX_V7_ILP32_OFF32_CFLAGS,
        _CS_POSIX_V7_ILP32_OFF32_LDFLAGS,
        _CS_POSIX_V7_ILP32_OFF32_LIBS,
        _CS_POSIX_V7_ILP32_OFFBIG_CFLAGS,
        _CS_POSIX_V7_ILP32_OFFBIG_LDFLAGS,
        _CS_POSIX_V7_ILP32_OFFBIG_LIBS,
        _CS_POSIX_V7_LP64_OFF64_CFLAGS,
        _CS_POSIX_V7_LP64_OFF64_LDFLAGS,
        _CS_POSIX_V7_LP64_OFF64_LIBS,
        _CS_POSIX_V7_LPBIG_OFFBIG_CFLAGS,
        _CS_POSIX_V7_LPBIG_OFFBIG_LDFLAGS,
        _CS_POSIX_V7_LPBIG_OFFBIG_LIBS,
        _CS_POSIX_V7_WIDTH_RESTRICTED_ENVS,
        _CS_V6_ENV,
        _CS_V7_ENV,
        _CS_GNU_LIBC_VERSION,
        _CS_GNU_LIBPTHREAD_VERSION,
    ]);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    // The copies out of the C structs, which is where the pointers are
    // followed: run under miri against entries built here, since miri cannot
    // call the C library's lookups.

    #[test]
    fn passwd_from_copies_each_field_and_reads_null_as_empty() {
        let (name, shell) = (c"alice", c"/bin/sh");
        // SAFETY: all zeroes is a valid `passwd`.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        entry.pw_name = name.as_ptr().cast_mut();
        entry.pw_shell = shell.as_ptr().cast_mut();
        entry.pw_uid = 1000;
        entry.pw_gid = 100;
        // SAFETY: each pointer is null or one of the C strings above.
        let copy = unsafe { passwd_from(&entry) };
        assert_eq!(
            copy,
            Passwd {
                name: b"alice".to_vec(),
                passwd: Vec::new(),
                uid: 1000,
                gid: 100,
                gecos: Vec::new(),
                dir: Vec::new(),
                shell: b"/bin/sh".to_vec(),
            }
        );
    }

    #[test]
    fn group_from_walks_the_member_list_to_its_null() {
        let (name, a, b) = (c"wheel", c"alice", c"bob");
        let mut members = [
            a.as_ptr().cast_mut(),
            b.as_ptr().cast_mut(),
            std::ptr::null_mut(),
        ];
        // SAFETY: all zeroes is a valid `group`.
        let mut entry: libc::group = unsafe { std::mem::zeroed() };
        entry.gr_name = name.as_ptr().cast_mut();
        entry.gr_gid = 10;
        entry.gr_mem = members.as_mut_ptr();
        // SAFETY: the strings are C strings and the array ends in a null.
        let copy = unsafe { group_from(&entry) };
        assert_eq!(copy.name, b"wheel");
        assert_eq!(copy.passwd, b"");
        assert_eq!(copy.gid, 10);
        assert_eq!(copy.mem, vec![b"alice".to_vec(), b"bob".to_vec()]);

        entry.gr_mem = std::ptr::null_mut();
        // SAFETY: as above, with no member array at all.
        assert!(unsafe { group_from(&entry) }.mem.is_empty());
    }

    #[test]
    fn lookup_grows_its_buffer_until_the_entry_fits() {
        let mut sizes = Vec::new();
        let answer = lookup::<libc::group>(|entry, buffer, found| {
            sizes.push(buffer.len());
            if buffer.len() < 4096 {
                return libc::ERANGE;
            }
            entry.gr_gid = 7;
            *found = entry;
            0
        });
        assert_eq!(answer.map(|g| g.gr_gid), Some(7));
        assert_eq!(sizes, [1024, 2048, 4096]);
    }

    #[test]
    fn lookup_answers_none_for_no_entry_and_for_a_failure() {
        assert!(lookup::<libc::passwd>(|_, _, _| 0).is_none());
        assert!(lookup::<libc::passwd>(|_, _, _| libc::ENOENT).is_none());
        assert!(lookup::<libc::passwd>(|_, _, _| libc::EIO).is_none());
    }

    // The calls themselves, which miri cannot make.

    #[test]
    #[cfg_attr(miri, ignore)]
    fn this_process_user_and_group_are_found_by_id_and_by_name() {
        // SAFETY: no arguments.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let user = passwd_by_uid(uid).expect("the current user has an entry");
        assert_eq!(user.uid, uid);
        assert_eq!(passwd_by_name(&user.name), Some(user));
        assert_eq!(passwd_by_name(b"no\0one"), None);
        if let Some(group) = group_by_gid(gid) {
            assert_eq!(group.gid, gid);
            assert_eq!(group_by_name(&group.name), Some(group));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn sysconf_confstr_uname_and_the_processor_count() {
        assert!(sysconf(libc::_SC_PAGESIZE).unwrap().unwrap() > 0);
        assert!(sysconf(-1).is_err());
        assert!(!confstr(libc::_CS_PATH).unwrap().unwrap().is_empty());
        assert_eq!(confstr(-1).unwrap_err().raw_os_error(), Some(libc::EINVAL));
        assert!(!uname().unwrap()[0].is_empty());
        assert!(nprocessors().unwrap() >= 1);
        assert!(fpathconf(-1, libc::_PC_NAME_MAX).is_err());
    }
}
