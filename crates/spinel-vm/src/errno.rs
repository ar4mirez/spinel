//! The `Errno` table: which `Errno::E*` classes exist, and what number each is.
//!
//! The *names* are CRuby's, and they are the same on every platform: CRuby
//! defines one constant per entry of its `known_errors.def` whether or not the
//! platform has that error, and a name the platform lacks becomes an alias of
//! `Errno::NOERROR`. That is 158 constants on every Unix CRuby runs on.
//!
//! The *numbers* and *messages* are the platform's, and nothing here commits
//! either (#29, owner's decision). A number is the `libc` crate's constant for
//! the target being built, so `EAGAIN` is 11 on Linux and 35 on macOS without
//! a table that says so; a message is `strerror(3)`, read at run time through
//! `std`. Which names a target has is what the three lists below record — the
//! grouping is the `libc` crate's knowledge, and a name in the wrong group is a
//! compile error on that target rather than a wrong number.
//!
//! Measured against ruby 4.0.7 on Linux: the names missing from the Linux list
//! are exactly the ones CRuby aliases to `NOERROR` there. `ENOATTR` is the one
//! the `libc` crate exposes on Linux and CRuby does not define — glibc keeps it
//! out of `<errno.h>` — so it is listed for Apple only.

/// One `(name, number)` per name the platform defines, from `libc`.
macro_rules! errnos {
    ($($name:ident),* $(,)?) => {
        &[$((stringify!($name), libc::$name)),*]
    };
}

/// POSIX and common BSD/Linux errors, which both supported platforms define.
#[cfg(unix)]
#[rustfmt::skip]
const COMMON: &[(&str, i32)] = errnos![
    E2BIG, EACCES, EADDRINUSE, EADDRNOTAVAIL, EAFNOSUPPORT, EAGAIN, EALREADY, EBADF,
    EBADMSG, EBUSY, ECANCELED, ECHILD, ECONNABORTED, ECONNREFUSED, ECONNRESET, EDEADLK,
    EDESTADDRREQ, EDOM, EDQUOT, EEXIST, EFAULT, EFBIG, EHOSTDOWN, EHOSTUNREACH,
    EIDRM, EILSEQ, EINPROGRESS, EINTR, EINVAL, EIO, EISCONN, EISDIR,
    ELOOP, EMFILE, EMLINK, EMSGSIZE, EMULTIHOP, ENAMETOOLONG, ENETDOWN, ENETRESET,
    ENETUNREACH, ENFILE, ENOBUFS, ENODATA, ENODEV, ENOENT, ENOEXEC, ENOLCK,
    ENOLINK, ENOMEM, ENOMSG, ENOPROTOOPT, ENOSPC, ENOSR, ENOSTR, ENOSYS,
    ENOTBLK, ENOTCONN, ENOTDIR, ENOTEMPTY, ENOTRECOVERABLE, ENOTSOCK, ENOTSUP, ENOTTY,
    ENXIO, EOPNOTSUPP, EOVERFLOW, EOWNERDEAD, EPERM, EPFNOSUPPORT, EPIPE, EPROTO,
    EPROTONOSUPPORT, EPROTOTYPE, ERANGE, EREMOTE, EROFS, ESHUTDOWN, ESOCKTNOSUPPORT, ESPIPE,
    ESRCH, ESTALE, ETIME, ETIMEDOUT, ETOOMANYREFS, ETXTBSY, EUSERS, EWOULDBLOCK,
    EXDEV,
];

#[cfg(target_os = "linux")]
#[rustfmt::skip]
const PLATFORM: &[(&str, i32)] = errnos![
    EADV, EBADE, EBADFD, EBADR, EBADRQC, EBADSLT, EBFONT, ECHRNG,
    ECOMM, EDEADLOCK, EDOTDOT, EHWPOISON, EISNAM, EKEYEXPIRED, EKEYREJECTED, EKEYREVOKED,
    EL2HLT, EL2NSYNC, EL3HLT, EL3RST, ELIBACC, ELIBBAD, ELIBEXEC, ELIBMAX,
    ELIBSCN, ELNRNG, EMEDIUMTYPE, ENAVAIL, ENOANO, ENOCSI, ENOKEY, ENOMEDIUM,
    ENONET, ENOPKG, ENOTNAM, ENOTUNIQ, EREMCHG, EREMOTEIO, ERESTART, ERFKILL,
    ESRMNT, ESTRPIPE, EUCLEAN, EUNATCH, EXFULL,
];

#[cfg(target_vendor = "apple")]
#[rustfmt::skip]
const PLATFORM: &[(&str, i32)] = errnos![
    EAUTH, EBADARCH, EBADEXEC, EBADMACHO, EBADRPC, EDEVERR, EFTYPE, ELAST,
    ENEEDAUTH, ENOATTR, ENOPOLICY, ENOTCAPABLE, EPROCLIM, EPROCUNAVAIL, EPROGMISMATCH, EPROGUNAVAIL,
    EPWROFF, EQFULL, ERPCMISMATCH, ESHLIBVERS,
];

// ponytail: Unix only. Windows (#139) has its own errno numbering and its own
// `known_errors.def` subset; it gets a `PLATFORM` list of its own there.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
const PLATFORM: &[(&str, i32)] = &[];

/// Every name CRuby defines under `Errno`, whether or not this platform has it.
/// `NOERROR` is not here: it is number 0 and is defined first, separately.
#[rustfmt::skip]
const NAMES: [&str; 157] = [
    "E2BIG", "EACCES", "EADDRINUSE", "EADDRNOTAVAIL", "EADV", "EAFNOSUPPORT", "EAGAIN",
    "EALREADY", "EAUTH", "EBADARCH", "EBADE", "EBADEXEC", "EBADF", "EBADFD",
    "EBADMACHO", "EBADMSG", "EBADR", "EBADRPC", "EBADRQC", "EBADSLT", "EBFONT",
    "EBUSY", "ECANCELED", "ECAPMODE", "ECHILD", "ECHRNG", "ECOMM", "ECONNABORTED",
    "ECONNREFUSED", "ECONNRESET", "EDEADLK", "EDEADLOCK", "EDESTADDRREQ", "EDEVERR", "EDOM",
    "EDOOFUS", "EDOTDOT", "EDQUOT", "EEXIST", "EFAULT", "EFBIG", "EFTYPE",
    "EHOSTDOWN", "EHOSTUNREACH", "EHWPOISON", "EIDRM", "EILSEQ", "EINPROGRESS", "EINTR",
    "EINVAL", "EIO", "EIPSEC", "EISCONN", "EISDIR", "EISNAM", "EKEYEXPIRED",
    "EKEYREJECTED", "EKEYREVOKED", "EL2HLT", "EL2NSYNC", "EL3HLT", "EL3RST", "ELAST",
    "ELIBACC", "ELIBBAD", "ELIBEXEC", "ELIBMAX", "ELIBSCN", "ELNRNG", "ELOOP",
    "EMEDIUMTYPE", "EMFILE", "EMLINK", "EMSGSIZE", "EMULTIHOP", "ENAMETOOLONG", "ENAVAIL",
    "ENEEDAUTH", "ENETDOWN", "ENETRESET", "ENETUNREACH", "ENFILE", "ENOANO", "ENOATTR",
    "ENOBUFS", "ENOCSI", "ENODATA", "ENODEV", "ENOENT", "ENOEXEC", "ENOKEY",
    "ENOLCK", "ENOLINK", "ENOMEDIUM", "ENOMEM", "ENOMSG", "ENONET", "ENOPKG",
    "ENOPOLICY", "ENOPROTOOPT", "ENOSPC", "ENOSR", "ENOSTR", "ENOSYS", "ENOTBLK",
    "ENOTCAPABLE", "ENOTCONN", "ENOTDIR", "ENOTEMPTY", "ENOTNAM", "ENOTRECOVERABLE", "ENOTSOCK",
    "ENOTSUP", "ENOTTY", "ENOTUNIQ", "ENXIO", "EOPNOTSUPP", "EOVERFLOW", "EOWNERDEAD",
    "EPERM", "EPFNOSUPPORT", "EPIPE", "EPROCLIM", "EPROCUNAVAIL", "EPROGMISMATCH", "EPROGUNAVAIL",
    "EPROTO", "EPROTONOSUPPORT", "EPROTOTYPE", "EPWROFF", "EQFULL", "ERANGE", "EREMCHG",
    "EREMOTE", "EREMOTEIO", "ERESTART", "ERFKILL", "EROFS", "ERPCMISMATCH", "ESHLIBVERS",
    "ESHUTDOWN", "ESOCKTNOSUPPORT", "ESPIPE", "ESRCH", "ESRMNT", "ESTALE", "ESTRPIPE",
    "ETIME", "ETIMEDOUT", "ETOOMANYREFS", "ETXTBSY", "EUCLEAN", "EUNATCH", "EUSERS",
    "EWOULDBLOCK", "EXDEV", "EXFULL",
];

/// Every `Errno` constant in definition order, with this platform's number, or
/// 0 for a name the platform does not have — which is what makes it an alias of
/// `NOERROR`.
///
/// The order is alphabetical because `known_errors.def` is, and it decides
/// which name an aliased class is *called*: `EAGAIN` and `EWOULDBLOCK` are one
/// class on Linux, and it is `Errno::EAGAIN` because that name came first.
pub fn table() -> impl Iterator<Item = (&'static str, i32)> {
    std::iter::once(("NOERROR", 0)).chain(NAMES.iter().map(|&name| (name, number(name))))
}

fn number(name: &str) -> i32 {
    COMMON
        .iter()
        .chain(PLATFORM)
        .find(|&&(known, _)| known == name)
        .map_or(0, |&(_, number)| number)
}

/// `strerror(3)` for `number`, which is the message CRuby gives
/// `SystemCallError` — including the platform's own wording for a number it
/// does not know ("Unknown error 99999" on glibc, "Unknown error: 99999" on
/// macOS).
///
/// Through `std` rather than `libc::strerror_r`: glibc has two incompatible
/// `strerror_r`s and `std` already picks the right one. `std` appends
/// " (os error N)", which is its own and comes off — every copy of it, since
/// miri's `strerror` shim answers with the suffix already on.
#[must_use]
pub fn message(number: i32) -> String {
    let mut text = std::io::Error::from_raw_os_error(number).to_string();
    let suffix = format!(" (os error {number})");
    while let Some(stripped) = text.strip_suffix(&suffix) {
        text = stripped.to_owned();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_name_is_one_crubys_table_has() {
        for &(name, _) in COMMON.iter().chain(PLATFORM) {
            assert!(NAMES.contains(&name), "{name} is not in NAMES");
        }
    }

    #[test]
    fn names_are_sorted_and_unique() {
        assert!(NAMES.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_name_the_platform_lacks_is_zero() {
        assert_eq!(number("EDOOFUS"), 0);
        assert_eq!(number("EINVAL"), libc::EINVAL);
    }

    #[test]
    fn messages_are_strerror_without_stds_suffix() {
        assert_eq!(message(libc::ENOENT), "No such file or directory");
        assert_eq!(message(libc::EINVAL), "Invalid argument");
    }
}
