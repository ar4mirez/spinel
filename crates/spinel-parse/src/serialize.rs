//! Prism's serialization API, for Prism's own Ruby half (#837).
//!
//! `require "prism"` loads the prism gem's Ruby files, which build their
//! node objects from the bytes `pm_serialize_parse` and its siblings write,
//! as the gem's FFI backend does. These are those calls, on the copy of
//! Prism Spinel parses with, so the Ruby half (the gem at the same release,
//! vendored in `stdlib/`) reads exactly what this C half writes. No Prism
//! node crosses this API: what comes out is bytes and booleans.
//!
//! The functions are declared here rather than bound by `ruby-prism-sys`,
//! whose bindings stop at the parser itself; they are in the same
//! `libprism.a` that crate builds and links.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char};

/// Prism's `pm_buffer_t`: storage it grows and owns until `pm_buffer_free`.
/// Read only through `pm_buffer_value` and `pm_buffer_length`; the fields
/// are here for its size and alignment.
#[repr(C)]
struct Buffer {
    length: usize,
    capacity: usize,
    value: *mut c_char,
}

unsafe extern "C" {
    fn pm_buffer_sizeof() -> usize;
    fn pm_buffer_init(buffer: *mut Buffer) -> bool;
    fn pm_buffer_free(buffer: *mut Buffer);
    fn pm_buffer_value(buffer: *const Buffer) -> *const c_char;
    fn pm_buffer_length(buffer: *const Buffer) -> usize;
    fn pm_serialize_parse(buffer: *mut Buffer, source: *const u8, size: usize, data: *const c_char);
    fn pm_serialize_lex(buffer: *mut Buffer, source: *const u8, size: usize, data: *const c_char);
    fn pm_serialize_parse_lex(
        buffer: *mut Buffer,
        source: *const u8,
        size: usize,
        data: *const c_char,
    );
    fn pm_serialize_parse_comments(
        buffer: *mut Buffer,
        source: *const u8,
        size: usize,
        data: *const c_char,
    );
    fn pm_parse_success_p(source: *const u8, size: usize, data: *const c_char) -> bool;
    fn pm_string_query_local(source: *const u8, length: usize, encoding: *const c_char) -> i32;
    fn pm_string_query_constant(source: *const u8, length: usize, encoding: *const c_char) -> i32;
    fn pm_string_query_method_name(
        source: *const u8,
        length: usize,
        encoding: *const c_char,
    ) -> i32;
    fn pm_version() -> *const c_char;
}

/// What to serialize: the four calls the Ruby half's loaders read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dump {
    /// `pm_serialize_parse`, read by `Prism::Serialize.load_parse`.
    Parse,
    /// `pm_serialize_lex`, read by `load_lex`.
    Lex,
    /// `pm_serialize_parse_lex`, read by `load_parse_lex`.
    ParseLex,
    /// `pm_serialize_parse_comments`, read by `load_parse_comments`.
    Comments,
}

/// Which of Prism's string queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// `pm_string_query_local`.
    Local,
    /// `pm_string_query_constant`.
    Constant,
    /// `pm_string_query_method_name`.
    MethodName,
}

/// An options blob `pm_options_read` would read past the end of, or that
/// has bytes left over after the layout it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedOptions;

/// The bytes `pm_options_read` takes one at a time after the encoding name
/// (prism 1.9.0, `src/options.c`): `frozen_string_literal`, `command_line`,
/// `version`, `encoding_locked`, `main_script`, `partial_script`, `freeze`.
const FLAG_BYTES: usize = 7;

/// The options blob Prism reads (`pm_options_read`), or a null pointer for
/// none. The Ruby half packs it, as the gem's `dump_options` does. Prism
/// checks none of the lengths the blob carries, so the layout it reads is
/// walked here against the blob's length first, and a blob that does not
/// fit it exactly is refused before Prism sees it.
fn options_ptr(options: &[u8]) -> Result<*const c_char, MalformedOptions> {
    struct Cursor<'a>(&'a [u8]);
    impl<'a> Cursor<'a> {
        fn skip(&mut self, n: usize) -> Result<&'a [u8], MalformedOptions> {
            let (head, tail) = self.0.split_at_checked(n).ok_or(MalformedOptions)?;
            self.0 = tail;
            Ok(head)
        }
        fn length(&mut self) -> Result<usize, MalformedOptions> {
            let bytes: [u8; 4] = self.skip(4)?.try_into().map_err(|_| MalformedOptions)?;
            usize::try_from(u32::from_ne_bytes(bytes)).map_err(|_| MalformedOptions)
        }
        fn string(&mut self) -> Result<(), MalformedOptions> {
            let n = self.length()?;
            self.skip(n).map(drop)
        }
    }

    if options.is_empty() {
        return Ok(std::ptr::null());
    }
    let mut blob = Cursor(options);
    blob.string()?;
    blob.skip(4)?;
    blob.string()?;
    blob.skip(FLAG_BYTES)?;
    for _ in 0..blob.length()? {
        let locals = blob.length()?;
        blob.skip(1)?;
        for _ in 0..locals {
            blob.string()?;
        }
    }
    if blob.0.is_empty() {
        Ok(options.as_ptr().cast())
    } else {
        Err(MalformedOptions)
    }
}

/// Serialize `source` as `dump` asks, under the packed `options`.
///
/// # Errors
///
/// [`MalformedOptions`] when `options` is not a blob Prism can read.
pub fn dump(dump: Dump, source: &[u8], options: &[u8]) -> Result<Vec<u8>, MalformedOptions> {
    let options = options_ptr(options)?;
    // Prism's own struct is three words; this one is too. Checked once
    // against the library rather than assumed.
    // SAFETY: `pm_buffer_sizeof` reads nothing and has no preconditions.
    assert_eq!(
        unsafe { pm_buffer_sizeof() },
        std::mem::size_of::<Buffer>(),
        "pm_buffer_t is not three words"
    );
    let mut buffer = Buffer {
        length: 0,
        capacity: 0,
        value: std::ptr::null_mut(),
    };
    // SAFETY: `buffer` is a valid, writable `pm_buffer_t`; `pm_buffer_init`
    // allocates its storage, which `pm_buffer_free` below releases. The
    // source pointer is valid for its length for the call, and the options
    // are null or a blob whose every length `options_ptr` checked.
    unsafe {
        assert!(
            pm_buffer_init(&raw mut buffer),
            "Prism could not allocate a buffer"
        );
        let run = match dump {
            Dump::Parse => pm_serialize_parse,
            Dump::Lex => pm_serialize_lex,
            Dump::ParseLex => pm_serialize_parse_lex,
            Dump::Comments => pm_serialize_parse_comments,
        };
        run(&raw mut buffer, source.as_ptr(), source.len(), options);
        let length = pm_buffer_length(&raw const buffer);
        let value = pm_buffer_value(&raw const buffer);
        let bytes = if length == 0 || value.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(value.cast::<u8>(), length).to_vec()
        };
        pm_buffer_free(&raw mut buffer);
        Ok(bytes)
    }
}

/// Whether `source` parses without errors under `options`.
///
/// # Errors
///
/// [`MalformedOptions`] when `options` is not a blob Prism can read.
pub fn success(source: &[u8], options: &[u8]) -> Result<bool, MalformedOptions> {
    let options = options_ptr(options)?;
    // SAFETY: `source` is valid for its length for the call, and `options`
    // is null or a blob whose every length `options_ptr` checked.
    Ok(unsafe { pm_parse_success_p(source.as_ptr(), source.len(), options) })
}

/// Prism's answer for `string` in the encoding named `encoding`: 1 yes, 0
/// no, -1 an encoding Prism does not know.
#[must_use]
pub fn query(query: Query, string: &[u8], encoding: &CStr) -> i32 {
    // SAFETY: `string` is valid for its length and `encoding` is a
    // NUL-terminated name, for the call.
    unsafe {
        match query {
            Query::Local => pm_string_query_local(string.as_ptr(), string.len(), encoding.as_ptr()),
            Query::Constant => {
                pm_string_query_constant(string.as_ptr(), string.len(), encoding.as_ptr())
            }
            Query::MethodName => {
                pm_string_query_method_name(string.as_ptr(), string.len(), encoding.as_ptr())
            }
        }
    }
}

/// The Prism release Spinel parses with, `pm_version`.
#[must_use]
pub fn version() -> &'static str {
    // SAFETY: `pm_version` answers a pointer to a static NUL-terminated
    // string.
    unsafe { CStr::from_ptr(pm_version()) }
        .to_str()
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_a_parse_with_the_version_header() {
        let bytes = dump(Dump::Parse, b"1 + 2", b"").unwrap();
        assert_eq!(&bytes[..5], b"PRISM");
        assert_eq!(&bytes[5..8], &[1, 9, 0]);
        assert_eq!(version(), "1.9.0");
    }

    #[test]
    fn reports_success_and_queries() {
        assert_eq!(success(b"def m; end", b""), Ok(true));
        assert_eq!(success(b"def m(", b""), Ok(false));
        assert_eq!(query(Query::Local, b"foo", c"UTF-8"), 1);
        assert_eq!(query(Query::Constant, b"foo", c"UTF-8"), 0);
        assert_eq!(query(Query::MethodName, b"foo?", c"UTF-8"), 1);
        assert_eq!(query(Query::Local, b"foo", c"NOPE"), -1);
    }

    fn u32(n: u32) -> [u8; 4] {
        n.to_ne_bytes()
    }

    /// The layout `lib/prism/prism.rb` packs: filepath "a.rb", line 3,
    /// encoding "UTF-8", the seven flag bytes, and one scope with the
    /// forwarding byte and two locals, "x" and "yz".
    fn packed() -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend(u32(4));
        blob.extend(b"a.rb");
        blob.extend(3i32.to_ne_bytes());
        blob.extend(u32(5));
        blob.extend(b"UTF-8");
        blob.extend([0u8; FLAG_BYTES]);
        blob.extend(u32(1));
        blob.extend(u32(2));
        blob.push(0);
        blob.extend(u32(1));
        blob.extend(b"x");
        blob.extend(u32(2));
        blob.extend(b"yz");
        blob
    }

    #[test]
    fn accepts_the_options_the_ruby_half_packs() {
        let blob = packed();
        assert!(options_ptr(&blob).is_ok());
        assert_eq!(success(b"x + yz", &blob), Ok(true));
        assert!(dump(Dump::Parse, b"x + yz", &blob).is_ok());
    }

    #[test]
    fn refuses_options_prism_would_read_past() {
        let blob = packed();
        for cut in 1..blob.len() {
            assert_eq!(options_ptr(&blob[..cut]), Err(MalformedOptions), "{cut}");
        }
        assert_eq!(success(b"1", b"1"), Err(MalformedOptions));
        let mut huge = u32(0x7fff_ffff).to_vec();
        huge.extend([0u8; 16]);
        assert_eq!(dump(Dump::Parse, b"1", &huge), Err(MalformedOptions));
        assert_eq!(dump(Dump::Lex, b"1", b"x"), Err(MalformedOptions));
    }

    #[test]
    fn refuses_options_with_bytes_left_over() {
        let mut blob = packed();
        blob.push(0);
        assert_eq!(options_ptr(&blob), Err(MalformedOptions));
        assert_eq!(success(b"1", &blob), Err(MalformedOptions));
    }
}
