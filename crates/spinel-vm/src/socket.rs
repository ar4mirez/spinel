//! The socket system calls `lib/socket.rb` is Ruby over (#61).
//!
//! An address crosses to Ruby as the bytes of the platform's own `struct
//! sockaddr_*`, which is also what CRuby's `Addrinfo#to_sockaddr` and
//! `Socket.sockaddr_in` answer, so this module is the one place that knows
//! the layouts. Every call answers an `io::Error` (an errno for the Ruby side
//! to raise) or, for the resolver, `getaddrinfo(3)`'s code and message.
//! Descriptors are close-on-exec, as every other one Spinel opens.

use std::ffi::{CStr, CString};
use std::io;
use std::mem::{size_of, zeroed};

pub type Fd = libc::c_int;

/// `<netdb.h>`'s, on every target; the `libc` crate leaves it out.
const NI_MAXSERV: usize = 32;

// In every libc Spinel builds against, and missing from the `libc` crate.
unsafe extern "C" {
    fn inet_ntop(
        family: libc::c_int,
        source: *const libc::c_void,
        destination: *mut libc::c_char,
        size: libc::socklen_t,
    ) -> *const libc::c_char;
    fn inet_pton(
        family: libc::c_int,
        source: *const libc::c_char,
        destination: *mut libc::c_void,
    ) -> libc::c_int;
}

/// An address as `getaddrinfo(3)` answered it: family, socket type,
/// protocol, the `sockaddr`, and the canonical name when one was asked for.
pub type AddrInfo = (i32, i32, i32, Vec<u8>, Option<Vec<u8>>);

/// A control message: level, type and data.
pub type Control = (i32, i32, Vec<u8>);

/// What `recvmsg(2)` received: data, sender, `msg_flags`, control messages.
pub type Received = (Vec<u8>, Vec<u8>, i32, Vec<Control>);

/// One interface address from `getifaddrs(3)`: name, flags, index, and the
/// address, netmask, broadcast and destination, each a `sockaddr` if set.
pub type IfAddr = (
    Vec<u8>,
    u32,
    u32,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

/// A `sockaddr` read back: what `Socket.unpack_sockaddr_in` and `_un` and
/// `Addrinfo` need from one.
pub enum Parsed {
    Inet {
        port: u16,
        addr: [u8; 4],
    },
    Inet6 {
        port: u16,
        addr: [u8; 16],
        flowinfo: u32,
        scope_id: u32,
    },
    Unix(Vec<u8>),
    Other(i32),
}

fn check(answer: libc::c_int) -> io::Result<libc::c_int> {
    if answer < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(answer)
    }
}

fn cloexec(fd: Fd) {
    // SAFETY: integer `fcntl`s on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// `bytes` copied into a `sockaddr_storage`, which is aligned for every
/// address type, with its length; EINVAL when it cannot be one.
fn storage(bytes: &[u8]) -> io::Result<(libc::sockaddr_storage, libc::socklen_t)> {
    if bytes.len() > size_of::<libc::sockaddr_storage>() {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: an all-zero `sockaddr_storage` is a valid one.
    let mut out: libc::sockaddr_storage = unsafe { zeroed() };
    // SAFETY: `out` has room for `bytes`, checked above, and the two do not
    // overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), (&raw mut out).cast::<u8>(), bytes.len());
    }
    Ok((out, bytes.len() as libc::socklen_t))
}

fn storage_bytes(storage: &libc::sockaddr_storage, len: libc::socklen_t) -> Vec<u8> {
    let len = (len as usize).min(size_of::<libc::sockaddr_storage>());
    // SAFETY: `storage` is readable for its whole size, and `len` is no more.
    unsafe {
        std::slice::from_raw_parts((storage as *const libc::sockaddr_storage).cast::<u8>(), len)
    }
    .to_vec()
}

pub fn socket(domain: i32, kind: i32, protocol: i32) -> io::Result<Fd> {
    // SAFETY: `socket` takes three integers and touches no memory.
    let fd = check(unsafe { libc::socket(domain, kind, protocol) })?;
    cloexec(fd);
    Ok(fd)
}

pub fn socketpair(domain: i32, kind: i32, protocol: i32) -> io::Result<[Fd; 2]> {
    let mut fds = [0 as Fd; 2];
    // SAFETY: `fds` is room for the two descriptors `socketpair` writes.
    check(unsafe { libc::socketpair(domain, kind, protocol, fds.as_mut_ptr()) })?;
    fds.iter().for_each(|&fd| cloexec(fd));
    Ok(fds)
}

pub fn bind(fd: Fd, address: &[u8]) -> io::Result<()> {
    let (address, len) = storage(address)?;
    // SAFETY: `address` is a `sockaddr` readable for `len` bytes.
    check(unsafe { libc::bind(fd, (&raw const address).cast(), len) }).map(drop)
}

pub fn connect(fd: Fd, address: &[u8]) -> io::Result<()> {
    let (address, len) = storage(address)?;
    // SAFETY: `address` is a `sockaddr` readable for `len` bytes.
    check(unsafe { libc::connect(fd, (&raw const address).cast(), len) }).map(drop)
}

pub fn listen(fd: Fd, backlog: i32) -> io::Result<()> {
    // SAFETY: `listen` takes two integers and touches no memory.
    check(unsafe { libc::listen(fd, backlog) }).map(drop)
}

pub fn accept(fd: Fd) -> io::Result<(Fd, Vec<u8>)> {
    // SAFETY: an all-zero `sockaddr_storage` is a valid one.
    let mut address: libc::sockaddr_storage = unsafe { zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: `address` is writable for `len` bytes, and `accept` writes no
    // more and says how many it did.
    let client = check(unsafe { libc::accept(fd, (&raw mut address).cast(), &raw mut len) })?;
    cloexec(client);
    Ok((client, storage_bytes(&address, len)))
}

/// `getsockname(2)`, or `getpeername(2)` when `peer`.
pub fn name(fd: Fd, peer: bool) -> io::Result<Vec<u8>> {
    // SAFETY: an all-zero `sockaddr_storage` is a valid one.
    let mut address: libc::sockaddr_storage = unsafe { zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let at = (&raw mut address).cast();
    // SAFETY: `address` is writable for `len` bytes, and neither call writes
    // more.
    check(unsafe {
        if peer {
            libc::getpeername(fd, at, &raw mut len)
        } else {
            libc::getsockname(fd, at, &raw mut len)
        }
    })?;
    Ok(storage_bytes(&address, len))
}

pub fn getsockopt(fd: Fd, level: i32, option: i32) -> io::Result<Vec<u8>> {
    // Room for the largest option read whole, `TCP_INFO`.
    let mut buffer = vec![0u8; 1024];
    let mut len = buffer.len() as libc::socklen_t;
    // SAFETY: `buffer` is writable for `len` bytes, and `getsockopt` writes no
    // more and says how many it did.
    check(unsafe {
        libc::getsockopt(fd, level, option, buffer.as_mut_ptr().cast(), &raw mut len)
    })?;
    buffer.truncate(len as usize);
    Ok(buffer)
}

pub fn setsockopt(fd: Fd, level: i32, option: i32, value: &[u8]) -> io::Result<()> {
    // SAFETY: `value` is readable for its length.
    check(unsafe {
        libc::setsockopt(
            fd,
            level,
            option,
            value.as_ptr().cast(),
            value.len() as libc::socklen_t,
        )
    })
    .map(drop)
}

/// `sendto(2)`, or `send(2)` without a destination.
pub fn send(fd: Fd, data: &[u8], flags: i32, to: Option<&[u8]>) -> io::Result<usize> {
    let sent = match to {
        None => {
            // SAFETY: `data` is readable for its length.
            unsafe { libc::send(fd, data.as_ptr().cast(), data.len(), flags) }
        }
        Some(to) => {
            let (to, len) = storage(to)?;
            // SAFETY: `data` is readable for its length, `to` for `len`.
            unsafe {
                libc::sendto(
                    fd,
                    data.as_ptr().cast(),
                    data.len(),
                    flags,
                    (&raw const to).cast(),
                    len,
                )
            }
        }
    };
    if sent < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(sent as usize)
    }
}

/// `recvfrom(2)`: the data and who sent it, which is no bytes for a
/// connected stream.
pub fn recv(fd: Fd, max: usize, flags: i32) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buffer = vec![0u8; max];
    // SAFETY: an all-zero `sockaddr_storage` is a valid one.
    let mut from: libc::sockaddr_storage = unsafe { zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: `buffer` is writable for its length and `from` for `len`, and
    // `recvfrom` writes no more than either.
    let got = unsafe {
        libc::recvfrom(
            fd,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            flags,
            (&raw mut from).cast(),
            &raw mut len,
        )
    };
    if got < 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(got as usize);
    Ok((buffer, storage_bytes(&from, len)))
}

pub fn shutdown(fd: Fd, how: i32) -> io::Result<()> {
    // SAFETY: `shutdown` takes two integers and touches no memory.
    check(unsafe { libc::shutdown(fd, how) }).map(drop)
}

pub fn sendmsg(
    fd: Fd,
    data: &[u8],
    flags: i32,
    to: Option<&[u8]>,
    controls: &[Control],
) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr().cast_mut().cast(),
        iov_len: data.len(),
    };
    // SAFETY: an all-zero `msghdr` is an empty one.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    let mut to_storage = None;
    if let Some(to) = to {
        to_storage = Some(storage(to)?);
    }
    if let Some((address, len)) = to_storage.as_mut() {
        message.msg_name = (address as *mut libc::sockaddr_storage).cast();
        message.msg_namelen = *len;
    }
    // SAFETY: `CMSG_SPACE` is arithmetic on a length.
    let space: usize = controls
        .iter()
        .map(|(_, _, data)| unsafe { libc::CMSG_SPACE(data.len() as u32) } as usize)
        .sum();
    // u64s, for the alignment a `cmsghdr` needs.
    let mut control = vec![0u64; space.div_ceil(8)];
    if !controls.is_empty() {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = space as _;
        // SAFETY: `msg_control` is `space` bytes, room for every message as
        // `CMSG_SPACE` measured them, so each `CMSG_NXTHDR` stays inside it
        // and each data copy fits its header's `CMSG_LEN`.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&raw const message);
            for (level, kind, data) in controls {
                if header.is_null() {
                    break;
                }
                (*header).cmsg_level = *level;
                (*header).cmsg_type = *kind;
                (*header).cmsg_len = libc::CMSG_LEN(data.len() as u32) as _;
                std::ptr::copy_nonoverlapping(data.as_ptr(), libc::CMSG_DATA(header), data.len());
                header = libc::CMSG_NXTHDR(&raw const message, header);
            }
        }
    }
    // SAFETY: every pointer in `message` is to memory alive for the call.
    let sent = unsafe { libc::sendmsg(fd, &raw const message, flags) };
    if sent < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(sent as usize)
    }
}

pub fn recvmsg(fd: Fd, max: usize, flags: i32, control_max: usize) -> io::Result<Received> {
    let mut buffer = vec![0u8; max];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: an all-zero `sockaddr_storage` is a valid one, and an all-zero
    // `msghdr` an empty one.
    let mut from: libc::sockaddr_storage = unsafe { zeroed() };
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_name = (&raw mut from).cast();
    message.msg_namelen = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    let mut control = vec![0u64; control_max.div_ceil(8)];
    if control_max > 0 {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = (control.len() * 8) as _;
    }
    // SAFETY: every pointer in `message` is to memory alive for the call,
    // with the lengths beside them.
    let got = unsafe { libc::recvmsg(fd, &raw mut message, flags) };
    if got < 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(got as usize);
    let mut controls = Vec::new();
    if control_max > 0 {
        // SAFETY: the kernel wrote `msg_controllen` bytes of whole control
        // messages, which `CMSG_FIRSTHDR` and `CMSG_NXTHDR` walk without
        // leaving, and each one's data is `cmsg_len` less its header.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&raw const message);
            while !header.is_null() {
                let len = (*header).cmsg_len as usize;
                let data_len = len.saturating_sub(libc::CMSG_LEN(0) as usize);
                let data = std::slice::from_raw_parts(libc::CMSG_DATA(header), data_len).to_vec();
                controls.push(((*header).cmsg_level, (*header).cmsg_type, data));
                header = libc::CMSG_NXTHDR(&raw const message, header);
            }
        }
    }
    let from = storage_bytes(&from, message.msg_namelen);
    Ok((buffer, from, message.msg_flags, controls))
}

fn gai_error(code: i32) -> (i32, String) {
    let message = if code == libc::EAI_SYSTEM {
        io::Error::last_os_error().to_string()
    } else {
        // SAFETY: `gai_strerror` answers a static C string for any code.
        unsafe { CStr::from_ptr(libc::gai_strerror(code)) }
            .to_string_lossy()
            .into_owned()
    };
    (code, message)
}

fn c_string(bytes: Option<&[u8]>) -> Result<Option<CString>, (i32, String)> {
    match bytes {
        None => Ok(None),
        Some(bytes) => CString::new(bytes)
            .map(Some)
            .map_err(|_| gai_error(libc::EAI_NONAME)),
    }
}

pub fn getaddrinfo(
    host: Option<&[u8]>,
    service: Option<&[u8]>,
    family: i32,
    kind: i32,
    protocol: i32,
    flags: i32,
) -> Result<Vec<AddrInfo>, (i32, String)> {
    let host = c_string(host)?;
    let service = c_string(service)?;
    // SAFETY: an all-zero `addrinfo` is the empty hints.
    let mut hints: libc::addrinfo = unsafe { zeroed() };
    hints.ai_family = family;
    hints.ai_socktype = kind;
    hints.ai_protocol = protocol;
    hints.ai_flags = flags;
    let mut list: *mut libc::addrinfo = std::ptr::null_mut();
    // SAFETY: the strings are NUL-terminated or null, `hints` is a valid
    // `addrinfo`, and `list` is where the answer is written.
    let code = unsafe {
        libc::getaddrinfo(
            host.as_ref().map_or(std::ptr::null(), |h| h.as_ptr()),
            service.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            &raw const hints,
            &raw mut list,
        )
    };
    if code != 0 {
        return Err(gai_error(code));
    }
    let mut out = Vec::new();
    let mut at = list;
    // SAFETY: `getaddrinfo` answered a list of entries ending in null, each
    // with an address of `ai_addrlen` bytes and a canonical name that is a C
    // string or null; it is freed once, after it is read.
    unsafe {
        while !at.is_null() {
            let entry = &*at;
            let address =
                std::slice::from_raw_parts(entry.ai_addr.cast::<u8>(), entry.ai_addrlen as usize)
                    .to_vec();
            let canonical = (!entry.ai_canonname.is_null())
                .then(|| CStr::from_ptr(entry.ai_canonname).to_bytes().to_vec());
            out.push((
                entry.ai_family,
                entry.ai_socktype,
                entry.ai_protocol,
                address,
                canonical,
            ));
            at = entry.ai_next;
        }
        libc::freeaddrinfo(list);
    }
    Ok(out)
}

pub fn getnameinfo(address: &[u8], flags: i32) -> Result<(Vec<u8>, Vec<u8>), (i32, String)> {
    let (address, len) = storage(address).map_err(|_| gai_error(libc::EAI_FAMILY))?;
    let mut host = [0 as libc::c_char; libc::NI_MAXHOST as usize];
    let mut service = [0 as libc::c_char; NI_MAXSERV];
    // SAFETY: `address` is a `sockaddr` of `len` bytes, and the two buffers
    // are writable for the lengths passed beside them.
    let code = unsafe {
        libc::getnameinfo(
            (&raw const address).cast(),
            len,
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            service.as_mut_ptr(),
            service.len() as libc::socklen_t,
            flags,
        )
    };
    if code != 0 {
        return Err(gai_error(code));
    }
    // SAFETY: `getnameinfo` NUL-terminates both on success.
    let read = |buffer: &[libc::c_char]| {
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_bytes()
            .to_vec()
    };
    Ok((read(&host), read(&service)))
}

pub fn gethostname() -> io::Result<Vec<u8>> {
    let mut buffer = [0 as libc::c_char; 256];
    // SAFETY: `buffer` is writable for its length, and the last byte is left
    // for the terminator `gethostname` may not write.
    check(unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len() - 1) })?;
    // SAFETY: the buffer was zeroed, so it is NUL-terminated.
    Ok(unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_bytes()
        .to_vec())
}

/// `getservbyname(3)`: the port in host order.
///
/// ponytail: the non-reentrant call, whose answer is libc's static buffer,
/// read at once; one OS thread per Ractor never races it until Ractors run
/// in parallel (#119), when `getservbyname_r` where there is one is the
/// upgrade.
pub fn service_port(name: &[u8], protocol: &[u8]) -> Option<u16> {
    let name = CString::new(name).ok()?;
    let protocol = CString::new(protocol).ok()?;
    // SAFETY: both are C strings; the answer is read before any other call.
    let entry = unsafe { libc::getservbyname(name.as_ptr(), protocol.as_ptr()) };
    if entry.is_null() {
        return None;
    }
    // SAFETY: a non-null answer is a valid `servent`.
    Some(u16::from_be(unsafe { (*entry).s_port } as u16))
}

/// `getservbyport(3)`: the service's name. As [`service_port`], the call
/// is the non-reentrant one.
pub fn service_name(port: u16, protocol: &[u8]) -> Option<Vec<u8>> {
    let protocol = CString::new(protocol).ok()?;
    // SAFETY: `protocol` is a C string; the answer is read at once.
    let entry = unsafe { libc::getservbyport(i32::from(port.to_be()), protocol.as_ptr()) };
    if entry.is_null() {
        return None;
    }
    // SAFETY: a non-null answer's name is a C string.
    Some(
        unsafe { CStr::from_ptr((*entry).s_name) }
            .to_bytes()
            .to_vec(),
    )
}

pub fn if_nametoindex(name: &[u8]) -> u32 {
    let Ok(name) = CString::new(name) else {
        return 0;
    };
    // SAFETY: `name` is a C string.
    unsafe { libc::if_nametoindex(name.as_ptr()) }
}

pub fn if_indextoname(index: u32) -> Option<Vec<u8>> {
    let mut buffer = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: `buffer` is `IF_NAMESIZE` bytes, as `if_indextoname` needs.
    let answer = unsafe { libc::if_indextoname(index, buffer.as_mut_ptr()) };
    if answer.is_null() {
        return None;
    }
    // SAFETY: a non-null answer is the NUL-terminated `buffer`.
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_bytes()
            .to_vec(),
    )
}

/// How many bytes of `address` are the `sockaddr`, for one `getifaddrs`
/// hands over without a length.
fn sockaddr_len(address: *const libc::sockaddr) -> usize {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    {
        // SAFETY: the caller's address is a valid `sockaddr`.
        return usize::from(unsafe { (*address).sa_len });
    }
    #[allow(unreachable_code)]
    {
        // SAFETY: the caller's address is a valid `sockaddr`.
        match i32::from(unsafe { (*address).sa_family }) {
            libc::AF_INET => size_of::<libc::sockaddr_in>(),
            libc::AF_INET6 => size_of::<libc::sockaddr_in6>(),
            #[cfg(target_os = "linux")]
            libc::AF_PACKET => size_of::<libc::sockaddr_ll>(),
            _ => size_of::<libc::sockaddr>(),
        }
    }
}

pub fn getifaddrs() -> io::Result<Vec<IfAddr>> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is where the answer is written.
    check(unsafe { libc::getifaddrs(&raw mut list) })?;
    let mut out = Vec::new();
    let mut at = list;
    // SAFETY: `getifaddrs` answered a list ending in null whose entries'
    // names are C strings and whose addresses are valid or null; it is freed
    // once, after it is read.
    unsafe {
        let copy = |address: *mut libc::sockaddr| {
            (!address.is_null()).then(|| {
                std::slice::from_raw_parts(address.cast::<u8>(), sockaddr_len(address)).to_vec()
            })
        };
        while !at.is_null() {
            let entry = &*at;
            let name = CStr::from_ptr(entry.ifa_name).to_bytes().to_vec();
            let index = libc::if_nametoindex(entry.ifa_name);
            #[cfg(target_os = "linux")]
            let other = entry.ifa_ifu;
            #[cfg(not(target_os = "linux"))]
            let other = entry.ifa_dstaddr;
            let broadcast = entry.ifa_flags & (libc::IFF_BROADCAST as u32) != 0;
            let point = entry.ifa_flags & (libc::IFF_POINTOPOINT as u32) != 0;
            out.push((
                name,
                entry.ifa_flags,
                index,
                copy(entry.ifa_addr),
                copy(entry.ifa_netmask),
                if broadcast { copy(other) } else { None },
                if point { copy(other) } else { None },
            ));
            at = entry.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    Ok(out)
}

/// A `sockaddr_in` or `sockaddr_in6` for `addr`, which is 4 or 16 bytes in
/// network order.
pub fn sockaddr_in(addr: &[u8], port: u16, flowinfo: u32, scope_id: u32) -> Option<Vec<u8>> {
    match addr.len() {
        4 => {
            // SAFETY: an all-zero `sockaddr_in` is a valid one.
            let mut out: libc::sockaddr_in = unsafe { zeroed() };
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                out.sin_len = size_of::<libc::sockaddr_in>() as u8;
            }
            out.sin_family = libc::AF_INET as libc::sa_family_t;
            out.sin_port = port.to_be();
            out.sin_addr.s_addr = u32::from_ne_bytes(addr.try_into().ok()?);
            Some(raw_bytes(&out))
        }
        16 => {
            // SAFETY: an all-zero `sockaddr_in6` is a valid one.
            let mut out: libc::sockaddr_in6 = unsafe { zeroed() };
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                out.sin6_len = size_of::<libc::sockaddr_in6>() as u8;
            }
            out.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            out.sin6_port = port.to_be();
            out.sin6_flowinfo = flowinfo.to_be();
            out.sin6_addr.s6_addr = addr.try_into().ok()?;
            out.sin6_scope_id = scope_id;
            Some(raw_bytes(&out))
        }
        _ => None,
    }
}

/// A `sockaddr_un` for `path`, as long as it is: CRuby's is no longer than
/// the path needs. None when the path does not fit.
pub fn sockaddr_un(path: &[u8]) -> Option<Vec<u8>> {
    // SAFETY: an all-zero `sockaddr_un` is a valid one.
    let mut out: libc::sockaddr_un = unsafe { zeroed() };
    if path.len() > out.sun_path.len() {
        return None;
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    {
        out.sun_len = size_of::<libc::sockaddr_un>() as u8;
    }
    out.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, &byte) in out.sun_path.iter_mut().zip(path) {
        *slot = byte as libc::c_char;
    }
    Some(raw_bytes(&out))
}

fn raw_bytes<T>(value: &T) -> Vec<u8> {
    // SAFETY: `value` is a plain C struct readable for its size.
    unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }.to_vec()
}

/// The family of a `sockaddr`'s bytes; None when they are too short to say.
pub fn family(bytes: &[u8]) -> Option<i32> {
    let offset = std::mem::offset_of!(libc::sockaddr, sa_family);
    let size = size_of::<libc::sa_family_t>();
    let field = bytes.get(offset..offset + size)?;
    Some(if size == 1 {
        i32::from(field[0])
    } else {
        i32::from(u16::from_ne_bytes([field[0], field[1]]))
    })
}

pub fn parse(bytes: &[u8]) -> Option<Parsed> {
    let family = family(bytes)?;
    let (address, _) = storage(bytes).ok()?;
    let at = &raw const address;
    match family {
        libc::AF_INET if bytes.len() >= size_of::<libc::sockaddr_in>() => {
            // SAFETY: `address` holds a `sockaddr_in`'s bytes, and storage is
            // aligned for one.
            let inet = unsafe { &*at.cast::<libc::sockaddr_in>() };
            Some(Parsed::Inet {
                port: u16::from_be(inet.sin_port),
                addr: inet.sin_addr.s_addr.to_ne_bytes(),
            })
        }
        libc::AF_INET6 if bytes.len() >= size_of::<libc::sockaddr_in6>() => {
            // SAFETY: as above, for a `sockaddr_in6`.
            let inet = unsafe { &*at.cast::<libc::sockaddr_in6>() };
            Some(Parsed::Inet6 {
                port: u16::from_be(inet.sin6_port),
                addr: inet.sin6_addr.s6_addr,
                flowinfo: u32::from_be(inet.sin6_flowinfo),
                scope_id: inet.sin6_scope_id,
            })
        }
        libc::AF_UNIX => {
            let start = std::mem::offset_of!(libc::sockaddr_un, sun_path);
            let path = bytes.get(start..).unwrap_or_default();
            let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
            Some(Parsed::Unix(path[..end].to_vec()))
        }
        family => Some(Parsed::Other(family)),
    }
}

/// `inet_ntop(3)`: an address's bytes, 4 or 16, as text.
pub fn ntop(addr: &[u8]) -> Option<String> {
    let family = match addr.len() {
        4 => libc::AF_INET,
        16 => libc::AF_INET6,
        _ => return None,
    };
    let mut buffer = [0 as libc::c_char; 64];
    // SAFETY: `addr` is an address of `family`'s size, and `buffer` is
    // writable for the length passed.
    let answer = unsafe {
        inet_ntop(
            family,
            addr.as_ptr().cast(),
            buffer.as_mut_ptr(),
            buffer.len() as libc::socklen_t,
        )
    };
    if answer.is_null() {
        return None;
    }
    // SAFETY: a non-null answer is the NUL-terminated `buffer`.
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// `inet_pton(3)`: text as an address of `family`, or None if it is not one.
pub fn pton(family: i32, text: &[u8]) -> Option<Vec<u8>> {
    let text = CString::new(text).ok()?;
    let mut buffer = [0u8; 16];
    // SAFETY: `text` is a C string, and `buffer` has room for either family.
    let answer = unsafe { inet_pton(family, text.as_ptr(), buffer.as_mut_ptr().cast()) };
    if answer != 1 {
        return None;
    }
    Some(buffer[..if family == libc::AF_INET { 4 } else { 16 }].to_vec())
}

/// `Socket::Constants`: CRuby's list, with each value this platform's.
// `SOCK_PACKET` is deprecated for `AF_PACKET`, and CRuby still defines it.
#[allow(deprecated)]
pub fn constants() -> Vec<(&'static str, i64)> {
    #[allow(clippy::unnecessary_cast)]
    let mut out = vec![
        ("AF_APPLETALK", libc::AF_APPLETALK as i64),
        ("AF_DECnet", libc::AF_DECnet as i64),
        ("AF_INET", libc::AF_INET as i64),
        ("AF_INET6", libc::AF_INET6 as i64),
        ("AF_IPX", libc::AF_IPX as i64),
        ("AF_ISDN", libc::AF_ISDN as i64),
        ("AF_LOCAL", libc::AF_LOCAL as i64),
        ("AF_ROUTE", libc::AF_ROUTE as i64),
        ("AF_SNA", libc::AF_SNA as i64),
        ("AF_UNIX", libc::AF_UNIX as i64),
        ("AF_UNSPEC", libc::AF_UNSPEC as i64),
        ("AF_VSOCK", libc::AF_VSOCK as i64),
        ("AI_ADDRCONFIG", libc::AI_ADDRCONFIG as i64),
        ("AI_ALL", libc::AI_ALL as i64),
        ("AI_CANONNAME", libc::AI_CANONNAME as i64),
        ("AI_NUMERICHOST", libc::AI_NUMERICHOST as i64),
        ("AI_NUMERICSERV", libc::AI_NUMERICSERV as i64),
        ("AI_PASSIVE", libc::AI_PASSIVE as i64),
        ("AI_V4MAPPED", libc::AI_V4MAPPED as i64),
        ("EAI_AGAIN", libc::EAI_AGAIN as i64),
        ("EAI_BADFLAGS", libc::EAI_BADFLAGS as i64),
        ("EAI_FAIL", libc::EAI_FAIL as i64),
        ("EAI_FAMILY", libc::EAI_FAMILY as i64),
        ("EAI_MEMORY", libc::EAI_MEMORY as i64),
        ("EAI_NODATA", libc::EAI_NODATA as i64),
        ("EAI_NONAME", libc::EAI_NONAME as i64),
        ("EAI_OVERFLOW", libc::EAI_OVERFLOW as i64),
        ("EAI_SERVICE", libc::EAI_SERVICE as i64),
        ("EAI_SOCKTYPE", libc::EAI_SOCKTYPE as i64),
        ("EAI_SYSTEM", libc::EAI_SYSTEM as i64),
        ("IFF_ALLMULTI", libc::IFF_ALLMULTI as i64),
        ("IFF_BROADCAST", libc::IFF_BROADCAST as i64),
        ("IFF_DEBUG", libc::IFF_DEBUG as i64),
        ("IFF_LOOPBACK", libc::IFF_LOOPBACK as i64),
        ("IFF_MULTICAST", libc::IFF_MULTICAST as i64),
        ("IFF_NOARP", libc::IFF_NOARP as i64),
        ("IFF_NOTRAILERS", libc::IFF_NOTRAILERS as i64),
        ("IFF_POINTOPOINT", libc::IFF_POINTOPOINT as i64),
        ("IFF_PROMISC", libc::IFF_PROMISC as i64),
        ("IFF_RUNNING", libc::IFF_RUNNING as i64),
        ("IFF_UP", libc::IFF_UP as i64),
        ("IFNAMSIZ", libc::IFNAMSIZ as i64),
        ("IF_NAMESIZE", libc::IF_NAMESIZE as i64),
        ("INADDR_ANY", libc::INADDR_ANY as i64),
        ("INADDR_BROADCAST", libc::INADDR_BROADCAST as i64),
        ("INADDR_LOOPBACK", libc::INADDR_LOOPBACK as i64),
        ("INADDR_NONE", libc::INADDR_NONE as i64),
        ("IPPROTO_AH", libc::IPPROTO_AH as i64),
        ("IPPROTO_DSTOPTS", libc::IPPROTO_DSTOPTS as i64),
        ("IPPROTO_EGP", libc::IPPROTO_EGP as i64),
        ("IPPROTO_ESP", libc::IPPROTO_ESP as i64),
        ("IPPROTO_FRAGMENT", libc::IPPROTO_FRAGMENT as i64),
        ("IPPROTO_HOPOPTS", libc::IPPROTO_HOPOPTS as i64),
        ("IPPROTO_ICMP", libc::IPPROTO_ICMP as i64),
        ("IPPROTO_ICMPV6", libc::IPPROTO_ICMPV6 as i64),
        ("IPPROTO_IDP", libc::IPPROTO_IDP as i64),
        ("IPPROTO_IGMP", libc::IPPROTO_IGMP as i64),
        ("IPPROTO_IP", libc::IPPROTO_IP as i64),
        ("IPPROTO_IPV6", libc::IPPROTO_IPV6 as i64),
        ("IPPROTO_NONE", libc::IPPROTO_NONE as i64),
        ("IPPROTO_PUP", libc::IPPROTO_PUP as i64),
        ("IPPROTO_RAW", libc::IPPROTO_RAW as i64),
        ("IPPROTO_ROUTING", libc::IPPROTO_ROUTING as i64),
        ("IPPROTO_TCP", libc::IPPROTO_TCP as i64),
        ("IPPROTO_TP", libc::IPPROTO_TP as i64),
        ("IPPROTO_UDP", libc::IPPROTO_UDP as i64),
        ("IPV6_CHECKSUM", libc::IPV6_CHECKSUM as i64),
        ("IPV6_HOPLIMIT", libc::IPV6_HOPLIMIT as i64),
        ("IPV6_MULTICAST_HOPS", libc::IPV6_MULTICAST_HOPS as i64),
        ("IPV6_MULTICAST_IF", libc::IPV6_MULTICAST_IF as i64),
        ("IPV6_MULTICAST_LOOP", libc::IPV6_MULTICAST_LOOP as i64),
        ("IPV6_PKTINFO", libc::IPV6_PKTINFO as i64),
        ("IPV6_RECVHOPLIMIT", libc::IPV6_RECVHOPLIMIT as i64),
        ("IPV6_RECVPKTINFO", libc::IPV6_RECVPKTINFO as i64),
        ("IPV6_RECVTCLASS", libc::IPV6_RECVTCLASS as i64),
        ("IPV6_TCLASS", libc::IPV6_TCLASS as i64),
        ("IPV6_UNICAST_HOPS", libc::IPV6_UNICAST_HOPS as i64),
        ("IPV6_V6ONLY", libc::IPV6_V6ONLY as i64),
        ("IP_ADD_MEMBERSHIP", libc::IP_ADD_MEMBERSHIP as i64),
        (
            "IP_ADD_SOURCE_MEMBERSHIP",
            libc::IP_ADD_SOURCE_MEMBERSHIP as i64,
        ),
        ("IP_BLOCK_SOURCE", libc::IP_BLOCK_SOURCE as i64),
        ("IP_DROP_MEMBERSHIP", libc::IP_DROP_MEMBERSHIP as i64),
        (
            "IP_DROP_SOURCE_MEMBERSHIP",
            libc::IP_DROP_SOURCE_MEMBERSHIP as i64,
        ),
        ("IP_HDRINCL", libc::IP_HDRINCL as i64),
        ("IP_MULTICAST_IF", libc::IP_MULTICAST_IF as i64),
        ("IP_MULTICAST_LOOP", libc::IP_MULTICAST_LOOP as i64),
        ("IP_MULTICAST_TTL", libc::IP_MULTICAST_TTL as i64),
        ("IP_PKTINFO", libc::IP_PKTINFO as i64),
        ("IP_RECVTOS", libc::IP_RECVTOS as i64),
        ("IP_RECVTTL", libc::IP_RECVTTL as i64),
        ("IP_TOS", libc::IP_TOS as i64),
        ("IP_TTL", libc::IP_TTL as i64),
        ("IP_UNBLOCK_SOURCE", libc::IP_UNBLOCK_SOURCE as i64),
        ("MSG_CTRUNC", libc::MSG_CTRUNC as i64),
        ("MSG_DONTROUTE", libc::MSG_DONTROUTE as i64),
        ("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64),
        ("MSG_EOR", libc::MSG_EOR as i64),
        ("MSG_NOSIGNAL", libc::MSG_NOSIGNAL as i64),
        ("MSG_OOB", libc::MSG_OOB as i64),
        ("MSG_PEEK", libc::MSG_PEEK as i64),
        ("MSG_TRUNC", libc::MSG_TRUNC as i64),
        ("MSG_WAITALL", libc::MSG_WAITALL as i64),
        ("NI_DGRAM", libc::NI_DGRAM as i64),
        ("NI_MAXHOST", libc::NI_MAXHOST as i64),
        ("NI_NAMEREQD", libc::NI_NAMEREQD as i64),
        ("NI_NOFQDN", libc::NI_NOFQDN as i64),
        ("NI_NUMERICHOST", libc::NI_NUMERICHOST as i64),
        ("NI_NUMERICSERV", libc::NI_NUMERICSERV as i64),
        ("PF_APPLETALK", libc::PF_APPLETALK as i64),
        ("PF_DECnet", libc::PF_DECnet as i64),
        ("PF_INET", libc::PF_INET as i64),
        ("PF_INET6", libc::PF_INET6 as i64),
        ("PF_IPX", libc::PF_IPX as i64),
        ("PF_ISDN", libc::PF_ISDN as i64),
        ("PF_KEY", libc::PF_KEY as i64),
        ("PF_LOCAL", libc::PF_LOCAL as i64),
        ("PF_ROUTE", libc::PF_ROUTE as i64),
        ("PF_SNA", libc::PF_SNA as i64),
        ("PF_UNIX", libc::PF_UNIX as i64),
        ("PF_UNSPEC", libc::PF_UNSPEC as i64),
        ("PF_VSOCK", libc::PF_VSOCK as i64),
        ("SCM_RIGHTS", libc::SCM_RIGHTS as i64),
        ("SCM_TIMESTAMP", libc::SCM_TIMESTAMP as i64),
        ("SHUT_RD", libc::SHUT_RD as i64),
        ("SHUT_RDWR", libc::SHUT_RDWR as i64),
        ("SHUT_WR", libc::SHUT_WR as i64),
        ("SOCK_DGRAM", libc::SOCK_DGRAM as i64),
        ("SOCK_RAW", libc::SOCK_RAW as i64),
        ("SOCK_RDM", libc::SOCK_RDM as i64),
        ("SOCK_SEQPACKET", libc::SOCK_SEQPACKET as i64),
        ("SOCK_STREAM", libc::SOCK_STREAM as i64),
        ("SOL_SOCKET", libc::SOL_SOCKET as i64),
        ("SOMAXCONN", libc::SOMAXCONN as i64),
        ("SO_ACCEPTCONN", libc::SO_ACCEPTCONN as i64),
        ("SO_BROADCAST", libc::SO_BROADCAST as i64),
        ("SO_DEBUG", libc::SO_DEBUG as i64),
        ("SO_DONTROUTE", libc::SO_DONTROUTE as i64),
        ("SO_ERROR", libc::SO_ERROR as i64),
        ("SO_KEEPALIVE", libc::SO_KEEPALIVE as i64),
        ("SO_LINGER", libc::SO_LINGER as i64),
        ("SO_OOBINLINE", libc::SO_OOBINLINE as i64),
        ("SO_RCVBUF", libc::SO_RCVBUF as i64),
        ("SO_RCVLOWAT", libc::SO_RCVLOWAT as i64),
        ("SO_RCVTIMEO", libc::SO_RCVTIMEO as i64),
        ("SO_REUSEADDR", libc::SO_REUSEADDR as i64),
        ("SO_REUSEPORT", libc::SO_REUSEPORT as i64),
        ("SO_SNDBUF", libc::SO_SNDBUF as i64),
        ("SO_SNDLOWAT", libc::SO_SNDLOWAT as i64),
        ("SO_SNDTIMEO", libc::SO_SNDTIMEO as i64),
        ("SO_TIMESTAMP", libc::SO_TIMESTAMP as i64),
        ("SO_TYPE", libc::SO_TYPE as i64),
        ("TCP_FASTOPEN", libc::TCP_FASTOPEN as i64),
        ("TCP_KEEPCNT", libc::TCP_KEEPCNT as i64),
        ("TCP_KEEPINTVL", libc::TCP_KEEPINTVL as i64),
        ("TCP_MAXSEG", libc::TCP_MAXSEG as i64),
        ("TCP_NODELAY", libc::TCP_NODELAY as i64),
    ];
    #[cfg(target_os = "linux")]
    #[allow(clippy::unnecessary_cast)]
    out.extend_from_slice(&[
        ("AF_ALG", libc::AF_ALG as i64),
        ("AF_AX25", libc::AF_AX25 as i64),
        ("AF_BLUETOOTH", libc::AF_BLUETOOTH as i64),
        ("AF_CAN", libc::AF_CAN as i64),
        ("AF_IB", libc::AF_IB as i64),
        ("AF_KEY", libc::AF_KEY as i64),
        ("AF_LLC", libc::AF_LLC as i64),
        ("AF_MPLS", libc::AF_MPLS as i64),
        ("AF_NETLINK", libc::AF_NETLINK as i64),
        ("AF_PACKET", libc::AF_PACKET as i64),
        ("AF_PPPOX", libc::AF_PPPOX as i64),
        ("AF_RDS", libc::AF_RDS as i64),
        ("AF_TIPC", libc::AF_TIPC as i64),
        ("IFF_AUTOMEDIA", libc::IFF_AUTOMEDIA as i64),
        ("IFF_DYNAMIC", libc::IFF_DYNAMIC as i64),
        ("IFF_MASTER", libc::IFF_MASTER as i64),
        ("IFF_PORTSEL", libc::IFF_PORTSEL as i64),
        ("IFF_SLAVE", libc::IFF_SLAVE as i64),
        ("IPV6_DSTOPTS", libc::IPV6_DSTOPTS as i64),
        ("IPV6_HOPOPTS", libc::IPV6_HOPOPTS as i64),
        ("IPV6_MTU_DISCOVER", libc::IPV6_MTU_DISCOVER as i64),
        ("IPV6_NEXTHOP", libc::IPV6_NEXTHOP as i64),
        ("IPV6_RECVDSTOPTS", libc::IPV6_RECVDSTOPTS as i64),
        ("IPV6_RECVERR", libc::IPV6_RECVERR as i64),
        ("IPV6_RECVHOPOPTS", libc::IPV6_RECVHOPOPTS as i64),
        ("IPV6_RECVRTHDR", libc::IPV6_RECVRTHDR as i64),
        ("IPV6_RTHDR", libc::IPV6_RTHDR as i64),
        ("IPV6_RTHDRDSTOPTS", libc::IPV6_RTHDRDSTOPTS as i64),
        (
            "IP_DEFAULT_MULTICAST_LOOP",
            libc::IP_DEFAULT_MULTICAST_LOOP as i64,
        ),
        (
            "IP_DEFAULT_MULTICAST_TTL",
            libc::IP_DEFAULT_MULTICAST_TTL as i64,
        ),
        ("IP_FREEBIND", libc::IP_FREEBIND as i64),
        ("IP_IPSEC_POLICY", libc::IP_IPSEC_POLICY as i64),
        ("IP_MINTTL", libc::IP_MINTTL as i64),
        ("IP_MSFILTER", libc::IP_MSFILTER as i64),
        ("IP_MTU", libc::IP_MTU as i64),
        ("IP_MTU_DISCOVER", libc::IP_MTU_DISCOVER as i64),
        ("IP_OPTIONS", libc::IP_OPTIONS as i64),
        ("IP_PASSSEC", libc::IP_PASSSEC as i64),
        ("IP_PKTOPTIONS", libc::IP_PKTOPTIONS as i64),
        ("IP_PMTUDISC_DO", libc::IP_PMTUDISC_DO as i64),
        ("IP_PMTUDISC_DONT", libc::IP_PMTUDISC_DONT as i64),
        ("IP_PMTUDISC_WANT", libc::IP_PMTUDISC_WANT as i64),
        ("IP_RECVERR", libc::IP_RECVERR as i64),
        ("IP_RECVOPTS", libc::IP_RECVOPTS as i64),
        ("IP_RETOPTS", libc::IP_RETOPTS as i64),
        ("IP_ROUTER_ALERT", libc::IP_ROUTER_ALERT as i64),
        ("IP_TRANSPARENT", libc::IP_TRANSPARENT as i64),
        ("IP_XFRM_POLICY", libc::IP_XFRM_POLICY as i64),
        ("MCAST_BLOCK_SOURCE", libc::MCAST_BLOCK_SOURCE as i64),
        ("MCAST_EXCLUDE", libc::MCAST_EXCLUDE as i64),
        ("MCAST_INCLUDE", libc::MCAST_INCLUDE as i64),
        ("MCAST_JOIN_GROUP", libc::MCAST_JOIN_GROUP as i64),
        (
            "MCAST_JOIN_SOURCE_GROUP",
            libc::MCAST_JOIN_SOURCE_GROUP as i64,
        ),
        ("MCAST_LEAVE_GROUP", libc::MCAST_LEAVE_GROUP as i64),
        (
            "MCAST_LEAVE_SOURCE_GROUP",
            libc::MCAST_LEAVE_SOURCE_GROUP as i64,
        ),
        ("MCAST_MSFILTER", libc::MCAST_MSFILTER as i64),
        ("MCAST_UNBLOCK_SOURCE", libc::MCAST_UNBLOCK_SOURCE as i64),
        ("MSG_CONFIRM", libc::MSG_CONFIRM as i64),
        ("MSG_ERRQUEUE", libc::MSG_ERRQUEUE as i64),
        ("MSG_FASTOPEN", libc::MSG_FASTOPEN as i64),
        ("MSG_FIN", libc::MSG_FIN as i64),
        ("MSG_MORE", libc::MSG_MORE as i64),
        ("MSG_RST", libc::MSG_RST as i64),
        ("MSG_SYN", libc::MSG_SYN as i64),
        ("PF_ALG", libc::PF_ALG as i64),
        ("PF_AX25", libc::PF_AX25 as i64),
        ("PF_BLUETOOTH", libc::PF_BLUETOOTH as i64),
        ("PF_CAN", libc::PF_CAN as i64),
        ("PF_IB", libc::PF_IB as i64),
        ("PF_LLC", libc::PF_LLC as i64),
        ("PF_MPLS", libc::PF_MPLS as i64),
        ("PF_NETLINK", libc::PF_NETLINK as i64),
        ("PF_PACKET", libc::PF_PACKET as i64),
        ("PF_PPPOX", libc::PF_PPPOX as i64),
        ("PF_RDS", libc::PF_RDS as i64),
        ("PF_TIPC", libc::PF_TIPC as i64),
        ("SCM_CREDENTIALS", libc::SCM_CREDENTIALS as i64),
        ("SCM_TIMESTAMPING", libc::SCM_TIMESTAMPING as i64),
        ("SCM_TIMESTAMPNS", libc::SCM_TIMESTAMPNS as i64),
        ("SCM_WIFI_STATUS", libc::SCM_WIFI_STATUS as i64),
        ("SOCK_CLOEXEC", libc::SOCK_CLOEXEC as i64),
        ("SOCK_NONBLOCK", libc::SOCK_NONBLOCK as i64),
        ("SOCK_PACKET", libc::SOCK_PACKET as i64),
        ("SOL_IP", libc::SOL_IP as i64),
        ("SOL_TCP", libc::SOL_TCP as i64),
        ("SOL_UDP", libc::SOL_UDP as i64),
        ("SO_ATTACH_FILTER", libc::SO_ATTACH_FILTER as i64),
        ("SO_BINDTODEVICE", libc::SO_BINDTODEVICE as i64),
        ("SO_BPF_EXTENSIONS", libc::SO_BPF_EXTENSIONS as i64),
        ("SO_BUSY_POLL", libc::SO_BUSY_POLL as i64),
        ("SO_DETACH_FILTER", libc::SO_DETACH_FILTER as i64),
        ("SO_DOMAIN", libc::SO_DOMAIN as i64),
        ("SO_GET_FILTER", libc::SO_GET_FILTER as i64),
        ("SO_LOCK_FILTER", libc::SO_LOCK_FILTER as i64),
        ("SO_MARK", libc::SO_MARK as i64),
        ("SO_MAX_PACING_RATE", libc::SO_MAX_PACING_RATE as i64),
        ("SO_NOFCS", libc::SO_NOFCS as i64),
        ("SO_NO_CHECK", libc::SO_NO_CHECK as i64),
        ("SO_PASSCRED", libc::SO_PASSCRED as i64),
        ("SO_PASSSEC", libc::SO_PASSSEC as i64),
        ("SO_PEEK_OFF", libc::SO_PEEK_OFF as i64),
        ("SO_PEERCRED", libc::SO_PEERCRED as i64),
        ("SO_PEERNAME", libc::SO_PEERNAME as i64),
        ("SO_PEERSEC", libc::SO_PEERSEC as i64),
        ("SO_PRIORITY", libc::SO_PRIORITY as i64),
        ("SO_PROTOCOL", libc::SO_PROTOCOL as i64),
        ("SO_RCVBUFFORCE", libc::SO_RCVBUFFORCE as i64),
        ("SO_RXQ_OVFL", libc::SO_RXQ_OVFL as i64),
        (
            "SO_SECURITY_AUTHENTICATION",
            libc::SO_SECURITY_AUTHENTICATION as i64,
        ),
        (
            "SO_SECURITY_ENCRYPTION_NETWORK",
            libc::SO_SECURITY_ENCRYPTION_NETWORK as i64,
        ),
        (
            "SO_SECURITY_ENCRYPTION_TRANSPORT",
            libc::SO_SECURITY_ENCRYPTION_TRANSPORT as i64,
        ),
        ("SO_SELECT_ERR_QUEUE", libc::SO_SELECT_ERR_QUEUE as i64),
        ("SO_SNDBUFFORCE", libc::SO_SNDBUFFORCE as i64),
        ("SO_TIMESTAMPING", libc::SO_TIMESTAMPING as i64),
        ("SO_TIMESTAMPNS", libc::SO_TIMESTAMPNS as i64),
        ("SO_WIFI_STATUS", libc::SO_WIFI_STATUS as i64),
        ("TCP_CONGESTION", libc::TCP_CONGESTION as i64),
        (
            "TCP_COOKIE_TRANSACTIONS",
            libc::TCP_COOKIE_TRANSACTIONS as i64,
        ),
        ("TCP_CORK", libc::TCP_CORK as i64),
        ("TCP_DEFER_ACCEPT", libc::TCP_DEFER_ACCEPT as i64),
        ("TCP_INFO", libc::TCP_INFO as i64),
        ("TCP_KEEPIDLE", libc::TCP_KEEPIDLE as i64),
        ("TCP_LINGER2", libc::TCP_LINGER2 as i64),
        ("TCP_MD5SIG", libc::TCP_MD5SIG as i64),
        ("TCP_QUEUE_SEQ", libc::TCP_QUEUE_SEQ as i64),
        ("TCP_QUICKACK", libc::TCP_QUICKACK as i64),
        ("TCP_REPAIR", libc::TCP_REPAIR as i64),
        ("TCP_REPAIR_OPTIONS", libc::TCP_REPAIR_OPTIONS as i64),
        ("TCP_REPAIR_QUEUE", libc::TCP_REPAIR_QUEUE as i64),
        ("TCP_SYNCNT", libc::TCP_SYNCNT as i64),
        ("TCP_THIN_DUPACK", libc::TCP_THIN_DUPACK as i64),
        (
            "TCP_THIN_LINEAR_TIMEOUTS",
            libc::TCP_THIN_LINEAR_TIMEOUTS as i64,
        ),
        ("TCP_TIMESTAMP", libc::TCP_TIMESTAMP as i64),
        ("TCP_USER_TIMEOUT", libc::TCP_USER_TIMEOUT as i64),
        ("TCP_WINDOW_CLAMP", libc::TCP_WINDOW_CLAMP as i64),
        ("UDP_CORK", libc::UDP_CORK as i64),
        // Not in the `libc` crate: glibc's values.
        ("EAI_ADDRFAMILY", -9),
        ("IP_RECVRETOPTS", libc::IP_RETOPTS as i64),
        ("IP_MAX_MEMBERSHIPS", 20),
        ("MSG_PROXY", 0x10),
        ("IPV6_JOIN_GROUP", libc::IPV6_ADD_MEMBERSHIP as i64),
        ("IPV6_LEAVE_GROUP", libc::IPV6_DROP_MEMBERSHIP as i64),
        ("AF_MAX", 46),
        ("PF_MAX", 46),
    ]);
    // The same everywhere, and not in the `libc` crate.
    out.extend_from_slice(&[
        ("INADDR_UNSPEC_GROUP", 0xe000_0000),
        ("INADDR_ALLHOSTS_GROUP", 0xe000_0001),
        ("INADDR_MAX_LOCAL_GROUP", 0xe000_00ff),
        ("INET_ADDRSTRLEN", 16),
        ("INET6_ADDRSTRLEN", 46),
        ("IPPORT_RESERVED", 1024),
        ("IPPORT_USERRESERVED", 5000),
        ("IPV6_RTHDR_TYPE_0", 0),
        ("NI_MAXSERV", NI_MAXSERV as i64),
    ]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sockaddr_in_round_trips() {
        let bytes = sockaddr_in(&[127, 0, 0, 1], 80, 0, 0).expect("four bytes");
        assert_eq!(family(&bytes), Some(libc::AF_INET));
        let Some(Parsed::Inet { port, addr }) = parse(&bytes) else {
            panic!("an AF_INET address")
        };
        assert_eq!((port, addr), (80, [127, 0, 0, 1]));
    }

    // Miri cannot call into libc.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn text_addresses_are_inet_ntop_and_pton() {
        let addr = pton(libc::AF_INET6, b"::1").expect("an address");
        assert_eq!(ntop(&addr).as_deref(), Some("::1"));
        assert_eq!(pton(libc::AF_INET, b"1.2.3"), None);
    }
}
