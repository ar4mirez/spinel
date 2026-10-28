//! Transcoding (#19): `String#encode` and `Encoding::Converter`, for the
//! encodings whose mapping to Unicode is arithmetic — UTF-8, US-ASCII,
//! BINARY, ISO-8859-1, and UTF-16 and UTF-32 in both byte orders — and the
//! JIS encodings whose mapping is CRuby's table, EUC-JP, Shift_JIS and
//! Windows-31J (#378, in `jis.rs`). Any other pair is refused by the caller.
//!
//! The unit of work is [`step`]: convert from a byte offset until the input
//! ends or a character cannot be converted, and report which and why. The
//! Ruby side decides what an error means — replace it, ask a fallback, or
//! raise with CRuby's attributes — and calls again past it.

use crate::strings::{BINARY, US_ASCII, UTF_8};

pub const UTF_16BE: u8 = 3;
pub const UTF_16LE: u8 = 4;
pub const UTF_32BE: u8 = 5;
pub const UTF_32LE: u8 = 6;
pub const ISO_8859_1: u8 = 22;

/// Whether this VM can transcode from or to `encoding`.
#[must_use]
pub fn supported(encoding: u8) -> bool {
    matches!(
        encoding,
        BINARY | UTF_8 | US_ASCII | UTF_16BE | UTF_16LE | UTF_32BE | UTF_32LE | ISO_8859_1
    ) || crate::jis::supported(encoding)
}

/// One source character, or why there is none.
enum Decoded {
    /// A codepoint, and how many bytes it took.
    Char(u32, usize),
    /// Bytes that are not a character: `error` of them are the bad
    /// sequence, then `readagain` more were read and are given back.
    Invalid { error: usize, readagain: usize },
    /// A sequence the input ended in the middle of.
    Incomplete(usize),
    /// A BINARY byte past ASCII, or a JIS character Unicode has none for: a
    /// character here, but no character anywhere else.
    Undefined(usize),
}

/// The range a UTF-8 continuation byte must fall in, after `lead` — Unicode's
/// Table 3-7, which is what rules out overlongs and surrogates.
fn second_byte_range(lead: u8) -> (u8, u8) {
    match lead {
        0xe0 => (0xa0, 0xbf),
        0xed => (0x80, 0x9f),
        0xf0 => (0x90, 0xbf),
        0xf4 => (0x80, 0x8f),
        _ => (0x80, 0xbf),
    }
}

fn decode(encoding: u8, bytes: &[u8], at: usize) -> Decoded {
    let rest = &bytes[at..];
    match encoding {
        UTF_8 => {
            let lead = rest[0];
            let want = match lead {
                0x00..=0x7f => return Decoded::Char(u32::from(lead), 1),
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => {
                    return Decoded::Invalid {
                        error: 1,
                        readagain: 0,
                    };
                }
            };
            let mut code = u32::from(lead) & (0xff >> (want + 1));
            for i in 1..want {
                let Some(&b) = rest.get(i) else {
                    return Decoded::Incomplete(i);
                };
                let (low, high) = if i == 1 {
                    second_byte_range(lead)
                } else {
                    (0x80, 0xbf)
                };
                if b < low || b > high {
                    return Decoded::Invalid {
                        error: i,
                        readagain: 1,
                    };
                }
                code = (code << 6) | u32::from(b & 0x3f);
            }
            Decoded::Char(code, want)
        }
        US_ASCII => {
            if rest[0] < 0x80 {
                Decoded::Char(u32::from(rest[0]), 1)
            } else {
                Decoded::Invalid {
                    error: 1,
                    readagain: 0,
                }
            }
        }
        BINARY => {
            if rest[0] < 0x80 {
                Decoded::Char(u32::from(rest[0]), 1)
            } else {
                Decoded::Undefined(1)
            }
        }
        ISO_8859_1 => Decoded::Char(u32::from(rest[0]), 1),
        UTF_16BE | UTF_16LE => {
            let unit = |i: usize| -> Option<u32> {
                let pair = rest.get(i..i + 2)?;
                Some(if encoding == UTF_16BE {
                    u32::from(pair[0]) << 8 | u32::from(pair[1])
                } else {
                    u32::from(pair[1]) << 8 | u32::from(pair[0])
                })
            };
            let Some(first) = unit(0) else {
                return Decoded::Incomplete(rest.len());
            };
            match first {
                0xd800..=0xdbff => match unit(2) {
                    None if rest.len() < 4 => Decoded::Incomplete(rest.len()),
                    Some(second @ 0xdc00..=0xdfff) => {
                        Decoded::Char(0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00), 4)
                    }
                    _ => Decoded::Invalid {
                        error: 2,
                        readagain: 2,
                    },
                },
                0xdc00..=0xdfff => Decoded::Invalid {
                    error: 2,
                    readagain: 0,
                },
                _ => Decoded::Char(first, 2),
            }
        }
        UTF_32BE | UTF_32LE => {
            let Some(quad) = rest.get(..4) else {
                return Decoded::Incomplete(rest.len());
            };
            let code = if encoding == UTF_32BE {
                u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]])
            } else {
                u32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]])
            };
            if code > 0x10_ffff || (0xd800..=0xdfff).contains(&code) {
                Decoded::Invalid {
                    error: 4,
                    readagain: 0,
                }
            } else {
                Decoded::Char(code, 4)
            }
        }
        _ if crate::jis::supported(encoding) => match crate::jis::read(encoding, bytes, at) {
            crate::jis::Read::Char(jis, len) => match crate::jis::to_unicode(encoding, jis) {
                Some(code) => Decoded::Char(code, len),
                None => Decoded::Undefined(len),
            },
            crate::jis::Read::Invalid { error, readagain } => Decoded::Invalid { error, readagain },
            crate::jis::Read::Incomplete(len) => Decoded::Incomplete(len),
        },
        _ => Decoded::Invalid {
            error: 1,
            readagain: 0,
        },
    }
}

/// `code` in `encoding`, or `None` when the encoding has no such character.
#[must_use]
pub fn encode(encoding: u8, code: u32) -> Option<Vec<u8>> {
    match encoding {
        UTF_8 => {
            let c = char::from_u32(code)?;
            let mut buffer = [0u8; 4];
            Some(c.encode_utf8(&mut buffer).as_bytes().to_vec())
        }
        US_ASCII | BINARY => (code < 0x80).then(|| vec![code as u8]),
        ISO_8859_1 => (code < 0x100).then(|| vec![code as u8]),
        UTF_16BE | UTF_16LE => {
            let c = char::from_u32(code)?;
            let mut units = [0u16; 2];
            let units = c.encode_utf16(&mut units);
            Some(
                units
                    .iter()
                    .flat_map(|u| {
                        if encoding == UTF_16BE {
                            u.to_be_bytes()
                        } else {
                            u.to_le_bytes()
                        }
                    })
                    .collect(),
            )
        }
        UTF_32BE => Some(code.to_be_bytes().to_vec()),
        UTF_32LE => Some(code.to_le_bytes().to_vec()),
        _ if crate::jis::supported(encoding) => {
            crate::jis::write(encoding, crate::jis::from_unicode(encoding, code)?)
        }
        _ => None,
    }
}

/// How a [`step`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    /// The input is all converted.
    Done,
    /// Bytes that are not a character of the source encoding.
    Invalid,
    /// The input ends inside a character.
    Incomplete,
    /// A character the destination encoding does not have — its codepoint,
    /// or `None` for a BINARY byte, which has none, and for a character the
    /// direct JIS converter met, which CRuby reports by its bytes. `reference`
    /// is the codepoint `xml:` writes it as, which that character does have
    /// when Unicode does.
    Undefined {
        code: Option<u32>,
        reference: Option<u32>,
    },
}

/// What one [`step`] produced.
pub struct Step {
    pub output: Vec<u8>,
    pub stop: Stop,
    /// The bytes that stopped it.
    pub error: Vec<u8>,
    /// Bytes read past them and given back, for an invalid sequence.
    pub readagain: Vec<u8>,
    /// Where to continue: past the error, but before `readagain`.
    pub next: usize,
}

/// What [`convert`] makes of one character.
type Converted = Result<(Option<Vec<u8>>, usize), (Stop, usize, usize)>;

/// One character at `at`: its bytes in `destination` (`None` for the source's
/// own, unchanged) and how many it took, or why it stops — with how many bytes
/// are the error and how many are read again.
///
/// A conversion to the same encoding is `scrub`'s: a character is kept as it
/// is, even one Unicode has no character for, and even one a round trip would
/// spell differently.
fn convert(source: u8, destination: u8, bytes: &[u8], at: usize) -> Converted {
    if crate::jis::direct(source, destination) {
        return match crate::jis::read(source, bytes, at) {
            crate::jis::Read::Char(jis, len) => crate::jis::write(destination, jis)
                .map(|encoded| (Some(encoded), len))
                .ok_or_else(|| {
                    let reference = crate::jis::to_unicode(source, jis);
                    (
                        Stop::Undefined {
                            code: None,
                            reference,
                        },
                        len,
                        0,
                    )
                }),
            crate::jis::Read::Invalid { error, readagain } => {
                Err((Stop::Invalid, error, readagain))
            }
            crate::jis::Read::Incomplete(len) => Err((Stop::Incomplete, len, 0)),
        };
    }
    match decode(source, bytes, at) {
        Decoded::Char(_, len) | Decoded::Undefined(len) if source == destination => Ok((None, len)),
        Decoded::Char(code, len) => encode(destination, code)
            .map(|encoded| (Some(encoded), len))
            .ok_or((
                Stop::Undefined {
                    code: Some(code),
                    reference: Some(code),
                },
                len,
                0,
            )),
        Decoded::Invalid { error, readagain } => Err((Stop::Invalid, error, readagain)),
        Decoded::Incomplete(len) => Err((Stop::Incomplete, len, 0)),
        Decoded::Undefined(len) => Err((
            Stop::Undefined {
                code: None,
                reference: None,
            },
            len,
            0,
        )),
    }
}

/// Convert `bytes` from `start` until the end or the first character that
/// cannot be converted.
#[must_use]
pub fn step(source: u8, destination: u8, bytes: &[u8], start: usize) -> Step {
    let mut output = Vec::with_capacity(bytes.len().saturating_sub(start));
    let mut at = start;
    while at < bytes.len() {
        let (stop, error_len, readagain) = match convert(source, destination, bytes, at) {
            Ok((encoded, len)) => {
                output.extend_from_slice(encoded.as_deref().unwrap_or(&bytes[at..at + len]));
                at += len;
                continue;
            }
            Err(stopped) => stopped,
        };
        let error = bytes[at..at + error_len].to_vec();
        let readagain_bytes =
            bytes[at + error_len..(at + error_len + readagain).min(bytes.len())].to_vec();
        return Step {
            output,
            stop,
            error,
            readagain: readagain_bytes,
            next: at + error_len,
        };
    }
    Step {
        output,
        stop: Stop::Done,
        error: Vec::new(),
        readagain: Vec::new(),
        next: bytes.len(),
    }
}

/// Character boundaries for the non-ASCII-compatible encodings and Latin-1,
/// so `length` and `[]` work on a transcoded string: UTF-16 in units with
/// surrogate pairs, UTF-32 in fours, a broken tail as one character.
#[must_use]
pub fn char_len(encoding: u8, bytes: &[u8], at: usize) -> Option<usize> {
    let rest = bytes.len() - at;
    match encoding {
        ISO_8859_1 => Some(1),
        UTF_16BE | UTF_16LE => {
            if rest < 2 {
                return Some(rest);
            }
            Some(match decode(encoding, bytes, at) {
                Decoded::Char(_, len) => len,
                _ => 2,
            })
        }
        UTF_32BE | UTF_32LE => Some(rest.min(4)),
        _ => None,
    }
}

/// Whether every character is well formed, for the same encodings.
#[must_use]
pub fn valid(encoding: u8, bytes: &[u8]) -> Option<bool> {
    match encoding {
        ISO_8859_1 => Some(true),
        UTF_32BE | UTF_32LE => Some(
            bytes.len() % 4 == 0
                && bytes.chunks(4).all(|quad| {
                    let quad = [quad[0], quad[1], quad[2], quad[3]];
                    // Onigmo reads the unit as a signed int, so a code with its top
                    // bit set passes its range check: measured, `valid_encoding?`
                    // holds for "\xFF\xFF\xFF\xFF" in UTF-32BE although
                    // transcoding refuses it.
                    let code = if encoding == UTF_32BE {
                        i32::from_be_bytes(quad)
                    } else {
                        i32::from_le_bytes(quad)
                    };
                    code <= 0x10_ffff && !(code < 0x1_0000 && (code >> 8) & 0xf8 == 0xd8)
                }),
        ),
        UTF_16BE | UTF_16LE => {
            let mut at = 0;
            while at < bytes.len() {
                match decode(encoding, bytes, at) {
                    Decoded::Char(_, len) => at += len,
                    _ => return Some(false),
                }
            }
            Some(true)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_utf8_reports_like_cruby() {
        let s = step(UTF_8, UTF_16LE, b"a\xe3b", 0);
        assert_eq!(s.stop, Stop::Invalid);
        assert_eq!(
            (s.error.as_slice(), s.readagain.as_slice()),
            (&b"\xe3"[..], &b"b"[..])
        );
        assert_eq!(s.next, 2);
        let s = step(UTF_8, UTF_16LE, b"a\xe3\x81", 0);
        assert_eq!(s.stop, Stop::Incomplete);
        assert_eq!(s.error, b"\xe3\x81");
    }

    #[test]
    fn utf16_round_trips_a_surrogate_pair() {
        let there = step(UTF_8, UTF_16LE, "a😀".as_bytes(), 0);
        assert_eq!(there.output, [0x61, 0, 0x3d, 0xd8, 0x00, 0xde]);
        let back = step(UTF_16LE, UTF_8, &there.output, 0);
        assert_eq!(back.output, "a😀".as_bytes());
    }

    #[test]
    fn utf32_validity_reads_the_unit_as_signed() {
        assert_eq!(valid(UTF_32BE, b"\xff\xff\xff\xff"), Some(true));
        assert_eq!(valid(UTF_32BE, b"\x7f\xff\xff\xff"), Some(false));
        assert_eq!(valid(UTF_32BE, b"\xff\xff\xd8\x00"), Some(false));
        assert_eq!(valid(UTF_32LE, b"\x00\xd8\x00\x00"), Some(false));
        assert_eq!(valid(UTF_32LE, b"\x00\x00\x00\x80"), Some(true));
        assert_eq!(valid(UTF_32LE, b"\x00\x00\x00"), Some(false));
    }

    #[test]
    fn undefined_carries_the_codepoint() {
        let s = step(UTF_8, US_ASCII, "aé".as_bytes(), 0);
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: Some(0xe9),
                reference: Some(0xe9)
            }
        );
        assert_eq!(s.output, b"a");
    }

    #[test]
    fn direct_jis_undefined_is_referenced_by_its_source_codepoint() {
        use crate::strings::{EUC_JP, SHIFT_JIS};
        for (bytes, reference) in [
            (&b"\x8f\xab\xb1a"[..], 0xe9),
            (b"\x8f\xa2\xaf", 0x2d8),
            (b"\x8f\xa9\xa1", 0xc6),
        ] {
            let s = step(EUC_JP, SHIFT_JIS, bytes, 0);
            assert_eq!(
                s.stop,
                Stop::Undefined {
                    code: None,
                    reference: Some(reference)
                }
            );
            assert_eq!(s.error, &bytes[..3]);
        }
        let s = step(EUC_JP, SHIFT_JIS, b"\x8e\xe0", 0);
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: None,
                reference: None
            }
        );
        let s = step(SHIFT_JIS, EUC_JP, b"\xf0\x40", 0);
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: None,
                reference: None
            }
        );
    }
}
