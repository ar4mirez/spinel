# PRD 0087 — `socket`

Issue: [#61](https://github.com/ar4mirez/spinel/issues/61) · Phase 3 ·
`area:gems`

## Objective

`require "socket"` behaves as CRuby 4.0.6's: `Socket`, `Addrinfo`,
`BasicSocket`, `IPSocket`, `TCPSocket`, `TCPServer`, `UDPSocket`,
`UNIXSocket`, `UNIXServer`, `Socket::Option`, `Socket::AncillaryData` and
`Socket::Ifaddr`. Before this, `library/socket` was blocked entirely, and so
was everything that talks to a network: `net/http`, `net/ftp`, Bundler's
fetcher, any server.

## Design

**System calls in Rust, bytes in and out.** `crates/spinel-vm/src/socket.rs`
wraps `socket`, `socketpair`, `bind`, `connect`, `listen`, `accept`,
`getsockname`/`getpeername`, `getsockopt`/`setsockopt`, `send`/`recv`,
`sendmsg`/`recvmsg` with control messages, `shutdown`, `getaddrinfo`,
`getnameinfo`, `getservbyname`/`port`, `getifaddrs`, the interface-name
calls, `inet_ntop`/`pton` and `poll`. An address crosses as the bytes of the
platform's `struct sockaddr_*`, which is also what CRuby's
`Addrinfo#to_sockaddr` answers, so `socket.rs` is the only place that knows
the layouts (`sin_len` on the BSDs, a one-byte family there). `interp.rs`
turns each into a `__sock_*__` primitive answering an errno on failure, as
the descriptor calls do. `Socket::Constants` is CRuby's list, each value the
platform's from the `libc` crate, with the few the crate lacks spelled out.

**`lib/socket.rb`, in Ruby,** after `ext/socket`'s C files: the name and
number arguments (`:INET`, `"SOCK_STREAM"`, a level's own option prefix),
the error messages, `Addrinfo` with its `inspect` and `marshal_dump`, and
CRuby's `numeric_getaddrinfo`, which answers a numeric host and port without
the resolver — a stream, a datagram and a raw socket, as the hints allow —
and is why `Addrinfo.new(["AF_INET", 80, "x", "1.2.3.4"], nil, :RAW, 51)`
works where `getaddrinfo(3)` would refuse it. The methods of upstream's own
`lib/socket.rb` (`Addrinfo#connect`, `Socket.tcp_server_sockets`,
`udp_server_loop`, `unix_server_socket`, `connect_address`) are its code,
kept close to it.

**Nonblocking, as CRuby makes them.** A socket is `O_NONBLOCK` from the
start, as in CRuby 3; the blocking calls wait in `poll(2)` and retry, for as
long as `IO#timeout` allows, then raise `IO::TimeoutError`.
`read_nonblock`/`write_nonblock` use `MSG_DONTWAIT` and leave the
descriptor's mode alone, as upstream's do. For this `core/io.rb` gains
`IO.select`, `IO#wait`, `#wait_readable`, `#wait_writable` and
`#wait_priority`, `IO#timeout`, and `IO::TimeoutError`; reads and writes wait
on a nonblocking descriptor instead of failing, and the write primitive
answers a partial count so the rest can follow.

`ponytail:` `Socket.tcp` and `TCPSocket.new` try the addresses one after
another, which is upstream's `tcp_without_fast_fallback`: the happy-eyeballs
path starts threads (#45). `getservbyname` is the non-reentrant call, safe
while a Ractor is one OS thread (#119). `BasicSocket#ioctl` waits on
`IO#ioctl` (#41).

## Results

`library/socket` from 0 (every file stopped at `require "socket"`) to
**1,695** of 1,814 examples passing here, 0 failing. The 119 still blocked:
96 start a `Thread` (#45), 16 need the vendored `ipaddr` (#47), 6 connect to
192.0.2.1 to time out where this machine answers "no route" at once —
CRuby fails them the same way here — and one is `ioctl`.

Corpus 23,356 to **25,104** passing, 0 failed. Compared example by example
with `main`, 886 blocked examples pass — `library/socket`, and `core/io`,
`core/file` and `library/io-wait` through `IO.select` and `IO#wait` — and
none that passed fails; the rest of the gain is socket examples that never
loaded before. Every pass agrees with ruby 4.0.6.
