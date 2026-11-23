//! The CJK multibyte encodings for transcoding (#796): GBK, GB18030, GB2312
//! and GB12345; Big5, Big5-HKSCS, Big5-UAO, CP950 and CP951; CP949; and
//! EUC-JIS-2004 and eucJP-ms. Reading a character is arithmetic, which bytes
//! lead and which follow; what it means in Unicode is a table, generated from
//! CRuby into `cjk_table.rs` by `scripts/cjk-table.rb`, which also checks
//! everything this file assumes about the bytes against CRuby.
//!
//! A character past ASCII is a *cell*, numbered by its bytes. A double-byte
//! encoding's is its lead's place among the leads times the number of trails,
//! plus its trail's place among them. An EUC encoding's are the 94 after
//! 0x8E, then the 94 rows of 94 its two bytes spell, then the 94 rows after
//! 0x8F, as EUC-JP's are. GB18030 is GBK's shape with four bytes besides,
//! which read as runs of consecutive codepoints rather than a plane.
//!
//! A plane holds each cell's codepoint as a `u16`: 0 for none, and for a
//! character past the BMP, which only Big5-HKSCS and EUC-JIS-2004 read, a
//! surrogate, which no cell reads as, numbering it in the plane's list of
//! them. Writing looks, in order, at the characters written as one byte, at
//! those written as a cell that reads as another, at the cells sorted by
//! what they read as, and for GB18030 at its runs; the script checks that
//! this agrees with CRuby for every codepoint.

use crate::cjk_table as table;

pub const BIG5: u8 = 12;
pub const BIG5_HKSCS: u8 = 13;
pub const BIG5_UAO: u8 = 14;
pub const CP949: u8 = 16;
pub const GB18030: u8 = 20;
pub const GBK: u8 = 21;
pub const CP950: u8 = 76;
pub const CP951: u8 = 77;
pub const EUCJP_MS: u8 = 80;
pub const EUC_JIS_2004: u8 = 82;
pub const GB2312: u8 = 83;
pub const GB12345: u8 = 84;

/// Where a plane numbers its characters past the BMP.
const ASTRAL: u16 = 0xd800;

/// How an encoding's bytes make a character.
#[derive(Clone, Copy)]
enum Shape {
    /// A lead and a trail, each from its ranges.
    Double {
        leads: (u8, u8),
        trails: &'static [(u8, u8)],
    },
    /// GBK's two bytes, and four: a lead, a digit, a lead and a digit.
    Gb18030,
    /// EUC-JP's: 0x8E and a byte, two bytes from 0xA1 to 0xFE, or 0x8F and
    /// two more.
    Euc,
}

const GBK_TRAILS: &[(u8, u8)] = &[(0x40, 0x7e), (0x80, 0xfe)];
const GB_LEADS: (u8, u8) = (0x81, 0xfe);
const EUC_BYTES: (u8, u8) = (0xa1, 0xfe);
const BIG5_TRAILS: &[(u8, u8)] = &[(0x40, 0x7e), (0xa1, 0xfe)];
const CP949_TRAILS: &[(u8, u8)] = &[(0x41, 0x5a), (0x61, 0x7a), (0x81, 0xfe)];

const fn width(trails: &[(u8, u8)]) -> usize {
    let (mut n, mut i) = (0, 0);
    while i < trails.len() {
        n += (trails[i].1 - trails[i].0) as usize + 1;
        i += 1;
    }
    n
}

const GBK_CELLS: usize = 126 * width(GBK_TRAILS);
const GB2312_CELLS: usize = 94 * 94;
const BIG5_CELLS: usize = 126 * width(BIG5_TRAILS);
const CP949_CELLS: usize = 126 * width(CP949_TRAILS);
const EUC_CELLS: usize = 94 + 2 * 94 * 94;

/// An encoding's tables.
struct Table {
    shape: Shape,
    /// Bytes past ASCII that are a character alone, and what each reads as.
    single_reads: &'static [(u8, u32)],
    /// What each cell reads as.
    cells: &'static [u16],
    /// The characters past the BMP the cells read as, numbered from
    /// [`ASTRAL`].
    astral: &'static [u32],
    /// The cells CRuby writes as the character they read as, sorted by it.
    written: &'static [u16],
    /// Characters written as a cell that reads as another, sorted.
    one_way: &'static [(u32, u16)],
    /// Characters written as one byte, sorted.
    single_writes: &'static [(u32, u8)],
}

impl Table {
    fn code(&self, cell: usize) -> Option<u32> {
        match self.cells[cell] {
            0 => None,
            v if v & 0xf800 == ASTRAL => Some(self.astral[usize::from(v - ASTRAL)]),
            v => Some(u32::from(v)),
        }
    }
}

/// Decodes one UTF-8 character of `bytes` at `at`, as a plane's text has it.
const fn utf8(bytes: &[u8], at: usize) -> (u32, usize) {
    let lead = bytes[at] as u32;
    let (mut code, len) = match lead {
        0x00..=0x7f => (lead, 1),
        0xc0..=0xdf => (lead & 0x1f, 2),
        0xe0..=0xef => (lead & 0x0f, 3),
        _ => (lead & 0x07, 4),
    };
    let mut i = 1;
    while i < len {
        code = (code << 6) | (bytes[at + i] as u32 & 0x3f);
        i += 1;
    }
    (code, len)
}

/// A plane's text, one character per cell and NUL for none, as the `u16`s
/// [`Table::cells`] holds. Evaluated at compile time; a plane of the wrong
/// size does not compile.
const fn plane<const N: usize>(text: &str) -> [u16; N] {
    let bytes = text.as_bytes();
    let mut out = [0u16; N];
    let (mut at, mut cell, mut astral) = (0, 0, 0);
    while at < bytes.len() {
        let (code, len) = utf8(bytes, at);
        out[cell] = if code > 0xffff {
            astral += 1;
            ASTRAL + astral - 1
        } else {
            assert!(code & 0xf800 != 0xd800, "a plane reads no surrogate");
            code as u16
        };
        cell += 1;
        at += len;
    }
    assert!(cell == N, "a plane has the wrong number of cells");
    out
}

/// The characters past the BMP in a plane's text, in cell order.
const fn astral<const N: usize>(text: &str) -> [u32; N] {
    let bytes = text.as_bytes();
    let mut out = [0u32; N];
    let (mut at, mut n) = (0, 0);
    while at < bytes.len() {
        let (code, len) = utf8(bytes, at);
        if code > 0xffff {
            out[n] = code;
            n += 1;
        }
        at += len;
    }
    assert!(n == N, "the table's count of astral characters");
    out
}

/// The cells that read as a character, but those in `unchosen`, sorted by
/// what they read as. Evaluated at compile time, from the plane.
const fn writer<const N: usize>(cells: &[u16], astral: &[u32], unchosen: &[u16]) -> [u16; N] {
    // A cell plus one by the BMP character it reads as.
    let mut by_code = [0u16; 0x1_0000];
    let mut out = [0u16; N];
    let (mut cell, mut skip, mut far) = (0, 0, 0);
    // The cells reading past the BMP go at the end, in cell order for now.
    while cell < cells.len() {
        let code = cells[cell];
        if skip < unchosen.len() && unchosen[skip] as usize == cell {
            skip += 1;
        } else if code & 0xf800 == ASTRAL {
            far += 1;
            out[N - far] = cell as u16;
        } else if code != 0 {
            assert!(by_code[code as usize] == 0, "two cells write one character");
            by_code[code as usize] = cell as u16 + 1;
        }
        cell += 1;
    }
    assert!(skip == unchosen.len(), "the unchosen cells are sorted");
    let (mut code, mut n) = (0, 0);
    while code < by_code.len() {
        if by_code[code] != 0 {
            out[n] = by_code[code] - 1;
            n += 1;
        }
        code += 1;
    }
    assert!(n + far == N, "the table's count of written characters");
    // Sorts the far ones by what they read as.
    let mut i = n + 1;
    while i < N {
        let mut j = i;
        while j > n
            && astral[(cells[out[j - 1] as usize] - ASTRAL) as usize]
                > astral[(cells[out[j] as usize] - ASTRAL) as usize]
        {
            let held = out[j];
            out[j] = out[j - 1];
            out[j - 1] = held;
            j -= 1;
        }
        i += 1;
    }
    out
}

/// One encoding's [`Table`], built at compile time from its module of
/// `cjk_table.rs`.
macro_rules! table {
    ($module:ident, $shape:expr, $cells:expr) => {{
        static CELLS: [u16; $cells] = plane(table::$module::PLANE);
        static FAR: [u32; table::$module::ASTRAL] = astral(table::$module::PLANE);
        static WRITTEN: [u16; table::$module::WRITTEN] =
            writer(&CELLS, &FAR, table::$module::UNCHOSEN);
        static TABLE: Table = Table {
            shape: $shape,
            single_reads: table::$module::SINGLE_READS,
            cells: &CELLS,
            astral: &FAR,
            written: &WRITTEN,
            one_way: table::$module::ONE_WAY,
            single_writes: table::$module::SINGLE_WRITES,
        };
        &TABLE
    }};
}

const GBK_SHAPE: Shape = Shape::Double {
    leads: GB_LEADS,
    trails: GBK_TRAILS,
};
const GB2312_SHAPE: Shape = Shape::Double {
    leads: EUC_BYTES,
    trails: &[EUC_BYTES],
};
const BIG5_SHAPE: Shape = Shape::Double {
    leads: GB_LEADS,
    trails: BIG5_TRAILS,
};
const CP949_SHAPE: Shape = Shape::Double {
    leads: GB_LEADS,
    trails: CP949_TRAILS,
};

fn table_of(encoding: u8) -> &'static Table {
    match encoding {
        GBK => table!(gbk, GBK_SHAPE, GBK_CELLS),
        GB18030 => table!(gb18030, Shape::Gb18030, GBK_CELLS),
        GB2312 => table!(gb2312, GB2312_SHAPE, GB2312_CELLS),
        GB12345 => table!(gb12345, GB2312_SHAPE, GB2312_CELLS),
        BIG5 => table!(big5, BIG5_SHAPE, BIG5_CELLS),
        BIG5_HKSCS => table!(big5_hkscs, BIG5_SHAPE, BIG5_CELLS),
        BIG5_UAO => table!(big5_uao, BIG5_SHAPE, BIG5_CELLS),
        CP950 => table!(cp950, BIG5_SHAPE, BIG5_CELLS),
        CP951 => table!(cp951, BIG5_SHAPE, BIG5_CELLS),
        CP949 => table!(cp949, CP949_SHAPE, CP949_CELLS),
        EUC_JIS_2004 => table!(euc_jis_2004, Shape::Euc, EUC_CELLS),
        EUCJP_MS => table!(eucjp_ms, Shape::Euc, EUC_CELLS),
        _ => unreachable!("encoding {encoding} is not a CJK one"),
    }
}

/// Whether `encoding` is one of these.
#[must_use]
pub fn supported(encoding: u8) -> bool {
    matches!(
        encoding,
        GBK | GB18030
            | GB2312
            | GB12345
            | BIG5
            | BIG5_HKSCS
            | BIG5_UAO
            | CP950
            | CP951
            | CP949
            | EUC_JIS_2004
            | EUCJP_MS
    )
}

/// One character read, or why there is none — CRuby's shapes: a bad lead
/// byte is the error alone, and a bad byte after the first is given back to
/// be read again. A character is its codepoint, or `None` for one CRuby has
/// no character for.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    Char(Option<u32>, usize),
    Invalid { error: usize, readagain: usize },
    Incomplete(usize),
}

const fn within((low, high): (u8, u8), byte: u8) -> bool {
    low <= byte && byte <= high
}

/// `byte`'s place among `trails`, if it is one.
fn trail(trails: &[(u8, u8)], byte: u8) -> Option<usize> {
    let mut before = 0;
    for &(low, high) in trails {
        if within((low, high), byte) {
            return Some(before + usize::from(byte - low));
        }
        before += usize::from(high - low) + 1;
    }
    None
}

fn invalid(error: usize, readagain: usize) -> Read {
    Read::Invalid { error, readagain }
}

/// Reads the character at `at`.
#[must_use]
pub fn read(encoding: u8, bytes: &[u8], at: usize) -> Read {
    let table = table_of(encoding);
    let rest = &bytes[at..];
    let lead = rest[0];
    if lead < 0x80 {
        return Read::Char(Some(u32::from(lead)), 1);
    }
    if let Some(&(_, code)) = table.single_reads.iter().find(|&&(b, _)| b == lead) {
        return Read::Char(Some(code), 1);
    }
    let (leads, trails) = match table.shape {
        Shape::Euc => return read_euc(table, rest),
        Shape::Gb18030 => (GB_LEADS, GBK_TRAILS),
        Shape::Double { leads, trails } => (leads, trails),
    };
    if !within(leads, lead) {
        return invalid(1, 0);
    }
    let Some(&second) = rest.get(1) else {
        return Read::Incomplete(1);
    };
    if let Some(place) = trail(trails, second) {
        let cell = usize::from(lead - leads.0) * width(trails) + place;
        return Read::Char(table.code(cell), 2);
    }
    let digit = |b: u8| b.is_ascii_digit();
    if !matches!(table.shape, Shape::Gb18030) || !digit(second) {
        return invalid(1, 1);
    }
    let Some(&third) = rest.get(2) else {
        return Read::Incomplete(2);
    };
    if !within(GB_LEADS, third) {
        return invalid(2, 1);
    }
    let Some(&fourth) = rest.get(3) else {
        return Read::Incomplete(3);
    };
    if !digit(fourth) {
        return invalid(3, 1);
    }
    let index = ((u32::from(lead - 0x81) * 10 + u32::from(second - b'0')) * 126
        + u32::from(third - 0x81))
        * 10
        + u32::from(fourth - b'0');
    Read::Char(four_to_unicode(index), 4)
}

fn read_euc(table: &Table, rest: &[u8]) -> Read {
    let lead = rest[0];
    if lead != 0x8e && lead != 0x8f && !within(EUC_BYTES, lead) {
        return invalid(1, 0);
    }
    let Some(&second) = rest.get(1) else {
        return Read::Incomplete(1);
    };
    if !within(EUC_BYTES, second) {
        return invalid(1, 1);
    }
    let row = |a: u8, b: u8| usize::from(a - 0xa1) * 94 + usize::from(b - 0xa1);
    match lead {
        0x8e => Read::Char(table.code(usize::from(second - 0xa1)), 2),
        0x8f => {
            let Some(&third) = rest.get(2) else {
                return Read::Incomplete(2);
            };
            if !within(EUC_BYTES, third) {
                return invalid(2, 1);
            }
            Read::Char(table.code(94 + 94 * 94 + row(second, third)), 3)
        }
        _ => Read::Char(table.code(94 + row(lead, second)), 2),
    }
}

/// What `scrub` makes of the bytes at `at`: a character, as many bytes
/// as are one broken sequence, or bytes the end of the input cuts short.
#[derive(Debug, PartialEq, Eq)]
pub enum Scrubbed {
    Char(usize),
    Invalid(usize),
    Incomplete(usize),
}

/// Reads the bytes at `at` as `scrub` does, which is not as converting them
/// does: CRuby's `scrub` bounds a character by the encoding's own rules,
/// `rb_enc_precise_mbclen`'s, which the regex engine measured (#388), and
/// those are not always the converter's — `"\x81\x40"` is a character to
/// the Big5 converter and broken in Big5, and CP949's 0x80 is a character
/// alone that its converter refuses. A broken sequence is one byte, or in an
/// encoding whose characters run past two bytes the longest prefix of what
/// is left, short of all of it, that more bytes would finish, as
/// `enc_str_scrub` takes it. Measured.
#[must_use]
pub fn scrubbed(encoding: u8, bytes: &[u8], at: usize) -> Scrubbed {
    let Some(engine) = crate::strings::multibyte(encoding) else {
        unreachable!("the regex engine reads every CJK encoding");
    };
    let rest = &bytes[at..];
    if let Some(len) = engine.char_len(rest) {
        return Scrubbed::Char(len);
    }
    if engine.is_cut_short(rest) {
        return Scrubbed::Incomplete(rest.len());
    }
    let longest = match table_of(encoding).shape {
        Shape::Gb18030 => 4,
        Shape::Euc => 3,
        Shape::Double { .. } => 2,
    };
    let mut len = rest.len().min(longest);
    if len <= 2 {
        return Scrubbed::Invalid(1);
    }
    len -= 1;
    while len > 1 && !engine.is_cut_short(&rest[..len]) {
        len -= 1;
    }
    Scrubbed::Invalid(len)
}

/// What GB18030's four bytes numbered `index` read as.
fn four_to_unicode(index: u32) -> Option<u32> {
    let runs = table::gb18030::FOUR;
    let run = runs[..runs.partition_point(|&(first, ..)| first <= index)].last()?;
    (index < run.0 + run.2).then(|| run.1 + index - run.0)
}

/// The number of the four bytes GB18030 writes `code` as, if any.
fn four_from_unicode(code: u32) -> Option<u32> {
    let runs = table::gb18030::FOUR;
    let run = runs[..runs.partition_point(|&(_, first, _)| first <= code)].last()?;
    (code < run.1 + run.2).then(|| run.0 + code - run.1)
}

/// The bytes of `cell`.
fn spell(shape: Shape, cell: usize) -> Vec<u8> {
    let (leads, trails) = match shape {
        Shape::Euc => {
            let pair = |cell: usize| [0xa1 + (cell / 94) as u8, 0xa1 + (cell % 94) as u8];
            return match cell {
                0..94 => vec![0x8e, 0xa1 + cell as u8],
                _ if cell < 94 + 94 * 94 => pair(cell - 94).to_vec(),
                _ => {
                    let [a, b] = pair(cell - 94 - 94 * 94);
                    vec![0x8f, a, b]
                }
            };
        }
        Shape::Gb18030 => (GB_LEADS, GBK_TRAILS),
        Shape::Double { leads, trails } => (leads, trails),
    };
    let (lead, mut place) = (cell / width(trails), cell % width(trails));
    let mut trail = 0;
    for &(low, high) in trails {
        let span = usize::from(high - low) + 1;
        if place < span {
            trail = low + place as u8;
            break;
        }
        place -= span;
    }
    vec![leads.0 + lead as u8, trail]
}

/// `code`'s bytes in `encoding`, or `None` when CRuby writes no character
/// for it.
#[must_use]
pub fn encode(encoding: u8, code: u32) -> Option<Vec<u8>> {
    if code < 0x80 {
        return Some(vec![code as u8]);
    }
    let table = table_of(encoding);
    if let Ok(at) = table.single_writes.binary_search_by_key(&code, |&(c, _)| c) {
        return Some(vec![table.single_writes[at].1]);
    }
    if let Ok(at) = table.one_way.binary_search_by_key(&code, |&(c, _)| c) {
        return Some(spell(table.shape, usize::from(table.one_way[at].1)));
    }
    let found = table
        .written
        .binary_search_by_key(&code, |&cell| table.code(usize::from(cell)).unwrap_or(0));
    if let Ok(at) = found {
        return Some(spell(table.shape, usize::from(table.written[at])));
    }
    if encoding != GB18030 {
        return None;
    }
    let mut index = four_from_unicode(code)?;
    let mut bytes = [0u8; 4];
    for (i, base, radix) in [(3, b'0', 10), (2, 0x81, 126), (1, b'0', 10)] {
        bytes[i] = base + (index % radix) as u8;
        index /= radix;
    }
    bytes[0] = 0x81 + index as u8;
    Some(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [u8; 12] = [
        GBK,
        GB18030,
        GB2312,
        GB12345,
        BIG5,
        BIG5_HKSCS,
        BIG5_UAO,
        CP950,
        CP951,
        CP949,
        EUC_JIS_2004,
        EUCJP_MS,
    ];

    fn back(encoding: u8, bytes: &[u8]) -> Option<u32> {
        match read(encoding, bytes, 0) {
            Read::Char(code, len) if len == bytes.len() => code,
            _ => None,
        }
    }

    #[test]
    fn the_constants_are_the_encodings_they_name() {
        let named = |name: &str| crate::strings::index_named(name);
        for (name, encoding) in [
            ("GBK", GBK),
            ("GB18030", GB18030),
            ("GB2312", GB2312),
            ("GB12345", GB12345),
            ("Big5", BIG5),
            ("Big5-HKSCS", BIG5_HKSCS),
            ("Big5-UAO", BIG5_UAO),
            ("CP950", CP950),
            ("CP951", CP951),
            ("CP949", CP949),
            ("EUC-JIS-2004", EUC_JIS_2004),
            ("eucJP-ms", EUCJP_MS),
        ] {
            assert_eq!(named(name), Some(encoding), "{name}");
        }
    }

    // Measured on ruby 4.0.9 (#796).
    #[test]
    fn writes_and_reads_as_cruby_does() {
        // 一 and é where each has them; CP950 writes é as an e.
        assert_eq!(encode(GBK, 0x4e00), Some(vec![0xd2, 0xbb]));
        assert_eq!(encode(GBK, 0xe9), Some(vec![0xa8, 0xa6]));
        assert_eq!(encode(GBK, 0x20ac), Some(vec![0x80]));
        assert_eq!(back(GBK, &[0x80]), Some(0x20ac));
        assert_eq!(encode(GBK, 0x1f600), None);
        assert_eq!(encode(BIG5, 0x4e00), Some(vec![0xa4, 0x40]));
        assert_eq!(encode(BIG5, 0xe9), None);
        assert_eq!(encode(CP950, 0xe9), Some(vec![0x65]));
        assert_eq!(encode(BIG5_HKSCS, 0xe9), Some(vec![0x88, 0x6d]));
        assert_eq!(encode(EUC_JIS_2004, 0xe9), Some(vec![0xa9, 0xdf]));
        assert_eq!(encode(EUCJP_MS, 0xe9), Some(vec![0x8f, 0xab, 0xb1]));
        // GB18030's four bytes, in the BMP and past it.
        assert_eq!(encode(GB18030, 0x80), Some(vec![0x81, 0x30, 0x81, 0x30]));
        assert_eq!(encode(GB18030, 0x1f600), Some(vec![0x94, 0x39, 0xfc, 0x36]));
        assert_eq!(back(GB18030, &[0x94, 0x39, 0xfc, 0x36]), Some(0x1f600));
        assert_eq!(back(GB18030, &[0x84, 0x31, 0xa5, 0x30]), None);
        assert_eq!(read(GB18030, &[0x81, 0x30, 0x81], 0), Read::Incomplete(3));
        assert_eq!(read(GB18030, &[0x81, 0x30, 0x81, 0x41], 0), invalid(3, 1));
        assert_eq!(read(GB18030, &[0x81, 0x30, 0x41], 0), invalid(2, 1));
        assert_eq!(read(GB18030, &[0x80], 0), invalid(1, 0));
        assert_eq!(read(BIG5, &[0xa4, 0x80], 0), invalid(1, 1));
        assert_eq!(read(CP949, &[0x81], 0), Read::Incomplete(1));
        assert_eq!(read(EUCJP_MS, &[0x8f, 0xa1], 0), Read::Incomplete(2));
        assert_eq!(read(EUCJP_MS, &[0x8f, 0xa1, 0x41], 0), invalid(2, 1));
    }

    // Measured on ruby 4.0.9 (#796): `scrub` bounds characters by the
    // encoding, not by its converter.
    #[test]
    fn scrubs_as_cruby_does() {
        assert_eq!(scrubbed(BIG5, b"\x81@a", 0), Scrubbed::Invalid(1));
        assert_eq!(scrubbed(BIG5, b"\xa4@a", 0), Scrubbed::Char(2));
        assert_eq!(scrubbed(CP949, b"\x80", 0), Scrubbed::Char(1));
        assert_eq!(scrubbed(CP949, b"\xb0", 0), Scrubbed::Incomplete(1));
        assert_eq!(scrubbed(GB18030, b"\x810\x81A", 0), Scrubbed::Invalid(3));
        assert_eq!(
            scrubbed(EUC_JIS_2004, b"\x8f\xa1A", 0),
            Scrubbed::Invalid(2)
        );
        assert_eq!(scrubbed(EUCJP_MS, b"\xff", 0), Scrubbed::Invalid(1));
    }

    /// Every character an encoding writes as a cell reads back as itself
    /// but the one-way ones, and every cell spelled reads as the cell. Under
    /// miri, where a character costs milliseconds, every 97th.
    #[test]
    fn every_written_cell_reads_back() {
        let step = if cfg!(miri) { 97 } else { 1 };
        for encoding in ALL {
            let table = table_of(encoding);
            for &cell in table.written.iter().step_by(step) {
                let code = table.code(usize::from(cell)).unwrap();
                let bytes = encode(encoding, code).unwrap();
                assert_eq!(bytes, spell(table.shape, usize::from(cell)));
                assert_eq!(
                    back(encoding, &bytes),
                    Some(code),
                    "{encoding}: U+{code:04X}"
                );
            }
            for &(code, cell) in table.one_way.iter().step_by(step) {
                assert_eq!(
                    encode(encoding, code),
                    Some(spell(table.shape, usize::from(cell)))
                );
            }
        }
    }

    /// GB18030 writes every codepoint, and reads each back.
    #[test]
    fn gb18030_writes_every_codepoint() {
        let step = if cfg!(miri) { 7919 } else { 1 };
        for code in (0x80..0x11_0000).step_by(step) {
            if (0xd800..0xe000).contains(&code) {
                continue;
            }
            let bytes = encode(GB18030, code).unwrap_or_else(|| panic!("U+{code:04X}"));
            assert_eq!(back(GB18030, &bytes), Some(code), "U+{code:04X}");
        }
    }
}
