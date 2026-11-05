//! CP50220 and CP50221 for transcoding (#419): Microsoft's ISO-2022-JPs,
//! stateful encodings whose escape sequences and SO and SI switch G0 between
//! ASCII, JIS X 0208 and the half-width katakana. CRuby converts them to and
//! from CP51932, Microsoft's EUC-JP, not stateless-ISO-2022-JP, and by hand
//! rather than by table: a character is the CP51932 one with the same seven
//! bits, the katakana after 0x8E.
//!
//! Both decode alike. CP50221 writes the katakana as `ESC ( I`; CP50220
//! writes them full-width, in JIS X 0208, and holds one that a voiced or
//! semi-voiced mark can join until the next character says whether it does.
//! What each writes for the katakana, and every shape the decoder reads, is
//! checked against CRuby by `scripts/jis-table.rb`; the rest is measured on
//! ruby 4.0.9.

use crate::iso2022jp::Fail;
use crate::jis::Jis;
use crate::jis_table::{CP50220_KANA, CP50220_SEMI_VOICED, CP50220_VOICED};
use crate::transcode::State;

pub const CP50220: u8 = 87;
pub const CP50221: u8 = 88;

/// What G0 holds, which a decoder reads and an encoder writes by. The
/// encoder writes JIS X 0208 as the 1983 set only.
const ASCII: u8 = 0;
const JIS_1978: u8 = 1;
const JIS_1983: u8 = 2;
const KANA: u8 = 3;

const _: () = assert!(CP50220_KANA.len() == 63, "a cell per katakana");

const fn invalid(error: usize, readagain: usize) -> Fail {
    Fail::Invalid { error, readagain }
}

/// Whether `encoding` is one of these.
#[must_use]
pub fn supported(encoding: u8) -> bool {
    encoding == CP50220 || encoding == CP50221
}

/// The character at `at`, appended to `out` as CP51932, and how many bytes it
/// took. An escape sequence, SO and SI switch G0 and write nothing. `ESC ( J`
/// is read as ASCII, `ESC $ @` as JIS X 0208 without the rows 1983 added and
/// Microsoft's NEC rows, and in ASCII a byte through 0x92 is itself and one
/// from 0xA1 to 0xDF a katakana, as CRuby reads them.
///
/// # Errors
///
/// A byte that is no character in G0, or an escape sequence that is none of
/// the five.
pub fn decode(
    bytes: &[u8],
    at: usize,
    state: &mut State,
    out: &mut Vec<u8>,
) -> Result<usize, Fail> {
    let rest = &bytes[at..];
    let lead = rest[0];
    match lead {
        0x1b => {
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
                (b'(', b'I') => KANA,
                (b'$', b'@') => JIS_1978,
                (b'$', b'B') => JIS_1983,
                _ => return Err(invalid(2, 1)),
            });
            return Ok(3);
        }
        0x0e => {
            state.set_decoder(KANA);
            return Ok(1);
        }
        0x0f => {
            state.set_decoder(ASCII);
            return Ok(1);
        }
        _ => {}
    }
    let kana = (0xa1..=0xdf).contains(&lead);
    let g0 = state.decoder();
    if g0 == ASCII || g0 == KANA {
        if kana || (g0 == KANA && (0x21..=0x5f).contains(&lead)) {
            out.extend_from_slice(&[0x8e, lead | 0x80]);
        } else if g0 == ASCII && lead <= 0x92 {
            out.push(lead);
        } else {
            return Err(invalid(1, 0));
        }
        return Ok(1);
    }
    let row = matches!(lead, 0x21..=0x28 | 0x30..=0x74)
        || (g0 == JIS_1983 && matches!(lead, 0x2d | 0x79..=0x7c));
    if !row {
        return Err(invalid(1, 0));
    }
    let Some(&second) = rest.get(1) else {
        return Err(Fail::Incomplete(1));
    };
    if !(0x21..=0x7e).contains(&second) {
        return Err(invalid(1, 1));
    }
    out.extend_from_slice(&[lead | 0x80, second | 0x80]);
    Ok(2)
}

/// Switches G0 to `g0`, with its escape sequence, if it holds another.
fn switch(g0: u8, state: &mut State, out: &mut Vec<u8>) {
    if state.encoder() != g0 {
        out.extend_from_slice(match g0 {
            ASCII => b"\x1b(B",
            KANA => b"\x1b(I",
            _ => b"\x1b$B",
        });
        state.set_encoder(g0);
    }
}

/// Writes a JIS X 0208 cell.
fn cell(cell: u16, state: &mut State, out: &mut Vec<u8>) {
    switch(JIS_1983, state, out);
    out.extend_from_slice(&[0x21 + (cell / 94) as u8, 0x21 + (cell % 94) as u8]);
}

/// The full-width cell CP50220 writes for a half-width katakana.
fn full_width(kana: u8) -> u16 {
    CP50220_KANA[usize::from(kana - 0xa1)]
}

/// Writes `c`, a CP51932 character, in `encoding`, with the escape sequence
/// that switches to its set when G0 holds another. CP50220 first writes the
/// katakana it holds, joined with `c` when `c` is the mark that joins it,
/// and holds a katakana a mark can join rather than write it.
///
/// SO, SI and ESC, which these cannot carry as text, are the caller's to
/// refuse: CRuby's converter has no spelling for them.
pub fn encode(encoding: u8, c: Jis, state: &mut State, out: &mut Vec<u8>) {
    if encoding == CP50220 {
        if let Some(held) = state.held_kana() {
            state.set_held_kana(None);
            let joined = match c {
                Jis::Kana(0xde) if CP50220_VOICED.contains(&held) => 1,
                Jis::Kana(0xdf) if CP50220_SEMI_VOICED.contains(&held) => 2,
                _ => 0,
            };
            cell(full_width(held) + joined, state, out);
            if joined != 0 {
                return;
            }
        }
        if let Jis::Kana(kana @ 0xa1..=0xdf) = c {
            if CP50220_VOICED.contains(&kana) || CP50220_SEMI_VOICED.contains(&kana) {
                state.set_held_kana(Some(kana));
            } else {
                cell(full_width(kana), state, out);
            }
            return;
        }
    }
    match c {
        Jis::Ascii(b) => {
            switch(ASCII, state, out);
            out.push(b);
        }
        Jis::Kana(b) => {
            switch(KANA, state, out);
            out.push(b & 0x7f);
        }
        Jis::Cell(c) => cell(c, state, out),
        // CP51932 has no JIS X 0212.
        Jis::Supplementary(_) => {}
    }
}

/// Ends the output: the katakana CP50220 holds, then back to ASCII, if G0
/// holds something else.
pub fn finish(state: &mut State, out: &mut Vec<u8>) {
    if let Some(held) = state.held_kana() {
        state.set_held_kana(None);
        cell(full_width(held), state, out);
    }
    switch(ASCII, state, out);
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

    fn encoded(encoding: u8, chars: &[Jis]) -> Vec<u8> {
        let (mut state, mut out) = (State::default(), Vec::new());
        for &c in chars {
            encode(encoding, c, &mut state, &mut out);
        }
        finish(&mut state, &mut out);
        assert_eq!(state, State::default());
        out
    }

    // Measured on ruby 4.0.9.
    #[test]
    fn decodes_as_cruby_does() {
        assert_eq!(
            decoded(b"\x1b$B$\"\x1b(I1\x1b(B"),
            Ok(b"\xa4\xa2\x8e\xb1".to_vec())
        );
        assert_eq!(
            decoded(b"\x0e1\x0fa\xb1\x80"),
            Ok(b"\x8e\xb1a\x8e\xb1\x80".to_vec())
        );
        assert_eq!(decoded(b"\x1b$B-!y!"), Ok(b"\xad\xa1\xf9\xa1".to_vec()));
        assert_eq!(decoded(b"\x1b$@-!"), Err((invalid(1, 0), 3)));
        assert_eq!(decoded(b"\x1b$Bu!"), Err((invalid(1, 0), 3)));
        assert_eq!(decoded(b"\x1b(Ia"), Err((invalid(1, 0), 3)));
        assert_eq!(decoded(b"\xa0"), Err((invalid(1, 0), 0)));
        assert_eq!(decoded(b"\x1b$B!\x7f"), Err((invalid(1, 1), 3)));
        assert_eq!(decoded(b"\x1b$A"), Err((invalid(2, 1), 0)));
        assert_eq!(decoded(b"\x1b("), Err((Fail::Incomplete(2), 0)));
    }

    #[test]
    fn cp50221_writes_the_katakana_as_esc_paren_i() {
        let chars = [Jis::Cell(3 * 94 + 1), Jis::Kana(0xb1)];
        assert_eq!(encoded(CP50221, &chars), b"\x1b$B$\"\x1b(I1\x1b(B");
        assert_eq!(
            encoded(
                CP50221,
                &[Jis::Kana(0xb6), Jis::Kana(0xde), Jis::Ascii(b'a')]
            ),
            b"\x1b(I6^\x1b(Ba"
        );
    }

    #[test]
    fn cp50220_writes_the_katakana_full_width() {
        let a = Jis::Cell(3 * 94 + 1);
        assert_eq!(
            encoded(CP50220, &[a, Jis::Kana(0xb1)]),
            b"\x1b$B$\"%\"\x1b(B"
        );
        assert_eq!(
            encoded(CP50220, &[Jis::Kana(0xb6), Jis::Kana(0xde)]),
            b"\x1b$B%,\x1b(B"
        );
        assert_eq!(
            encoded(CP50220, &[Jis::Kana(0xb6), Jis::Kana(0xdf)]),
            b"\x1b$B%+!,\x1b(B"
        );
        assert_eq!(
            encoded(CP50220, &[Jis::Kana(0xca), Jis::Kana(0xdf)]),
            b"\x1b$B%Q\x1b(B"
        );
        assert_eq!(
            encoded(CP50220, &[Jis::Kana(0xb3), Jis::Kana(0xde)]),
            b"\x1b$B%&!+\x1b(B"
        );
        assert_eq!(
            encoded(CP50220, &[Jis::Kana(0xb6), Jis::Ascii(b'a')]),
            b"\x1b$B%+\x1b(Ba"
        );
        assert_eq!(encoded(CP50220, &[Jis::Kana(0xb6)]), b"\x1b$B%+\x1b(B");
        assert_eq!(
            encoded(CP50220, &[Jis::Kana(0xb6), Jis::Kana(0xe0)]),
            b"\x1b$B%+\x1b(I`\x1b(B"
        );
    }
}
