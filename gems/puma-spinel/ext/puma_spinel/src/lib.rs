//! The native half of the puma-spinel gem: what upstream Puma's C extension
//! (`ext/puma_http11`) does that Ruby cannot, as singleton methods of
//! `Puma::Spinel`. Everything the C decides on top of these — the Ruby
//! objects it builds, its checks, its exceptions and their messages — is
//! Ruby, in `lib/puma/puma_http11.rb`.
//!
//! - The HTTP request parser ([`parser`]): `http11_parser.c`'s state
//!   machine and `puma_http11.c`'s length checks, run over a request's
//!   bytes, answering the callbacks the C would have made.
//! - MiniSSL ([`minissl`]): `mini_ssl.c`'s OpenSSL calls, on the system's
//!   OpenSSL 3, an engine over a pair of memory BIOs as upstream's is.
//! - `Puma.enable_child_subreaper`'s `prctl`.
//!
//! A parser, an SSL context and an engine are boxes the Ruby side holds by
//! an Integer handle, the box's address, each starting with a tag of its
//! kind; `*_new` makes one, `*_free` (from its holder's finalizer) frees it.
//!
//! ponytail: a handle is an address, so a forged or freed one passed back
//! is undefined behavior, which the tag only catches when the address still
//! holds a box. Only `puma_http11.rb` passes them, from instance variables a
//! finalizer frees after. Upgrade: an object with native data and a free
//! function in `spinel-ext`, as CRuby's typed data.

mod minissl;
mod parser;

use std::ptr;

use spinel_ext::{Context, Error, Result, Value};

use parser::{Event, Parser};

spinel_ext::extension!(init);

/// What a parser box starts with.
const PARSER_TAG: u64 = 0x5350_494e_5055_4d50;

struct ParserBox {
    tag: u64,
    parser: Parser,
}

/// One method of `Puma::Spinel`: `$cx`, and the arguments as `$args`.
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
pub(crate) use methods;

fn init(cx: &mut Context<'_>) -> Result<()> {
    let puma = cx.define_module("Puma")?;
    let spinel = cx.define_module_under(puma, "Spinel")?;
    let table: &[(&str, i32, spinel_ext::Method)] = &[
        ("parser_new", 0, parser_new),
        ("parser_free", 1, parser_free),
        ("parser_init", 1, parser_init),
        ("parser_execute", 3, parser_execute),
        ("parser_nread", 1, parser_nread),
        ("parser_error?", 1, parser_has_error),
        ("parser_finished?", 1, parser_is_finished),
        ("child_subreaper", 0, child_subreaper),
    ];
    for &(name, arity, method) in table {
        cx.define_singleton_method(spinel, name, arity, method)?;
    }
    minissl::init(cx, spinel)
}

/// A new box, as its handle.
pub(crate) fn handle<'cx, T>(cx: &mut Context<'cx>, boxed: Box<T>) -> Value<'cx> {
    let address = Box::into_raw(boxed) as usize;
    cx.integer(i64::try_from(address).expect("a user-space address fits in i64"))
}

/// The box a handle names, checked by its tag.
pub(crate) fn boxed<'cx, 'a, T>(
    cx: &mut Context<'cx>,
    value: Value<'cx>,
    tag: u64,
    what: &str,
) -> Result<&'a mut T> {
    let address = cx.to_i64(value)?;
    let pointer = usize::try_from(address).unwrap_or(0) as *mut T;
    // SAFETY: the handle is the address of a live box (the module notes),
    // each of which starts with its `u64` tag.
    if pointer.is_null() || unsafe { *pointer.cast::<u64>() } != tag {
        return Err(Error::new("ArgumentError", format!("not a {what} handle")));
    }
    // SAFETY: as above, and the Ruby side uses a box from one call at a time.
    Ok(unsafe { &mut *pointer })
}

fn parser<'cx, 'a>(cx: &mut Context<'cx>, value: Value<'cx>) -> Result<&'a mut ParserBox> {
    boxed(cx, value, PARSER_TAG, "parser")
}

/// Records for the Ruby side: each a kind byte, a count of fields, and the
/// fields, each a little-endian 32-bit length and that many bytes.
#[derive(Default)]
pub(crate) struct Packer(Vec<u8>);

impl Packer {
    pub(crate) fn record(&mut self, kind: u8, fields: &[&[u8]]) {
        self.0.push(kind);
        self.0
            .push(u8::try_from(fields.len()).expect("a few fields"));
        for field in fields {
            let len = u32::try_from(field.len()).expect("a field under 4 GiB");
            self.0.extend_from_slice(&len.to_le_bytes());
            self.0.extend_from_slice(field);
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

methods! {
    /// A new parser, as `HttpParser_alloc` makes it: its handle.
    fn parser_new(cx, _args) {
        Ok(handle(cx, Box::new(ParserBox { tag: PARSER_TAG, parser: Parser::new() })))
    }

    /// Free a parser's box. Its holder's finalizer calls it; nothing uses
    /// the handle after.
    fn parser_free(cx, args) {
        let boxed = parser(cx, args[0])?;
        boxed.tag = 0;
        // SAFETY: a box `parser_new` made, freed once.
        drop(unsafe { Box::from_raw(ptr::from_mut(boxed)) });
        Ok(cx.nil())
    }

    /// `puma_parser_init`.
    fn parser_init(cx, args) {
        parser(cx, args[0])?.parser.init();
        Ok(cx.nil())
    }

    /// `puma_parser_execute(parser, data, from)` with `puma_http11.c`'s
    /// callbacks, as records: `M`, `U`, `F`, `P`, `Q` and `V` the request
    /// line's elements, `H` a header's name and value, `B` the body, `X` an
    /// offset in `data` and the upcased bytes written there, and `E` the
    /// name, limit and length of an element `VALIDATE_MAX_LENGTH` raised
    /// for, last. The Ruby side has checked `from` is inside `data`.
    fn parser_execute(cx, args) {
        let boxed = parser(cx, args[0])?;
        let mut data = cx.to_bytes(args[1])?;
        let from = usize::try_from(cx.to_i64(args[2])?)
            .ok()
            .filter(|&from| from < data.len())
            .ok_or_else(|| Error::new("ArgumentError", "start outside the data"))?;
        let mut packer = Packer::default();
        for event in boxed.parser.execute(&mut data, from) {
            match event {
                Event::Method(v) => packer.record(b'M', &[&v]),
                Event::Uri(v) => packer.record(b'U', &[&v]),
                Event::Fragment(v) => packer.record(b'F', &[&v]),
                Event::Path(v) => packer.record(b'P', &[&v]),
                Event::Query(v) => packer.record(b'Q', &[&v]),
                Event::Protocol(v) => packer.record(b'V', &[&v]),
                Event::Field(name, value) => packer.record(b'H', &[&name, &value]),
                Event::Body(v) => packer.record(b'B', &[&v]),
                Event::Upcase(at, bytes) => {
                    packer.record(b'X', &[at.to_string().as_bytes(), &bytes]);
                }
                Event::TooLong(name, max, len) => packer.record(
                    b'E',
                    &[name.as_bytes(), max.to_string().as_bytes(), len.to_string().as_bytes()],
                ),
            }
        }
        Ok(cx.string_from_bytes(packer.bytes()))
    }

    /// `puma_parser_nread`.
    fn parser_nread(cx, args) {
        let nread = parser(cx, args[0])?.parser.nread();
        Ok(cx.integer(i64::try_from(nread).expect("a count under 2**63")))
    }

    /// `puma_parser_has_error`.
    fn parser_has_error(cx, args) {
        let error = parser(cx, args[0])?.parser.has_error();
        Ok(cx.boolean(error))
    }

    /// `puma_parser_is_finished`.
    fn parser_is_finished(cx, args) {
        let finished = parser(cx, args[0])?.parser.is_finished();
        Ok(cx.boolean(finished))
    }

    /// `prctl(PR_SET_CHILD_SUBREAPER, 1)` where the C has it, Linux:
    /// `true`, or `-errno` when it fails. Elsewhere `false`, as the C
    /// answers without `HAVE_PRCTL`.
    fn child_subreaper(cx, _args) {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // SAFETY: sets a flag of this process; no pointers.
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) } < 0 {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO);
                return Ok(cx.integer(-i64::from(errno)));
            }
            Ok(cx.boolean(true))
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            Ok(cx.boolean(false))
        }
    }
}
