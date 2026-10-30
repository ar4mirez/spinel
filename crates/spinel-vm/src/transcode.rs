//! Transcoding (#19): `String#encode` and `Encoding::Converter`, for the
//! encodings whose mapping to Unicode is arithmetic — UTF-8, US-ASCII,
//! BINARY, ISO-8859-1, and UTF-16 and UTF-32 in both byte orders — the JIS
//! encodings whose mapping is CRuby's table, EUC-JP, Shift_JIS and
//! Windows-31J (#378, in `jis.rs`), and ISO-2022-JP and stateless-ISO-2022-JP
//! (#404, in `iso2022jp.rs`). Any other pair is refused by the caller.
//!
//! The unit of work is [`step`]: convert from a byte offset until the input
//! ends or a character cannot be converted, and report which and why. A
//! conversion runs along a path of CRuby's converters, which `core/` finds as
//! `search_convpath` does, a character at a time through each of them, so an
//! error names the converter that refused it, as CRuby's does. The Ruby side
//! decides what an error means — replace it, ask a fallback, or raise with
//! CRuby's attributes — and calls again past it.

use crate::iso2022jp::{ISO_2022_JP, STATELESS_ISO_2022_JP};
use crate::strings::{BINARY, EUC_JP, US_ASCII, UTF_8};

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
        BINARY
            | UTF_8
            | US_ASCII
            | UTF_16BE
            | UTF_16LE
            | UTF_32BE
            | UTF_32LE
            | ISO_8859_1
            | STATELESS_ISO_2022_JP
            | ISO_2022_JP
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
                // Big-endian, the next unit's first byte already says it is
                // no low surrogate, so only it is read again. Measured.
                0xd800..=0xdbff
                    if encoding == UTF_16BE
                        && rest.get(2).is_some_and(|b| !(0xdc..=0xdf).contains(b)) =>
                {
                    Decoded::Invalid {
                        error: 2,
                        readagain: 1,
                    }
                }
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
    /// The converter that stopped it, from and to: the first for an invalid
    /// sequence, and for an undefined character whichever had no spelling.
    pub from: u8,
    pub to: u8,
}

/// What a conversion keeps between steps, which Ruby holds as one Integer:
/// ISO-2022-JP's decoder G0 in the low two bits and its encoder's in the
/// next two, whether `xml: :attr` has opened its quote, and whether
/// `universal_newline` holds a CR. A fresh conversion is 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State(pub u8);

impl State {
    pub(crate) fn decoder(self) -> u8 {
        self.0 & 3
    }

    pub(crate) fn encoder(self) -> u8 {
        self.0 >> 2 & 3
    }

    pub(crate) fn set_decoder(&mut self, g0: u8) {
        self.0 = self.0 & !3 | g0;
    }

    pub(crate) fn set_encoder(&mut self, g0: u8) {
        self.0 = self.0 & !12 | g0 << 2;
    }

    fn quoted(self) -> bool {
        self.0 & 16 != 0
    }

    fn set_quoted(&mut self, quoted: bool) {
        self.0 = self.0 & !16 | u8::from(quoted) << 4;
    }

    fn held(self) -> bool {
        self.0 & 32 != 0
    }

    fn set_held(&mut self, held: bool) {
        self.0 = self.0 & !32 | u8::from(held) << 5;
    }
}

/// How `xml:` escapes: not at all, as text, or as an attribute's content.
/// The attribute's quote is a decorator of its own, which `xml: :attr` runs
/// with this one and `Encoding::Converter`'s flags can run alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Xml {
    #[default]
    None,
    Text,
    Attr,
}

/// Which newline decorator works: none; `universal_newline`, which `lf_newline`
/// is too, CRLF and CR to LF; `crlf_newline`, LF to CRLF; or `cr_newline`, LF
/// to CR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Newline {
    #[default]
    None,
    Universal,
    Crlf,
    Cr,
}

/// The decorators a conversion runs, which work together at one stage of its
/// path, the xml ones before the newline one. Measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decorators {
    pub xml: Xml,
    pub quote: bool,
    pub newline: Newline,
}

impl Decorators {
    fn any(self) -> bool {
        self != Self::default()
    }
}

/// The entity `xml:` writes for an ASCII byte, if it writes one.
fn escape(xml: Xml, byte: u8) -> Option<&'static [u8]> {
    match (xml, byte) {
        (Xml::None, _) => None,
        (_, b'&') => Some(b"&amp;"),
        (_, b'<') => Some(b"&lt;"),
        (_, b'>') => Some(b"&gt;"),
        (Xml::Attr, b'"') => Some(b"&quot;"),
        (Xml::Attr, b'\'') => Some(b"&apos;"),
        _ => None,
    }
}

/// Whether ASCII is ASCII in `encoding`, which is where CRuby's decorators
/// work: at the last such stage of a path.
fn ascii_compatible(encoding: u8) -> bool {
    !matches!(
        encoding,
        UTF_16BE | UTF_16LE | UTF_32BE | UTF_32LE | ISO_2022_JP
    )
}

/// Whether `encoding` is read to and written from a Unicode codepoint.
fn by_codepoint(encoding: u8) -> bool {
    matches!(
        encoding,
        BINARY | UTF_8 | US_ASCII | UTF_16BE | UTF_16LE | UTF_32BE | UTF_32LE | ISO_8859_1
    ) || crate::jis::supported(encoding)
}

/// Whether CRuby has a converter from `from` to `to` that this VM runs:
/// every encoding to and from UTF-8, EUC-JP to and from Shift_JIS, and
/// stateless-ISO-2022-JP to and from EUC-JP and ISO-2022-JP.
fn converts(from: u8, to: u8) -> bool {
    match (from, to) {
        (ISO_2022_JP, STATELESS_ISO_2022_JP)
        | (STATELESS_ISO_2022_JP, ISO_2022_JP | EUC_JP)
        | (EUC_JP, STATELESS_ISO_2022_JP) => true,
        _ if crate::jis::direct(from, to) => true,
        _ => from != to && (from == UTF_8 || to == UTF_8) && by_codepoint(from) && by_codepoint(to),
    }
}

/// What a [`hop`] makes of one character: how many bytes it took, or why it
/// stops, with how many bytes are the error and how many are read again.
type Hopped = Result<usize, (Stop, usize, usize)>;

fn failed(fail: crate::iso2022jp::Fail) -> (Stop, usize, usize) {
    match fail {
        crate::iso2022jp::Fail::Invalid { error, readagain } => (Stop::Invalid, error, readagain),
        crate::iso2022jp::Fail::Incomplete(len) => (Stop::Incomplete, len, 0),
    }
}

/// One of CRuby's converters: the character at `at`, from `from` to `to`,
/// appended to `out`. Nothing is appended when it stops.
///
/// A conversion to the same encoding is `scrub`'s: a character is kept as it
/// is, even one Unicode has no character for, and even one a round trip would
/// spell differently.
fn hop(from: u8, to: u8, bytes: &[u8], at: usize, state: &mut State, out: &mut Vec<u8>) -> Hopped {
    use crate::iso2022jp::{self, Char};
    use crate::jis::{self, Jis, Read};
    match (from, to) {
        (ISO_2022_JP, _) => iso2022jp::decode(bytes, at, state, out).map_err(failed),
        (STATELESS_ISO_2022_JP, _) => {
            let (c, len) = iso2022jp::read(bytes, at).map_err(failed)?;
            if to == ISO_2022_JP {
                iso2022jp::encode(c, state, out).map_err(failed)?;
            } else {
                match c {
                    Char::Ascii(b) => out.push(b),
                    Char::Kanji(_, first, second) => out.extend_from_slice(&[first, second]),
                }
            }
            Ok(len)
        }
        (EUC_JP, STATELESS_ISO_2022_JP) => match jis::read(EUC_JP, bytes, at) {
            // SO, SI and ESC, which ISO-2022-JP cannot carry as text, have no
            // spelling either. Measured.
            Read::Char(Jis::Ascii(b @ (0x0e | 0x0f | 0x1b)), len) => Err((
                Stop::Undefined {
                    code: None,
                    reference: Some(u32::from(b)),
                },
                len,
                0,
            )),
            Read::Char(Jis::Ascii(b), len) => {
                out.push(b);
                Ok(len)
            }
            Read::Char(Jis::Cell(cell), len) => {
                out.push(0x92);
                out.extend(jis::write(EUC_JP, Jis::Cell(cell)).unwrap_or_default());
                Ok(len)
            }
            // The katakana and JIS X 0212 have no spelling there. Measured.
            Read::Char(jis, len) => Err((
                Stop::Undefined {
                    code: None,
                    reference: jis::to_unicode(EUC_JP, jis),
                },
                len,
                0,
            )),
            Read::Invalid { error, readagain } => Err((Stop::Invalid, error, readagain)),
            Read::Incomplete(len) => Err((Stop::Incomplete, len, 0)),
        },
        _ if jis::direct(from, to) => match jis::read(from, bytes, at) {
            Read::Char(jis, len) => match jis::write(to, jis) {
                Some(encoded) => {
                    out.extend(encoded);
                    Ok(len)
                }
                None => Err((
                    Stop::Undefined {
                        code: None,
                        reference: jis::to_unicode(from, jis),
                    },
                    len,
                    0,
                )),
            },
            Read::Invalid { error, readagain } => Err((Stop::Invalid, error, readagain)),
            Read::Incomplete(len) => Err((Stop::Incomplete, len, 0)),
        },
        _ => match decode(from, bytes, at) {
            Decoded::Char(_, len) | Decoded::Undefined(len) if from == to => {
                out.extend_from_slice(&bytes[at..at + len]);
                Ok(len)
            }
            Decoded::Char(code, len) => match encode(to, code) {
                Some(encoded) => {
                    out.extend(encoded);
                    Ok(len)
                }
                None => Err((
                    Stop::Undefined {
                        code: Some(code),
                        reference: Some(code),
                    },
                    len,
                    0,
                )),
            },
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
        },
    }
}

/// A character a converter after the first refused, as the stage it was
/// refused at, why, and its bytes there.
struct Refused {
    stage: usize,
    stop: Stop,
    error: Vec<u8>,
}

/// Scratch for [`carry`], kept across characters so a step allocates once.
#[derive(Default)]
struct Stages {
    current: Vec<u8>,
    next: Vec<u8>,
}

/// Carries one character, `stages.current` in `path[k]`, through the rest of
/// `path` into `out`, decorated at stage `at`.
fn carry(
    path: &[u8],
    mut k: usize,
    stages: &mut Stages,
    state: &mut State,
    (decorators, at): (Decorators, Option<usize>),
    out: &mut Vec<u8>,
) -> Result<(), Refused> {
    let last = path.len() - 1;
    loop {
        if at == Some(k) {
            return decorate(path, k, stages, state, decorators, out);
        }
        if k == last {
            out.extend_from_slice(&stages.current);
            return Ok(());
        }
        stages.next.clear();
        match hop(
            path[k],
            path[k + 1],
            &stages.current,
            0,
            state,
            &mut stages.next,
        ) {
            Ok(_) => {}
            Err((stop, ..)) => {
                return Err(Refused {
                    stage: k,
                    stop,
                    error: std::mem::take(&mut stages.current),
                });
            }
        }
        std::mem::swap(&mut stages.current, &mut stages.next);
        k += 1;
    }
}

/// The decorators on one character at stage `k`, in CRuby's order: `xml:`'s
/// escape, its quote, which opens with the first character that reaches it
/// — so an entity written for an undefined one before is outside it — and
/// the newline decorator, which every piece of either goes through.
/// Measured.
fn decorate(
    path: &[u8],
    k: usize,
    stages: &mut Stages,
    state: &mut State,
    decorators: Decorators,
    out: &mut Vec<u8>,
) -> Result<(), Refused> {
    let Decorators {
        xml,
        quote,
        newline,
    } = decorators;
    if quote && !state.quoted() {
        state.set_quoted(true);
        newlines(path, k, b"\"", state, newline, out)?;
    }
    let entity = match stages.current.as_slice() {
        &[byte] => escape(xml, byte),
        _ => None,
    };
    match entity {
        Some(entity) => newlines(path, k, entity, state, newline, out),
        None => newlined(path, k, stages, state, newline, out),
    }
}

/// ASCII `text` at stage `k` through the newline decorator, byte by byte.
fn newlines(
    path: &[u8],
    k: usize,
    text: &[u8],
    state: &mut State,
    newline: Newline,
    out: &mut Vec<u8>,
) -> Result<(), Refused> {
    for &byte in text {
        let mut inner = Stages {
            current: vec![byte],
            ..Stages::default()
        };
        newlined(path, k, &mut inner, state, newline, out)?;
    }
    Ok(())
}

/// One character at stage `k` through the newline decorator, then the rest
/// of the way. `universal_newline` holds a CR until it sees what follows: an
/// LF with it is one LF, and anything else follows an LF. What is inserted
/// for an error goes past it, so it comes before a CR held. Measured.
fn newlined(
    path: &[u8],
    k: usize,
    stages: &mut Stages,
    state: &mut State,
    newline: Newline,
    out: &mut Vec<u8>,
) -> Result<(), Refused> {
    let byte = match stages.current.as_slice() {
        &[byte] => Some(byte),
        _ => None,
    };
    match (newline, byte) {
        (Newline::Universal, Some(b'\r')) => {
            let held = state.held();
            state.set_held(true);
            return if held {
                unescaped(path, k, b"\n", state, out)
            } else {
                Ok(())
            };
        }
        (Newline::Universal, Some(b'\n')) => state.set_held(false),
        (Newline::Universal, _) if state.held() => {
            state.set_held(false);
            unescaped(path, k, b"\n", state, out)?;
        }
        (Newline::Crlf, Some(b'\n')) => return unescaped(path, k, b"\r\n", state, out),
        (Newline::Cr, Some(b'\n')) => stages.current[0] = b'\r',
        _ => {}
    }
    carry(path, k, stages, state, (Decorators::default(), None), out)
}

/// Carries ASCII `text` from stage `k` the rest of the way, undecorated.
fn unescaped(
    path: &[u8],
    k: usize,
    text: &[u8],
    state: &mut State,
    out: &mut Vec<u8>,
) -> Result<(), Refused> {
    for &byte in text {
        let mut inner = Stages {
            current: vec![byte],
            ..Stages::default()
        };
        carry(
            path,
            k,
            &mut inner,
            state,
            (Decorators::default(), None),
            out,
        )?;
    }
    Ok(())
}

/// Convert `bytes` from `start` along `path` until the end or the first
/// character that cannot be converted. `path` is every encoding the
/// conversion passes through, as `Encoding::Converter.search_convpath` has
/// them, or one encoding alone, whose characters are read and kept as they
/// are. `state` is the conversion's at `start`, and is left as it is at
/// `next`; `finish` ends the output once the input is all converted: closes
/// `xml: :attr`'s quote, writes the CR `universal_newline` holds, and goes
/// back to ASCII in ISO-2022-JP. `None` for a path with a step this VM has
/// no converter for.
#[must_use]
pub fn step(
    path: &[u8],
    bytes: &[u8],
    start: usize,
    state: &mut State,
    decorators: Decorators,
    finish: bool,
) -> Option<Step> {
    let (&first, &end) = (path.first()?, path.last()?);
    let last = path.len() - 1;
    let readable = path.len() > 1 || by_codepoint(first);
    if !readable || !path.windows(2).all(|pair| converts(pair[0], pair[1])) {
        return None;
    }
    // Where the source itself is the last such stage — UTF-8 into UTF-16 or
    // UTF-32 — CRuby's decorators work on bytes before they are read, which
    // is the caller's to do.
    let decorate = if decorators.any() {
        (1..=last).rev().find(|&i| ascii_compatible(path[i]))
    } else {
        None
    };
    let mut output = Vec::with_capacity(bytes.len().saturating_sub(start));
    if decorators.quote && decorate == Some(last) && !state.quoted() {
        // At the end of the path the quote opens before anything, even what
        // is inserted for an error before the first character; before an
        // encoder it opens with the first character that reaches it, in
        // `decorate`. Measured.
        output.push(b'"');
        state.set_quoted(true);
    }
    let mut stages = Stages::default();
    let mut at = start;
    while at < bytes.len() {
        let to = path.get(1).copied().unwrap_or(first);
        stages.current.clear();
        let len = match hop(first, to, bytes, at, state, &mut stages.current) {
            Ok(len) => len,
            Err((stop, error_len, readagain)) => {
                let readagain_end = (at + error_len + readagain).min(bytes.len());
                return Some(Step {
                    output,
                    stop,
                    error: bytes[at..at + error_len].to_vec(),
                    readagain: bytes[at + error_len..readagain_end].to_vec(),
                    next: at + error_len,
                    from: first,
                    to,
                });
            }
        };
        // An escape sequence is no character, and goes no further.
        if !stages.current.is_empty() {
            if let Err(refused) = carry(
                path,
                last.min(1),
                &mut stages,
                state,
                (decorators, decorate),
                &mut output,
            ) {
                return Some(Step {
                    output,
                    stop: refused.stop,
                    error: refused.error,
                    readagain: Vec::new(),
                    next: at + len,
                    from: path[refused.stage],
                    to: path[(refused.stage + 1).min(last)],
                });
            }
        }
        at += len;
    }
    if finish {
        if let Some(stage) = decorate {
            // ASCII, which every converter has a spelling for.
            if decorators.quote {
                let quotes: &[u8] = if state.quoted() { b"\"" } else { b"\"\"" };
                let _ = newlines(path, stage, quotes, state, decorators.newline, &mut output);
                state.set_quoted(false);
            }
            if state.held() {
                state.set_held(false);
                let _ = unescaped(path, stage, b"\n", state, &mut output);
            }
        }
        if end == ISO_2022_JP {
            crate::iso2022jp::finish(state, &mut output);
        }
    }
    Some(Step {
        output,
        stop: Stop::Done,
        error: Vec::new(),
        readagain: Vec::new(),
        next: bytes.len(),
        from: first,
        to: end,
    })
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

    fn xml(xml: Xml) -> Decorators {
        Decorators {
            xml,
            quote: xml == Xml::Attr,
            ..Decorators::default()
        }
    }

    /// A conversion from the start, from a fresh state, undecorated.
    fn run(path: &[u8], bytes: &[u8]) -> Step {
        step(
            path,
            bytes,
            0,
            &mut State::default(),
            Decorators::default(),
            true,
        )
        .expect("a path this VM runs")
    }

    #[test]
    fn invalid_utf8_reports_like_cruby() {
        let s = run(&[UTF_8, UTF_16LE], b"a\xe3b");
        assert_eq!(s.stop, Stop::Invalid);
        assert_eq!(
            (s.error.as_slice(), s.readagain.as_slice()),
            (&b"\xe3"[..], &b"b"[..])
        );
        assert_eq!(s.next, 2);
        let s = run(&[UTF_8, UTF_16LE], b"a\xe3\x81");
        assert_eq!(s.stop, Stop::Incomplete);
        assert_eq!(s.error, b"\xe3\x81");
    }

    #[test]
    fn utf16_round_trips_a_surrogate_pair() {
        let there = run(&[UTF_8, UTF_16LE], "a😀".as_bytes());
        assert_eq!(there.output, [0x61, 0, 0x3d, 0xd8, 0x00, 0xde]);
        let back = run(&[UTF_16LE, UTF_8], &there.output);
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
        let s = run(&[UTF_8, US_ASCII], "aé".as_bytes());
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
        use crate::strings::SHIFT_JIS;
        for (bytes, reference) in [
            (&b"\x8f\xab\xb1a"[..], 0xe9),
            (b"\x8f\xa2\xaf", 0x2d8),
            (b"\x8f\xa9\xa1", 0xc6),
        ] {
            let s = run(&[EUC_JP, SHIFT_JIS], bytes);
            assert_eq!(
                s.stop,
                Stop::Undefined {
                    code: None,
                    reference: Some(reference)
                }
            );
            assert_eq!(s.error, &bytes[..3]);
        }
        let s = run(&[EUC_JP, SHIFT_JIS], b"\x8e\xe0");
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: None,
                reference: None
            }
        );
        let s = run(&[SHIFT_JIS, EUC_JP], b"\xf0\x40");
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: None,
                reference: None
            }
        );
    }

    const TO_ISO_2022_JP: [u8; 4] = [UTF_8, EUC_JP, STATELESS_ISO_2022_JP, ISO_2022_JP];
    const FROM_ISO_2022_JP: [u8; 4] = [ISO_2022_JP, STATELESS_ISO_2022_JP, EUC_JP, UTF_8];

    // The issue's example, and the rest measured on ruby 4.0.9.
    #[test]
    fn iso_2022_jp_goes_through_stateless_and_euc_jp() {
        assert_eq!(
            run(&TO_ISO_2022_JP, "あ".as_bytes()).output,
            b"\x1b$B$\"\x1b(B"
        );
        let s = run(&FROM_ISO_2022_JP, b"a\x1b$B$\"$$\x1b(Bb");
        assert_eq!(
            (s.stop, s.output),
            (Stop::Done, "aあいb".as_bytes().to_vec())
        );
        // Back to ASCII only when the output is finished.
        let mut state = State::default();
        let s = step(
            &TO_ISO_2022_JP,
            "あ".as_bytes(),
            0,
            &mut state,
            Decorators::default(),
            false,
        );
        assert_eq!(s.map(|s| s.output), Some(b"\x1b$B$\"".to_vec()));
        let s = step(
            &TO_ISO_2022_JP,
            b"",
            0,
            &mut state,
            Decorators::default(),
            true,
        );
        assert_eq!(s.map(|s| s.output), Some(b"\x1b(B".to_vec()));
    }

    #[test]
    fn an_error_names_the_converter_that_refused() {
        let s = run(&FROM_ISO_2022_JP, b"\x1b$B)!");
        assert_eq!(
            (s.from, s.to, s.error.as_slice()),
            (EUC_JP, UTF_8, &b"\xa9\xa1"[..])
        );
        assert_eq!(s.next, 5);
        let s = run(&FROM_ISO_2022_JP, b"a\x1b$C");
        assert_eq!(s.stop, Stop::Invalid);
        assert_eq!((s.from, s.to), (ISO_2022_JP, STATELESS_ISO_2022_JP));
        let s = run(&TO_ISO_2022_JP, "aｱ".as_bytes());
        assert_eq!(
            (s.from, s.to, s.error.as_slice()),
            (EUC_JP, STATELESS_ISO_2022_JP, &b"\x8e\xb1"[..])
        );
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: None,
                reference: Some(0xff71)
            }
        );
        // ESC has no spelling in stateless-ISO-2022-JP; the encoder after it
        // refuses one it is given as invalid.
        let s = run(&TO_ISO_2022_JP, b"\x1b");
        assert_eq!(
            (s.from, s.to, s.error.as_slice()),
            (EUC_JP, STATELESS_ISO_2022_JP, &b"\x1b"[..])
        );
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: None,
                reference: Some(0x1b)
            }
        );
        let s = run(&[STATELESS_ISO_2022_JP, ISO_2022_JP], b"\x1b");
        assert_eq!(s.stop, Stop::Invalid);
        let s = run(&TO_ISO_2022_JP, "😀".as_bytes());
        assert_eq!((s.from, s.to), (UTF_8, EUC_JP));
        assert_eq!(
            s.stop,
            Stop::Undefined {
                code: Some(0x1f600),
                reference: Some(0x1f600)
            }
        );
    }

    #[test]
    fn xml_escapes_after_a_stateful_source_is_decoded() {
        // `<` in JIS X 0208 is half a character, not markup.
        let s = step(
            &FROM_ISO_2022_JP,
            b"<\x1b$B<!\x1b(B",
            0,
            &mut State::default(),
            xml(Xml::Text),
            true,
        );
        assert_eq!(s.map(|s| s.output), Some("&lt;次".as_bytes().to_vec()));
        // Through UTF-8 into UTF-16, after the first converter.
        let s = step(
            &[EUC_JP, UTF_8, UTF_16LE],
            b"<",
            0,
            &mut State::default(),
            xml(Xml::Text),
            true,
        );
        assert_eq!(s.map(|s| s.output), Some(b"&\0l\0t\0;\0".to_vec()));
    }

    #[test]
    fn an_attribute_opens_its_quote_with_the_first_character_through() {
        let attr = |bytes: &[u8]| {
            step(
                &TO_ISO_2022_JP,
                bytes,
                0,
                &mut State::default(),
                xml(Xml::Attr),
                true,
            )
            .unwrap()
        };
        assert_eq!(attr("あ".as_bytes()).output, b"\"\x1b$B$\"\x1b(B\"");
        assert_eq!(attr(b"").output, b"\"\"");
        // ESC is refused before it reaches the quote.
        let s = attr(b"\x1b");
        assert_eq!((s.output.as_slice(), s.from), (&b""[..], EUC_JP));
        // Into an ASCII-compatible destination, the quote comes first.
        let s = step(
            &[UTF_8, US_ASCII],
            "ü".as_bytes(),
            0,
            &mut State::default(),
            xml(Xml::Attr),
            true,
        );
        assert_eq!(s.map(|s| s.output), Some(b"\"".to_vec()));
    }

    #[test]
    fn the_attribute_quote_is_a_decorator_of_its_own() {
        let attr = |decorators| {
            step(
                &[UTF_8, EUC_JP],
                b"<'\"",
                0,
                &mut State::default(),
                decorators,
                true,
            )
            .map(|s| s.output)
        };
        let quote = Decorators {
            quote: true,
            ..Decorators::default()
        };
        assert_eq!(attr(quote), Some(b"\"<'\"\"".to_vec()));
        assert_eq!(
            attr(Decorators {
                xml: Xml::Attr,
                ..Decorators::default()
            }),
            Some(b"&lt;&apos;&quot;".to_vec())
        );
        assert_eq!(
            attr(Decorators {
                xml: Xml::Text,
                ..quote
            }),
            Some(b"\"&lt;'\"\"".to_vec())
        );
    }

    #[test]
    fn newlines_are_decorated_where_ascii_is_ascii() {
        let newline = |path: &[u8], bytes: &[u8], newline| {
            let decorators = Decorators {
                newline,
                ..Decorators::default()
            };
            step(path, bytes, 0, &mut State::default(), decorators, true)
                .unwrap()
                .output
        };
        // Before the encoder into UTF-16, so the CR is a character there.
        assert_eq!(
            newline(&[EUC_JP, UTF_8, UTF_16LE], b"a\nb", Newline::Crlf),
            b"a\0\r\0\n\0b\0"
        );
        // After the decoder from ISO-2022-JP, so its JIS X 0208 bytes are not
        // newlines and its CR and LF are its own.
        assert_eq!(
            newline(
                &FROM_ISO_2022_JP,
                b"a\r\n\x1b$B$\"\x1b(B\r",
                Newline::Universal
            ),
            "a\nあ\n".as_bytes()
        );
        assert_eq!(
            newline(&TO_ISO_2022_JP, "\nあ\n".as_bytes(), Newline::Cr),
            b"\r\x1b$B$\"\x1b(B\r"
        );
    }

    #[test]
    fn universal_newline_holds_a_cr_across_steps() {
        let decorators = Decorators {
            newline: Newline::Universal,
            ..Decorators::default()
        };
        let mut state = State::default();
        let path = [UTF_8, US_ASCII];
        let s = step(&path, "a\ré".as_bytes(), 0, &mut state, decorators, false).unwrap();
        // The CR waits while the error is reported, and what is inserted for
        // it comes first.
        assert_eq!((s.output.as_slice(), s.next), (&b"a"[..], 4));
        let s = step(
            &path,
            "a\ré\nb".as_bytes(),
            4,
            &mut state,
            decorators,
            false,
        )
        .unwrap();
        assert_eq!(s.output, b"\nb");
        let s = step(&path, b"\r", 0, &mut state, decorators, true).unwrap();
        assert_eq!(s.output, b"\n");
        assert_eq!(state, State::default());
    }

    #[test]
    fn an_attribute_quote_goes_through_the_newline_decorator() {
        let decorators = Decorators {
            xml: Xml::Attr,
            quote: true,
            newline: Newline::Universal,
        };
        let s = step(
            &[UTF_8, EUC_JP],
            b"\r<",
            0,
            &mut State::default(),
            decorators,
            true,
        );
        assert_eq!(s.map(|s| s.output), Some(b"\"\n&lt;\"".to_vec()));
        let s = step(
            &TO_ISO_2022_JP,
            b"a\r",
            0,
            &mut State::default(),
            decorators,
            true,
        );
        assert_eq!(s.map(|s| s.output), Some(b"\"a\n\"".to_vec()));
    }

    #[test]
    fn a_path_without_a_converter_is_refused() {
        assert!(
            step(
                &[UTF_16LE, UTF_16BE],
                b"",
                0,
                &mut State::default(),
                Decorators::default(),
                true
            )
            .is_none()
        );
        assert!(
            step(
                &[ISO_2022_JP],
                b"",
                0,
                &mut State::default(),
                Decorators::default(),
                true
            )
            .is_none()
        );
        assert!(
            step(
                &[],
                b"",
                0,
                &mut State::default(),
                Decorators::default(),
                true
            )
            .is_none()
        );
    }
}
