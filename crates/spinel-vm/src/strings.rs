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
        _ => crate::transcode::char_len(encoding, bytes, at),
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
        _ => crate::transcode::valid(encoding, bytes),
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
    // An empty string compares with anything, whatever the encodings.
    if left_enc == right_enc || left_bytes.is_empty() || right_bytes.is_empty() {
        return true;
    }
    let left_ascii = ascii_compatible(left_enc) && left_bytes.is_ascii();
    let right_ascii = ascii_compatible(right_enc) && right_bytes.is_ascii();
    (left_ascii && (right_ascii || ascii_compatible(right_enc)))
        || (right_ascii && ascii_compatible(left_enc))
}

/// CRuby's `rb_enc_compatible` for two strings: the encoding their
/// concatenation is in, or `None` when they cannot be combined.
///
/// The same encoding is that encoding. An empty side takes the other's —
/// except that an empty left keeps its own when the right is ASCII it can
/// hold. Otherwise both must be ASCII-compatible, and a pure-ASCII side
/// yields to the other.
#[must_use]
pub fn compatible(left: (u8, &[u8]), right: (u8, &[u8])) -> Option<u8> {
    let ((left_enc, left_bytes), (right_enc, right_bytes)) = (left, right);
    if left_enc == right_enc {
        return Some(left_enc);
    }
    if right_bytes.is_empty() {
        return Some(left_enc);
    }
    if left_bytes.is_empty() {
        // ASCII-only is a property of the string, not its bytes: `"x"` in
        // UTF-16LE is `x\0`, ASCII bytes, and not ASCII-only. Measured.
        let right_ascii_only = ascii_compatible(right_enc) && right_bytes.is_ascii();
        return Some(if ascii_compatible(left_enc) && right_ascii_only {
            left_enc
        } else {
            right_enc
        });
    }
    if !ascii_compatible(left_enc) || !ascii_compatible(right_enc) {
        return None;
    }
    if right_bytes.is_ascii() {
        return Some(left_enc);
    }
    if left_bytes.is_ascii() {
        return Some(right_enc);
    }
    None
}

/// The index of the encoding `name` names, ignoring ASCII case — how a magic
/// comment's value is read. `None` for a name CRuby does not have.
#[must_use]
pub fn index_named(name: &str) -> Option<u8> {
    crate::encoding_table::ENCODINGS
        .iter()
        .position(|&(names, _, _)| {
            names
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(name))
        })
        .and_then(|index| u8::try_from(index).ok())
}

/// What `enc_succ_char` and `enc_pred_char` report: a neighbour of the same
/// length, a wrap past the end of that length, or nothing usable.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Neighbor {
    Found,
    Wrapped,
    NotChar,
}

/// A character's class for `succ`: Onigmo's `[[:digit:]]` or `[[:alpha:]]`,
/// Unicode's in UTF-8 and ASCII's in a byte encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctype {
    Digit,
    Alpha,
}

fn in_ranges(ranges: &[(u32, u32)], code: u32) -> bool {
    ranges
        .binary_search_by(|&(first, last)| {
            if code < first {
                std::cmp::Ordering::Greater
            } else if code > last {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn ctype_of(encoding: u8, code: u32) -> Option<Ctype> {
    if encoding == UTF_8 {
        if in_ranges(crate::encoding_table::DIGIT, code) {
            return Some(Ctype::Digit);
        }
        return in_ranges(crate::encoding_table::ALPHA, code).then_some(Ctype::Alpha);
    }
    match u8::try_from(code).ok()? {
        b if b.is_ascii_digit() => Some(Ctype::Digit),
        b if b.is_ascii_alphabetic() => Some(Ctype::Alpha),
        _ => None,
    }
}

fn utf8_len(code: u32) -> usize {
    match code {
        0..=0x7f => 1,
        0x80..=0x7ff => 2,
        0x800..=0xffff => 3,
        _ => 4,
    }
}

/// The next character of the same byte length: a UTF-8 codepoint skipping
/// the surrogates, or a byte. Past the end of the length it wraps to the
/// lowest character of that length.
fn succ_char(encoding: u8, code: u32, len: usize) -> (Neighbor, u32) {
    if encoding != UTF_8 {
        return if code >= 0xff {
            (Neighbor::Wrapped, 0)
        } else {
            (Neighbor::Found, code + 1)
        };
    }
    let mut next = code + 1;
    if (0xd800..=0xdfff).contains(&next) {
        next = 0xe000;
    }
    if next > 0x10_ffff || utf8_len(next) != len {
        let lowest = [0, 0x80, 0x800, 0x1_0000][len - 1];
        return (Neighbor::Wrapped, lowest);
    }
    (Neighbor::Found, next)
}

fn pred_char(encoding: u8, code: u32, len: usize) -> Option<u32> {
    if code == 0 {
        return None;
    }
    let mut previous = code - 1;
    if encoding == UTF_8 && (0xd800..=0xdfff).contains(&previous) {
        previous = 0xd7ff;
    }
    (encoding != UTF_8 || utf8_len(previous) == len).then_some(previous)
}

/// `enc_succ_alnum_char`: the next character of the same class, allowing one
/// gap; or, at the end of a run of that class, the run's first character and
/// the carry to insert — the first again for letters, the second for digits
/// (`z` wraps to `a` carrying `a`, `9` to `0` carrying `1`).
fn succ_alnum_char(encoding: u8, code: u32, len: usize) -> (Neighbor, u32, u32) {
    let Some(class) = ctype_of(encoding, code) else {
        return (Neighbor::NotChar, code, code);
    };
    let mut probe = code;
    for _ in 0..=1 {
        let (found, next) = succ_char(encoding, probe, len);
        probe = next;
        if found == Neighbor::Found && ctype_of(encoding, next) == Some(class) {
            return (Neighbor::Found, next, next);
        }
    }
    let mut first = code;
    let mut range = 1;
    while let Some(previous) = pred_char(encoding, first, len) {
        if ctype_of(encoding, previous) != Some(class) {
            break;
        }
        first = previous;
        range += 1;
    }
    if range == 1 {
        return (Neighbor::NotChar, code, code);
    }
    let carry = match class {
        Ctype::Alpha => first,
        Ctype::Digit => succ_char(encoding, first, len).1,
    };
    (Neighbor::Wrapped, first, carry)
}

fn encode_char(encoding: u8, code: u32) -> Vec<u8> {
    if encoding != UTF_8 {
        return vec![code as u8];
    }
    let mut buffer = [0u8; 4];
    char::from_u32(code).map_or_else(Vec::new, |c| c.encode_utf8(&mut buffer).as_bytes().to_vec())
}

/// CRuby's `str_succ`, ported: the rightmost alphanumeric — Unicode's
/// letters and digits in UTF-8 — steps within its class, wrapping and
/// carrying leftward over the others; a non-alphanumeric between a letter and
/// a digit stops the carry (`"a.9"` → `"a.10"`), and a carry off the left
/// inserts a new first character. With no alphanumerics the last character
/// steps, wrapping bytewise. `None` for an encoding this VM cannot walk.
#[must_use]
pub fn succ(encoding: u8, bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let offsets = char_offsets(encoding, bytes)?;
    // Each character as (start, len, code), or None for an invalid byte,
    // which `succ` steps over.
    let chars: Vec<Option<(usize, usize, u32)>> = offsets
        .windows(2)
        .map(|w| {
            let piece = &bytes[w[0]..w[1]];
            if encoding == UTF_8 {
                std::str::from_utf8(piece)
                    .ok()
                    .and_then(|text| text.chars().next())
                    .map(|c| (w[0], piece.len(), u32::from(c)))
            } else {
                Some((w[0], 1, u32::from(piece[0])))
            }
        })
        .collect();
    let mut out = bytes.to_vec();
    let mut neighbor = Neighbor::Found;
    let mut last_alnum: Option<u8> = None;
    let mut any_alnum = false;
    let mut carry: Vec<u8> = vec![1];
    let mut carry_pos = 0;
    for &entry in chars.iter().rev() {
        let Some((start, len, code)) = entry else {
            continue;
        };
        if neighbor == Neighbor::NotChar
            && let Some(last) = last_alnum
        {
            let first = u32::from(out[start]);
            let alpha = |b: u32| u8::try_from(b).is_ok_and(|b| b.is_ascii_alphabetic());
            let digit = |b: u32| u8::try_from(b).is_ok_and(|b| b.is_ascii_digit());
            let last = u32::from(last);
            if (alpha(last) && digit(first)) || (digit(last) && alpha(first)) {
                break;
            }
        }
        let (result, next, carried) = succ_alnum_char(encoding, code, len);
        neighbor = result;
        match result {
            Neighbor::NotChar => continue,
            Neighbor::Found => {
                out.splice(start..start + len, encode_char(encoding, next));
                return Some(out);
            }
            Neighbor::Wrapped => {
                let wrapped = encode_char(encoding, next);
                last_alnum = wrapped.first().copied();
                out.splice(start..start + len, wrapped);
                any_alnum = true;
                carry_pos = start;
                carry = encode_char(encoding, carried);
            }
        }
    }
    if !any_alnum {
        for &entry in chars.iter().rev() {
            let Some((start, len, code)) = entry else {
                continue;
            };
            let (result, next) = succ_char(encoding, code, len);
            out.splice(start..start + len, encode_char(encoding, next));
            if result == Neighbor::Found {
                return Some(out);
            }
            carry_pos = start;
            carry = vec![1];
        }
    }
    out.splice(carry_pos..carry_pos, carry);
    Some(out)
}

/// The first `needle` in `haystack` at or after byte `start`.
#[must_use]
pub fn find(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if start > haystack.len() {
        return None;
    }
    if needle.is_empty() {
        return Some(start);
    }
    haystack[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + start)
}

/// The last `needle` in `haystack` starting at or before byte `start`.
#[must_use]
pub fn rfind(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    let last = start.min(haystack.len() - needle.len());
    (0..=last)
        .rev()
        .find(|&at| &haystack[at..at + needle.len()] == needle)
}

/// Case mapping (#19): kind 0 upcase, 1 downcase, 2 swapcase, 3 capitalize,
/// 4 fold. Full Unicode mapping for UTF-8 — Rust's, which is Unicode's,
/// `"ß".upcase` is `"SS"` — and ASCII letters only for `ascii_only`, for
/// US-ASCII and BINARY, and for an invalid UTF-8 string's bytes. `None` for an
/// encoding this VM cannot walk.
#[must_use]
pub fn case_map(
    encoding: u8,
    bytes: &[u8],
    kind: u8,
    ascii_only: bool,
    turkic: bool,
) -> Option<Vec<u8>> {
    let ascii = |b: u8, first: bool| -> u8 {
        match kind {
            0 => b.to_ascii_uppercase(),
            1 | 4 => b.to_ascii_lowercase(),
            2 if b.is_ascii_uppercase() => b.to_ascii_lowercase(),
            2 => b.to_ascii_uppercase(),
            _ if first => b.to_ascii_uppercase(),
            _ => b.to_ascii_lowercase(),
        }
    };
    let text = match encoding {
        UTF_8 if !ascii_only => std::str::from_utf8(bytes).ok(),
        UTF_8 | BINARY | US_ASCII => None,
        _ => return None,
    };
    let Some(text) = text else {
        return Some(
            bytes
                .iter()
                .enumerate()
                .map(|(i, &b)| ascii(b, i == 0))
                .collect(),
        );
    };
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.chars().enumerate() {
        let upper = |out: &mut String, c: char| match c {
            'i' if turkic => out.push('İ'),
            _ => out.extend(c.to_uppercase()),
        };
        let lower = |out: &mut String, c: char| match c {
            'I' if turkic => out.push('ı'),
            'İ' if turkic => out.push('i'),
            _ => out.extend(c.to_lowercase()),
        };
        match kind {
            0 => upper(&mut out, c),
            1 => lower(&mut out, c),
            4 => match c {
                // Case folding is lowercasing plus the few characters whose
                // fold differs: the sharp s folds to "ss".
                'ß' => out.push_str("ss"),
                _ => lower(&mut out, c),
            },
            2 if c.is_uppercase() => lower(&mut out, c),
            2 if c.is_lowercase() => upper(&mut out, c),
            2 => out.push(c),
            // Titlecase, which for a character whose uppercase is several
            // (`ß` → `SS`) is the first of them followed by the rest
            // lowercased: `"ß".capitalize` is `"Ss"`.
            _ if i == 0 => match titlecase(c) {
                Some(title) => out.push(title),
                None => {
                    let mut up = String::new();
                    upper(&mut up, c);
                    let mut chars = up.chars();
                    if let Some(first) = chars.next() {
                        out.push(first);
                    }
                    for rest in chars {
                        out.extend(rest.to_lowercase());
                    }
                }
            },
            _ => lower(&mut out, c),
        }
    }
    Some(out.into_bytes())
}

/// The few characters whose titlecase is not their uppercase: the Latin
/// digraphs, `ǆ` capitalizing to `ǅ`. Unicode's whole list of them.
fn titlecase(c: char) -> Option<char> {
    match c {
        'Ǆ' | 'ǅ' | 'ǆ' => Some('ǅ'),
        'Ǉ' | 'ǈ' | 'ǉ' => Some('ǈ'),
        'Ǌ' | 'ǋ' | 'ǌ' => Some('ǋ'),
        'Ǳ' | 'ǲ' | 'ǳ' => Some('ǲ'),
        _ => None,
    }
}

/// A non-negative float's digits as C's printf writes them for `%f`, `%e`
/// and `%g` (capitals for the uppercase forms), at `precision`. `alternate`
/// is `#`: a decimal point always, and `%g` keeps its trailing zeros. `None`
/// for any other conversion.
#[must_use]
pub fn format_float(f: f64, conversion: u8, precision: usize, alternate: bool) -> Option<String> {
    let upper = conversion.is_ascii_uppercase();
    let text = match conversion.to_ascii_lowercase() {
        b'f' => {
            let mut text = format!("{f:.precision$}");
            if alternate && precision == 0 {
                text.push('.');
            }
            text
        }
        b'e' => exponent_form(f, precision, alternate),
        b'g' => {
            let p = precision.max(1);
            let exponent = if f == 0.0 {
                0
            } else {
                // The exponent `%e` would print after rounding to `p` digits.
                let rounded = format!("{:.*e}", p - 1, f);
                rounded
                    .rsplit('e')
                    .next()
                    .and_then(|e| e.parse::<i32>().ok())
                    .unwrap_or(0)
            };
            let mut text = if exponent < -4 || exponent >= p as i32 {
                exponent_form(f, p - 1, alternate)
            } else {
                let decimals = (p as i32 - 1 - exponent).max(0) as usize;
                let mut text = format!("{f:.decimals$}");
                if alternate && !text.contains('.') {
                    text.push('.');
                }
                text
            };
            if !alternate {
                text = strip_fraction_zeros(&text);
            }
            text
        }
        _ => return None,
    };
    Some(if upper {
        text.to_ascii_uppercase()
    } else {
        text
    })
}

/// `1.234568e+04`: the mantissa at `precision` and a signed exponent of at
/// least two digits.
fn exponent_form(f: f64, precision: usize, alternate: bool) -> String {
    let raw = format!("{f:.precision$e}");
    let (mantissa, exponent) = raw.split_once('e').unwrap_or((&raw, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let mut mantissa = mantissa.to_owned();
    if alternate && precision == 0 {
        mantissa.push('.');
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}

/// `%g`'s trailing-zero rule: zeros after the point go, and the point with
/// them; an exponent part is left alone.
fn strip_fraction_zeros(text: &str) -> String {
    let (number, exponent) = match text.find('e') {
        Some(at) => (&text[..at], &text[at..]),
        None => (text, ""),
    };
    let number = if number.contains('.') {
        number.trim_end_matches('0').trim_end_matches('.')
    } else {
        number
    };
    format!("{number}{exponent}")
}

/// An encoding's canonical name, for messages.
#[must_use]
pub fn name(encoding: u8) -> &'static str {
    crate::encoding_table::ENCODINGS
        .get(usize::from(encoding))
        .map_or("?", |&(names, _, _)| names[0])
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
