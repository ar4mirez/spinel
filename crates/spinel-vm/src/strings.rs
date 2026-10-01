//! A `String`'s representation (#19): three slots, `[buffer, bytesize,
//! encoding]`.
//!
//! `buffer` is a separate, classless `Payload::Bytes` object whose length is
//! the capacity; `bytesize` is how many of its bytes are live; `encoding` is
//! an index into the table `core/encoding.rb` builds. The indirection is
//! `Array`'s, for `Array`'s reason: a heap cell cannot grow, so a String whose
//! bytes lived in its own cell could only grow by becoming a different object,
//! and `s << "x"` must not change `s`'s identity.
//!
//! The VM knows an encoding only by index, and character boundaries only for
//! the three launch encodings. Every other encoding is a name `core/` can
//! answer about, and a character operation on it is refused by the caller
//! rather than guessed at.

use crate::heap::{Handle, HandleScope, Payload};
use crate::value::Value;

/// Slot of the byte buffer inside a `String`.
const BUFFER: usize = 0;
/// Slot of the live byte count.
const BYTESIZE: usize = 1;
/// Slot of the encoding index.
const ENCODING: usize = 2;
/// Slots in the `String` object itself.
pub const SLOTS: u32 = 3;
/// The smallest buffer a growing string allocates.
const MIN_CAPACITY: usize = 16;

/// `ASCII-8BIT`, also `BINARY`: every byte is a character.
pub const BINARY: u8 = 0;
/// `UTF-8`, the source encoding, and so every literal's.
pub const UTF_8: u8 = 1;
/// `US-ASCII`: what `Integer#to_s` and `Symbol#to_s` of an ASCII name answer.
pub const US_ASCII: u8 = 2;

/// Fill a freshly allocated `String` object (of `SLOTS` slots) with `bytes` in
/// `encoding`.
pub fn init<'h>(scope: &mut HandleScope<'h>, string: Handle<'h>, bytes: &[u8], encoding: u8) {
    let buffer = scope.alloc(None, Payload::Bytes, bytes.len() as u32);
    scope.bytes_mut(buffer).copy_from_slice(bytes);
    let buffer = scope.get(buffer);
    scope.set_slot(string, BUFFER, buffer);
    scope.set_slot(string, BYTESIZE, fixnum(bytes.len()));
    scope.set_slot(string, ENCODING, fixnum(usize::from(encoding)));
}

/// The live bytes, copied out.
pub fn bytes<'h>(scope: &mut HandleScope<'h>, string: Handle<'h>) -> Vec<u8> {
    let len = bytesize(scope, string);
    let buffer = scope.slot(string, BUFFER);
    let buffer = scope.root(buffer);
    scope.bytes(buffer)[..len].to_vec()
}

pub fn bytesize<'h>(scope: &mut HandleScope<'h>, string: Handle<'h>) -> usize {
    scope
        .slot(string, BYTESIZE)
        .as_fixnum()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

pub fn encoding<'h>(scope: &mut HandleScope<'h>, string: Handle<'h>) -> u8 {
    scope
        .slot(string, ENCODING)
        .as_fixnum()
        .and_then(|n| u8::try_from(n).ok())
        .unwrap_or(UTF_8)
}

pub fn set_encoding<'h>(scope: &mut HandleScope<'h>, string: Handle<'h>, encoding: u8) {
    scope.set_slot(string, ENCODING, fixnum(usize::from(encoding)));
}

/// Replace bytes `start..start + len` with `with`: every in-place change —
/// `<<`, `replace`, `[]=`, `insert`, `slice!` — is a splice. The buffer is
/// reused when it has room and replaced, doubling, when it does not; the
/// `String` object itself never moves.
pub fn splice<'h>(
    scope: &mut HandleScope<'h>,
    string: Handle<'h>,
    start: usize,
    len: usize,
    with: &[u8],
) {
    let old = bytes(scope, string);
    let start = start.min(old.len());
    let end = (start + len).min(old.len());
    let new_len = old.len() - (end - start) + with.len();
    let buffer = scope.slot(string, BUFFER);
    let buffer = scope.root(buffer);
    let capacity = scope.len(buffer) as usize;
    let mut joined = Vec::with_capacity(new_len);
    joined.extend_from_slice(&old[..start]);
    joined.extend_from_slice(with);
    joined.extend_from_slice(&old[end..]);
    if new_len <= capacity {
        scope.bytes_mut(buffer)[..new_len].copy_from_slice(&joined);
    } else {
        let mut grown = capacity.max(MIN_CAPACITY);
        while grown < new_len {
            grown *= 2;
        }
        let fresh = scope.alloc(None, Payload::Bytes, grown as u32);
        scope.bytes_mut(fresh)[..new_len].copy_from_slice(&joined);
        let fresh = scope.get(fresh);
        scope.set_slot(string, BUFFER, fresh);
    }
    scope.set_slot(string, BYTESIZE, fixnum(new_len));
}

/// A deep copy's buffer: `dup` must not share bytes, or `s.dup << "x"` would
/// change `s`. Called on the copy after its slots were copied from the source.
pub fn unshare<'h>(scope: &mut HandleScope<'h>, copy: Handle<'h>) {
    let live = bytes(scope, copy);
    let encoding = encoding(scope, copy);
    init(scope, copy, &live, encoding);
}

/// The byte length of the character starting at `at`, or `None` when this
/// VM does not know `encoding`'s character boundaries.
///
/// An invalid UTF-8 sequence is one character per byte, as CRuby counts it:
/// `"\xff\xfe".length` is 2.
#[must_use]
pub fn char_len(encoding: u8, bytes: &[u8], at: usize) -> Option<usize> {
    match encoding {
        BINARY | US_ASCII => Some(1),
        UTF_8 => Some(utf8_char_len(&bytes[at..])),
        _ => None,
    }
}

/// The length of one UTF-8 character at the start of `bytes`: the whole
/// sequence when it is valid, one byte when it is not.
fn utf8_char_len(bytes: &[u8]) -> usize {
    let first = bytes[0];
    let (want, min) = match first {
        0x00..=0x7f => return 1,
        0xc2..=0xdf => (2, 0x80),
        0xe0..=0xef => (3, 0x800),
        0xf0..=0xf4 => (4, 0x1_0000),
        _ => return 1,
    };
    if bytes.len() < want || !bytes[1..want].iter().all(|&b| b & 0xc0 == 0x80) {
        return 1;
    }
    match std::str::from_utf8(&bytes[..want]) {
        Ok(text) => {
            let point = u32::from(text.chars().next().unwrap_or('\0'));
            if point >= min { want } else { 1 }
        }
        Err(_) => 1,
    }
}

/// The byte offset of every character start, and the total length as a last
/// entry; `None` for an encoding this VM cannot walk.
#[must_use]
pub fn char_offsets(encoding: u8, bytes: &[u8]) -> Option<Vec<usize>> {
    let mut offsets = Vec::with_capacity(bytes.len() + 1);
    let mut at = 0;
    while at < bytes.len() {
        offsets.push(at);
        at += char_len(encoding, bytes, at)?;
    }
    offsets.push(bytes.len());
    Some(offsets)
}

/// Whether every byte sequence is a character of `encoding`; `None` when this
/// VM cannot tell.
#[must_use]
pub fn valid(encoding: u8, bytes: &[u8]) -> Option<bool> {
    match encoding {
        BINARY => Some(true),
        US_ASCII => Some(bytes.is_ascii()),
        UTF_8 => Some(std::str::from_utf8(bytes).is_ok()),
        _ => None,
    }
}

/// Whether `encoding`'s ASCII range is ASCII, from CRuby's table.
#[must_use]
pub fn ascii_compatible(encoding: u8) -> bool {
    crate::encoding_table::ENCODINGS
        .get(usize::from(encoding))
        .is_some_and(|&(_, compatible, _)| compatible)
}

/// CRuby's `rb_str_comparable`: whether two strings' bytes may be compared
/// at all. The same encoding, or one side pure ASCII and the other side's
/// encoding ASCII-compatible. `"é" == "é".b` is false; `"a" == "a".b` is
/// true.
#[must_use]
pub fn comparable(left: (u8, &[u8]), right: (u8, &[u8])) -> bool {
    let ((left_enc, left_bytes), (right_enc, right_bytes)) = (left, right);
    if left_enc == right_enc {
        return true;
    }
    let left_ascii = ascii_compatible(left_enc) && left_bytes.is_ascii();
    let right_ascii = ascii_compatible(right_enc) && right_bytes.is_ascii();
    (left_ascii && (right_ascii || ascii_compatible(right_enc)))
        || (right_ascii && ascii_compatible(left_enc))
}

fn fixnum(n: usize) -> Value {
    Value::fixnum(n as i64).expect("a string length fits a fixnum")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_utf8_is_one_character_per_byte() {
        assert_eq!(char_offsets(UTF_8, b"\xff\xfe"), Some(vec![0, 1, 2]));
        assert_eq!(
            char_offsets(UTF_8, "aé€😀".as_bytes()),
            Some(vec![0, 1, 3, 6, 10])
        );
        // An overlong encoding of "/" is two invalid bytes, not a character.
        assert_eq!(char_offsets(UTF_8, b"\xc0\xaf"), Some(vec![0, 1, 2]));
        // A truncated sequence: the lead byte alone, then the rest.
        assert_eq!(char_offsets(UTF_8, b"\xe2\x82"), Some(vec![0, 1, 2]));
    }

    #[test]
    fn binary_and_ascii_are_bytes() {
        assert_eq!(char_offsets(BINARY, "é".as_bytes()), Some(vec![0, 1, 2]));
        assert_eq!(valid(US_ASCII, "é".as_bytes()), Some(false));
        assert_eq!(valid(BINARY, b"\xff"), Some(true));
    }

    #[test]
    fn an_unknown_encoding_is_unknown() {
        assert_eq!(char_offsets(9, b"abc"), None);
        assert_eq!(valid(9, b"abc"), None);
    }
}
