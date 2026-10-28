//! The JIS encodings — EUC-JP, Shift_JIS and Windows-31J — for transcoding
//! (#378). Reading a character is arithmetic: which bytes lead and which
//! follow. What it means in Unicode is a table, generated from CRuby into
//! `jis_table.rs` by `scripts/jis-table.rb`, which also checks everything this
//! file assumes about the bytes against CRuby.
//!
//! A character is a *cell*, its row and column from 0 in a plane of 94
//! columns: JIS X 0208 for EUC-JP's two bytes and Shift_JIS's, JIS X 0212 for
//! EUC-JP's three, and Windows-31J's 120 rows of two-byte Shift_JIS codes.
//! ASCII and the half-width katakana are arithmetic and have no cell.

use crate::jis_table;
use crate::strings::{EUC_JP, SHIFT_JIS, WINDOWS_31J};

const CELLS: usize = 94 * 94;
const WINDOWS_CELLS: usize = 120 * 94;

/// A plane's text, one character per cell and NUL for none, as codepoints.
/// Evaluated at compile time; a plane of the wrong size does not compile.
const fn plane<const N: usize>(text: &str) -> [u16; N] {
    let bytes = text.as_bytes();
    let mut out = [0u16; N];
    let (mut at, mut cell) = (0, 0);
    while at < bytes.len() {
        let lead = bytes[at] as u32;
        let (mut code, len) = match lead {
            0x00..=0x7f => (lead, 1),
            0xc0..=0xdf => (lead & 0x1f, 2),
            0xe0..=0xef => (lead & 0x0f, 3),
            _ => panic!("a JIS plane is in the BMP"),
        };
        let mut i = 1;
        while i < len {
            code = (code << 6) | (bytes[at + i] as u32 & 0x3f);
            i += 1;
        }
        out[cell] = code as u16;
        cell += 1;
        at += len;
    }
    assert!(cell == N, "a JIS plane has the wrong number of cells");
    out
}

/// How CRuby writes each character as a cell, sorted by codepoint: every
/// cell of `first` then `second` that reads as a character, but those CRuby
/// writes another cell for, and the characters it writes as a cell that reads
/// as something else. Evaluated at compile time, from the planes.
const fn writer<const N: usize>(
    first: &[u16],
    second: &[u16],
    unchosen: &[u16],
    one_way: &[(u16, u16)],
) -> [(u16, u16); N] {
    let mut by_code = [u16::MAX; 0x1_0000];
    let (mut cell, mut skip) = (0, 0);
    while cell < first.len() + second.len() {
        let code = if cell < first.len() {
            first[cell]
        } else {
            second[cell - first.len()]
        };
        if skip < unchosen.len() && unchosen[skip] as usize == cell {
            skip += 1;
        } else if code != 0 {
            assert!(
                by_code[code as usize] == u16::MAX,
                "two cells write one character"
            );
            by_code[code as usize] = cell as u16;
        }
        cell += 1;
    }
    assert!(skip == unchosen.len(), "the unchosen cells are sorted");
    let mut i = 0;
    while i < one_way.len() {
        by_code[one_way[i].0 as usize] = one_way[i].1;
        i += 1;
    }
    let mut out = [(0u16, 0u16); N];
    let (mut code, mut n) = (0, 0);
    while code < by_code.len() {
        if by_code[code] != u16::MAX {
            out[n] = (code as u16, by_code[code]);
            n += 1;
        }
        code += 1;
    }
    assert!(n == N, "the table's count of written characters");
    out
}

static JIS0208: [u16; CELLS] = plane(jis_table::JIS0208);
static JIS0212: [u16; CELLS] = plane(jis_table::JIS0212);
static WINDOWS_31J_READ: [u16; WINDOWS_CELLS] = plane(jis_table::WINDOWS_31J);

static EUC_JP_WRITE: [(u16, u16); jis_table::EUC_JP_WRITTEN] = writer(
    &JIS0208,
    &JIS0212,
    jis_table::EUC_JP_UNCHOSEN,
    jis_table::EUC_JP_ONE_WAY,
);
static SHIFT_JIS_WRITE: [(u16, u16); jis_table::SHIFT_JIS_WRITTEN] = writer(
    &JIS0208,
    &[],
    jis_table::SHIFT_JIS_UNCHOSEN,
    jis_table::SHIFT_JIS_ONE_WAY,
);
static WINDOWS_31J_WRITE: [(u16, u16); jis_table::WINDOWS_31J_WRITTEN] = writer(
    &WINDOWS_31J_READ,
    &[],
    jis_table::WINDOWS_31J_UNCHOSEN,
    jis_table::WINDOWS_31J_ONE_WAY,
);

/// Whether `encoding` is one of these.
#[must_use]
pub fn supported(encoding: u8) -> bool {
    matches!(encoding, EUC_JP | SHIFT_JIS | WINDOWS_31J)
}

/// Whether CRuby converts between the two without going through UTF-8: EUC-JP
/// and Shift_JIS are one plane spelled two ways, so their converter is
/// arithmetic and keeps cells Unicode has no character for. Measured.
#[must_use]
pub fn direct(source: u8, destination: u8) -> bool {
    matches!(
        (source, destination),
        (EUC_JP, SHIFT_JIS) | (SHIFT_JIS, EUC_JP)
    )
}

/// A character of these encodings, before it meets Unicode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jis {
    Ascii(u8),
    /// The half-width katakana byte, 0xA1 to 0xDF; EUC-JP also reads 0xE0 to
    /// 0xFE after 0x8E, which is no character.
    Kana(u8),
    /// A cell of JIS X 0208, or of Windows-31J's 120 rows.
    Cell(u16),
    /// A cell of JIS X 0212, EUC-JP's after 0x8F.
    Supplementary(u16),
}

/// One character read, or why there is none — CRuby's shapes: a bad lead
/// byte is the error alone; a bad byte after a lead is given back to be read
/// again.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    Char(Jis, usize),
    Invalid { error: usize, readagain: usize },
    Incomplete(usize),
}

/// Reads the character at `at`.
#[must_use]
pub fn read(encoding: u8, bytes: &[u8], at: usize) -> Read {
    let rest = &bytes[at..];
    let lead = rest[0];
    let invalid = |error, readagain| Read::Invalid { error, readagain };
    if lead < 0x80 {
        return Read::Char(Jis::Ascii(lead), 1);
    }
    if encoding == EUC_JP {
        let follows = |b: u8| (0xa1..=0xfe).contains(&b);
        if !(lead == 0x8e || lead == 0x8f || follows(lead)) {
            return invalid(1, 0);
        }
        let Some(&second) = rest.get(1) else {
            return Read::Incomplete(1);
        };
        if !follows(second) {
            return invalid(1, 1);
        }
        let cell = |a: u8, b: u8| u16::from(a - 0xa1) * 94 + u16::from(b - 0xa1);
        return match lead {
            0x8e => Read::Char(Jis::Kana(second), 2),
            0x8f => match rest.get(2) {
                None => Read::Incomplete(2),
                Some(&third) if follows(third) => {
                    Read::Char(Jis::Supplementary(cell(second, third)), 3)
                }
                Some(_) => invalid(2, 1),
            },
            _ => Read::Char(Jis::Cell(cell(lead, second)), 2),
        };
    }
    match lead {
        0xa1..=0xdf => Read::Char(Jis::Kana(lead), 1),
        0x81..=0x9f | 0xe0..=0xfc => match rest.get(1) {
            None => Read::Incomplete(1),
            Some(&trail @ (0x40..=0x7e | 0x80..=0xfc)) => {
                let row = u16::from(lead - if lead < 0xa0 { 0x81 } else { 0xc1 }) * 2;
                let trail = u16::from(trail - if trail < 0x7f { 0x40 } else { 0x41 });
                Read::Char(Jis::Cell((row + trail / 94) * 94 + trail % 94), 2)
            }
            Some(_) => invalid(1, 1),
        },
        _ => invalid(1, 0),
    }
}

/// The character's codepoint, or `None` when CRuby has none for it.
#[must_use]
pub fn to_unicode(encoding: u8, jis: Jis) -> Option<u32> {
    let code = match jis {
        Jis::Ascii(b) => return Some(u32::from(b)),
        Jis::Kana(b @ 0xa1..=0xdf) => return Some(0xff61 + u32::from(b - 0xa1)),
        Jis::Kana(_) => return None,
        Jis::Cell(cell) if encoding == WINDOWS_31J => WINDOWS_31J_READ[usize::from(cell)],
        Jis::Cell(cell) => *JIS0208.get(usize::from(cell))?,
        Jis::Supplementary(cell) => JIS0212[usize::from(cell)],
    };
    (code != 0).then_some(u32::from(code))
}

/// The character CRuby writes for `code`, or `None` when it has none.
#[must_use]
pub fn from_unicode(encoding: u8, code: u32) -> Option<Jis> {
    match code {
        0..=0x7f => return Some(Jis::Ascii(code as u8)),
        0xff61..=0xff9f => return Some(Jis::Kana(0xa1 + (code - 0xff61) as u8)),
        _ => {}
    }
    let code = u16::try_from(code).ok()?;
    let table: &[(u16, u16)] = match encoding {
        EUC_JP => &EUC_JP_WRITE,
        SHIFT_JIS => &SHIFT_JIS_WRITE,
        _ => &WINDOWS_31J_WRITE,
    };
    let at = table.binary_search_by_key(&code, |&(c, _)| c).ok()?;
    let cell = table[at].1;
    Some(match usize::from(cell).checked_sub(CELLS) {
        Some(supplementary) if encoding == EUC_JP => Jis::Supplementary(supplementary as u16),
        _ => Jis::Cell(cell),
    })
}

/// The character's bytes in `encoding`, or `None` when it has no spelling
/// there: JIS X 0212 and Windows-31J's rows past 94 outside their own
/// encodings, and EUC-JP's katakana bytes that are no katakana.
#[must_use]
pub fn write(encoding: u8, jis: Jis) -> Option<Vec<u8>> {
    let euc = |cell: u16| [0xa1 + (cell / 94) as u8, 0xa1 + (cell % 94) as u8];
    Some(match (encoding == EUC_JP, jis) {
        (_, Jis::Ascii(b)) => vec![b],
        (_, Jis::Kana(b)) if !(0xa1..=0xdf).contains(&b) => return None,
        (true, Jis::Kana(b)) => vec![0x8e, b],
        (false, Jis::Kana(b)) => vec![b],
        (true, Jis::Cell(cell)) if usize::from(cell) < CELLS => euc(cell).to_vec(),
        (true, Jis::Supplementary(cell)) => {
            let [a, b] = euc(cell);
            vec![0x8f, a, b]
        }
        (false, Jis::Cell(cell)) if usize::from(cell) < WINDOWS_CELLS => {
            let (row, column) = (cell / 94, cell % 94);
            let lead = (row / 2) as u8;
            let trail = (row % 2 * 94 + column) as u8;
            vec![
                lead + if lead < 31 { 0x81 } else { 0xc1 },
                trail + if trail < 63 { 0x40 } else { 0x41 },
            ]
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn through(encoding: u8, code: u32) -> Option<Vec<u8>> {
        write(encoding, from_unicode(encoding, code)?)
    }

    fn back(encoding: u8, bytes: &[u8]) -> Option<u32> {
        match read(encoding, bytes, 0) {
            Read::Char(jis, len) if len == bytes.len() => to_unicode(encoding, jis),
            _ => None,
        }
    }

    // The issue's examples, measured on ruby 4.0.7.
    #[test]
    fn writes_as_cruby_does() {
        assert_eq!(through(EUC_JP, 0x2192), Some(vec![0xa2, 0xaa]));
        assert_eq!(through(EUC_JP, 0x30a2), Some(vec![0xa5, 0xa2]));
        assert_eq!(through(SHIFT_JIS, 0x2192), Some(vec![0x81, 0xa8]));
        assert_eq!(through(EUC_JP, 0xe9), Some(vec![0x8f, 0xab, 0xb1]));
        assert_eq!(through(SHIFT_JIS, 0xe9), None);
        assert_eq!(through(WINDOWS_31J, 0xff5e), Some(vec![0x81, 0x60]));
        assert_eq!(through(SHIFT_JIS, 0xff5e), None);
        assert_eq!(through(EUC_JP, 0x2015), Some(vec![0xa1, 0xbd]));
        assert_eq!(through(SHIFT_JIS, 0xff71), Some(vec![0xb1]));
        assert_eq!(through(EUC_JP, 0xa5), None);
    }

    #[test]
    fn reads_as_cruby_does() {
        assert_eq!(back(EUC_JP, &[0xa2, 0xaa]), Some(0x2192));
        assert_eq!(back(EUC_JP, &[0xa9, 0xa1]), None);
        assert_eq!(back(EUC_JP, &[0x8f, 0xb0, 0xa1]), Some(0x4e02));
        assert_eq!(back(SHIFT_JIS, &[0x81, 0x60]), Some(0x301c));
        assert_eq!(back(WINDOWS_31J, &[0x81, 0x60]), Some(0xff5e));
        assert_eq!(back(WINDOWS_31J, &[0xf0, 0x40]), Some(0xe000));
        assert_eq!(back(SHIFT_JIS, &[0xf0, 0x40]), None);
        assert_eq!(
            read(EUC_JP, &[0xa1, 0x41], 0),
            Read::Invalid {
                error: 1,
                readagain: 1
            }
        );
        assert_eq!(
            read(EUC_JP, &[0x8f, 0xa1, 0x41], 0),
            Read::Invalid {
                error: 2,
                readagain: 1
            }
        );
        assert_eq!(read(EUC_JP, &[0x8f, 0xa1], 0), Read::Incomplete(2));
        assert_eq!(
            read(SHIFT_JIS, &[0x80], 0),
            Read::Invalid {
                error: 1,
                readagain: 0
            }
        );
    }

    #[test]
    fn every_written_cell_reads_back_but_the_one_way_ones() {
        for (encoding, table, one_way) in [
            (EUC_JP, &EUC_JP_WRITE[..], jis_table::EUC_JP_ONE_WAY),
            (
                SHIFT_JIS,
                &SHIFT_JIS_WRITE[..],
                jis_table::SHIFT_JIS_ONE_WAY,
            ),
            (
                WINDOWS_31J,
                &WINDOWS_31J_WRITE[..],
                jis_table::WINDOWS_31J_ONE_WAY,
            ),
        ] {
            for &(code, _) in table {
                let bytes = through(encoding, u32::from(code)).unwrap();
                let read = back(encoding, &bytes);
                if !one_way.iter().any(|&(c, _)| c == code) {
                    assert_eq!(read, Some(u32::from(code)), "{encoding} U+{code:04X}");
                }
            }
        }
    }

    #[test]
    fn euc_jp_and_shift_jis_meet_directly() {
        let Read::Char(jis, _) = read(EUC_JP, &[0xa9, 0xa1], 0) else {
            panic!()
        };
        assert_eq!(write(SHIFT_JIS, jis), Some(vec![0x85, 0x40]));
        let Read::Char(jis, _) = read(SHIFT_JIS, &[0xf0, 0x40], 0) else {
            panic!()
        };
        assert_eq!(write(EUC_JP, jis), None);
    }
}
