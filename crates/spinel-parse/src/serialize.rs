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

/// The options blob Prism reads (`pm_options_read`), or a null pointer for
/// none. The Ruby half packs it, as the gem's `dump_options` does.
fn options_ptr(options: &[u8]) -> *const c_char {
    if options.is_empty() {
        std::ptr::null()
    } else {
        options.as_ptr().cast()
    }
}

/// Serialize `source` as `dump` asks, under the packed `options`.
#[must_use]
pub fn dump(dump: Dump, source: &[u8], options: &[u8]) -> Vec<u8> {
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
    // source and options pointers are valid for their lengths for the call,
    // and Prism reads the options only up to the lengths they carry.
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
        run(
            &raw mut buffer,
            source.as_ptr(),
            source.len(),
            options_ptr(options),
        );
        let length = pm_buffer_length(&raw const buffer);
        let value = pm_buffer_value(&raw const buffer);
        let bytes = if length == 0 || value.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(value.cast::<u8>(), length).to_vec()
        };
        pm_buffer_free(&raw mut buffer);
        bytes
    }
}

/// Whether `source` parses without errors under `options`.
#[must_use]
pub fn success(source: &[u8], options: &[u8]) -> bool {
    // SAFETY: both pointers are valid for their lengths for the call.
    unsafe { pm_parse_success_p(source.as_ptr(), source.len(), options_ptr(options)) }
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
        let bytes = dump(Dump::Parse, b"1 + 2", b"");
        assert_eq!(&bytes[..5], b"PRISM");
        assert_eq!(&bytes[5..8], &[1, 9, 0]);
        assert_eq!(version(), "1.9.0");
    }

    #[test]
    fn reports_success_and_queries() {
        assert!(success(b"def m; end", b""));
        assert!(!success(b"def m(", b""));
        assert_eq!(query(Query::Local, b"foo", c"UTF-8"), 1);
        assert_eq!(query(Query::Constant, b"foo", c"UTF-8"), 0);
        assert_eq!(query(Query::MethodName, b"foo?", c"UTF-8"), 1);
        assert_eq!(query(Query::Local, b"foo", c"NOPE"), -1);
    }
}
