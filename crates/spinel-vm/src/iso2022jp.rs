//! ISO-2022-JP for transcoding (#404): a stateful encoding, whose escape
//! sequences switch G0 between ASCII and JIS X 0208, and
//! stateless-ISO-2022-JP, the ASCII-compatible encoding CRuby converts it
//! through, which spells a JIS X 0208 character as Emacs-Mule does — `92`,
//! or `90` for the 1978 set, then the two bytes with their top bits set.
//!
//! CRuby's converters are `enc/trans/iso2022.trans`: ISO-2022-JP to and from
//! stateless-ISO-2022-JP, and stateless-ISO-2022-JP to and from EUC-JP, whose
//! two-byte characters are the same cells. Every shape here — what is
//! invalid, what is read again, which escapes are taken — is measured on ruby
//! 4.0.9.

use crate::transcode::State;

pub const STATELESS_ISO_2022_JP: u8 = 79;
pub const ISO_2022_JP: u8 = 85;

/// What G0 holds, which a decoder reads and an encoder writes by.
const ASCII: u8 = 0;
const JIS_1978: u8 = 1;
const JIS_1983: u8 = 2;

/// Why a sequence is not a character: `error` bytes are the bad sequence and
/// `readagain` more were read and are given back, or the input ended inside it.
#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    Invalid { error: usize, readagain: usize },
    Incomplete(usize),
}

const fn invalid(error: usize, readagain: usize) -> Fail {
    Fail::Invalid { error, readagain }
}

/// A character of stateless-ISO-2022-JP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Char {
    Ascii(u8),
    /// The set's leading byte, `90` or `92`, and the cell's two bytes as
    /// EUC-JP spells them.
    Kanji(u8, u8, u8),
}

/// The character at `at` of ISO-2022-JP, appended to `out` as
/// stateless-ISO-2022-JP, and how many bytes it took. An escape sequence
/// switches G0 and writes nothing. `ESC ( J`, JIS X 0201 Roman, is read as
/// ASCII, as CRuby does.
///
/// # Errors
///
/// A byte that is no character in G0, or an escape sequence that is none of
/// the four.
pub fn decode(
    bytes: &[u8],
    at: usize,
    state: &mut State,
    out: &mut Vec<u8>,
) -> Result<usize, Fail> {
    let rest = &bytes[at..];
    let lead = rest[0];
    if lead == 0x1b {
        let Some(&intermediate) = rest.get(1) else {
            return Err(Fail::Incomplete(1));
        };
        if intermediate != b'(' && intermediate != b'$' {
            return Err(invalid(1, 1));
        }
        let Some(&last) = rest.get(2) else {
            return Err(Fail::Incomplete(2));
        };
        state.set_decoder(match (intermediate, last) {
            (b'(', b'B' | b'J') => ASCII,
            (b'$', b'@') => JIS_1978,
            (b'$', b'B') => JIS_1983,
            _ => return Err(invalid(2, 1)),
        });
        return Ok(3);
    }
    if matches!(lead, 0x0e | 0x0f | 0x80..=0xff) {
        return Err(invalid(1, 0));
    }
    let g0 = state.decoder();
    if g0 == ASCII {
        out.push(lead);
        return Ok(1);
    }
    let in_set = |b: u8| (0x21..=0x7e).contains(&b);
    if !in_set(lead) {
        return Err(invalid(1, 0));
    }
    let Some(&second) = rest.get(1) else {
        return Err(Fail::Incomplete(1));
    };
    if !in_set(second) {
        return Err(invalid(1, 1));
    }
    let set = if g0 == JIS_1978 { 0x90 } else { 0x92 };
    out.extend_from_slice(&[set, lead | 0x80, second | 0x80]);
    Ok(2)
}

/// Reads the stateless-ISO-2022-JP character at `at`, and how many bytes it
/// took: the same shapes as EUC-JP's three-byte characters.
///
/// # Errors
///
/// A byte past ASCII that leads no character, or a bad or missing byte after
/// one that does.
pub fn read(bytes: &[u8], at: usize) -> Result<(Char, usize), Fail> {
    let rest = &bytes[at..];
    let lead = rest[0];
    if lead < 0x80 {
        return Ok((Char::Ascii(lead), 1));
    }
    if lead != 0x90 && lead != 0x92 {
        return Err(invalid(1, 0));
    }
    let follows = |b: u8| (0xa1..=0xfe).contains(&b);
    let Some(&first) = rest.get(1) else {
        return Err(Fail::Incomplete(1));
    };
    if !follows(first) {
        return Err(invalid(1, 1));
    }
    let Some(&second) = rest.get(2) else {
        return Err(Fail::Incomplete(2));
    };
    if !follows(second) {
        return Err(invalid(2, 1));
    }
    Ok((Char::Kanji(lead, first, second), 3))
}

/// Writes `c` as ISO-2022-JP, with the escape sequence that switches to its
/// set when G0 holds another.
///
/// # Errors
///
/// SO, SI and ESC, which ISO-2022-JP cannot carry as text: CRuby's encoder
/// reads them as invalid.
pub fn encode(c: Char, state: &mut State, out: &mut Vec<u8>) -> Result<(), Fail> {
    let g0 = match c {
        Char::Ascii(0x0e | 0x0f | 0x1b) => return Err(invalid(1, 0)),
        Char::Ascii(_) => ASCII,
        Char::Kanji(0x90, ..) => JIS_1978,
        Char::Kanji(..) => JIS_1983,
    };
    if state.encoder() != g0 {
        out.extend_from_slice(match g0 {
            ASCII => b"\x1b(B",
            JIS_1978 => b"\x1b$@",
            _ => b"\x1b$B",
        });
        state.set_encoder(g0);
    }
    match c {
        Char::Ascii(b) => out.push(b),
        Char::Kanji(_, first, second) => out.extend_from_slice(&[first & 0x7f, second & 0x7f]),
    }
    Ok(())
}

/// Ends ISO-2022-JP output: back to ASCII, if G0 holds something else.
pub fn finish(state: &mut State, out: &mut Vec<u8>) {
    if state.encoder() != ASCII {
        out.extend_from_slice(b"\x1b(B");
        state.set_encoder(ASCII);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(bytes: &[u8]) -> Result<Vec<u8>, (Fail, usize)> {
        let (mut state, mut out, mut at) = (State::default(), Vec::new(), 0);
        while at < bytes.len() {
            at += decode(bytes, at, &mut state, &mut out).map_err(|fail| (fail, at))?;
        }
        Ok(out)
    }

    // Measured on ruby 4.0.9.
    #[test]
    fn decodes_as_cruby_does() {
        assert_eq!(
            decoded(b"a\x1b$B$\"$$\x1b(Bb"),
            Ok(b"a\x92\xa4\xa2\x92\xa4\xa4b".to_vec())
        );
        assert_eq!(
            decoded(b"\x1b(Jb\x1b$@$\"\x1b(B"),
            Ok(b"b\x90\xa4\xa2".to_vec())
        );
        assert_eq!(decoded(b"a\x1b$C"), Err((invalid(2, 1), 1)));
        assert_eq!(decoded(b"a\x1bb"), Err((invalid(1, 1), 1)));
        assert_eq!(decoded(b"a\x1b$"), Err((Fail::Incomplete(2), 1)));
        assert_eq!(decoded(b"\x0e"), Err((invalid(1, 0), 0)));
        assert_eq!(decoded(b"\x1b$B$\n"), Err((invalid(1, 1), 3)));
        assert_eq!(decoded(b"\x1b$B\n"), Err((invalid(1, 0), 3)));
        assert_eq!(decoded(b"\x1b$B$"), Err((Fail::Incomplete(1), 3)));
    }

    #[test]
    fn encodes_with_the_escapes_it_needs() {
        let (mut state, mut out) = (State::default(), Vec::new());
        for c in [
            Char::Ascii(b'a'),
            Char::Kanji(0x92, 0xa4, 0xa2),
            Char::Kanji(0x90, 0xa4, 0xa2),
            Char::Ascii(b'b'),
        ] {
            encode(c, &mut state, &mut out).unwrap();
        }
        finish(&mut state, &mut out);
        assert_eq!(out, b"a\x1b$B$\"\x1b$@$\"\x1b(Bb");
        assert_eq!(
            encode(Char::Ascii(0x1b), &mut state, &mut out),
            Err(invalid(1, 0))
        );
        encode(Char::Kanji(0x92, 0xa4, 0xa2), &mut state, &mut out).unwrap();
        out.clear();
        finish(&mut state, &mut out);
        assert_eq!(out, b"\x1b(B");
        assert_eq!(state, State::default());
    }

    #[test]
    fn reads_stateless_as_cruby_does() {
        assert_eq!(read(b"\x92\xa1x", 0), Err(invalid(2, 1)));
        assert_eq!(read(b"\x92\x41", 0), Err(invalid(1, 1)));
        assert_eq!(read(b"\x91\xa1\xa1", 0), Err(invalid(1, 0)));
        assert_eq!(read(b"\x92\xa1", 0), Err(Fail::Incomplete(2)));
        assert_eq!(
            read(b"\x90\xa4\xa2", 0),
            Ok((Char::Kanji(0x90, 0xa4, 0xa2), 3))
        );
    }
}
