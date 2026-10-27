//! How a character is read out of bytes, encoding by encoding (#339).
//!
//! The machine compares `char`s, so every encoding's characters have to be
//! carried as one. UTF-8's are themselves. BINARY's and every other
//! single-byte encoding's are a byte each, carried as the `char` with that
//! number — the character itself only in ISO-8859-1, so what a byte is to the
//! classes and case folds is the encoding's table, never Unicode's (#372).
//!
//! The other multibyte encodings' characters have no Unicode number this
//! engine can compute: that mapping is a table, and it is #378's. They do not
//! need one. Onigmo itself never maps them: a character's code is its bytes
//! read as one big-endian number (`あ` is `0xA4A2` in EUC-JP), a range compares
//! those codes, and the classes and case folds are rules over them. So a
//! two-byte code is carried at `U+F0000` plus the code, and EUC-JP's three-byte
//! `8F xx yy` at `U+100000` plus `xxyy`, both in the supplementary private-use
//! planes. The order of codes survives, so `[ぁ-ん]` still means what it says,
//! and no carried character can be mistaken for one of another kind: ASCII is
//! below `0x80`, a single byte past it is below `0x100`, and the planes are
//! above everything Unicode assigns.
//!
//! GB18030, EUC-TW and Emacs-Mule have codes of three and four bytes too many
//! for those planes (#388), so theirs are carried by rank instead: every
//! character past ASCII in code order, numbered from `U+0080` up and stepping
//! over the surrogates. The order survives that as well. GB18030's 1.6 million
//! four-byte codes are more than there are scalar values; the ones a GB18030
//! character is assigned — those of Unicode's BMP, `81308130` to `8431A439`,
//! and of the planes past it, `90308130` to `E3329A35` — and its two-byte codes
//! are exactly as many as the scalar values past ASCII, so those are carried,
//! and a pattern or subject with any other is refused ([`Encoding::uncarried`]).

use crate::Encoding;
use crate::parse::{Perl, Posix};
use crate::single_byte::Table;

/// Where a two-byte code is carried: `U+F0000 + code`.
const DOUBLE: u32 = 0xF_0000;
/// Where EUC-JP's three-byte `8F xx yy` is carried: `U+100000 + xxyy`.
const TRIPLE: u32 = 0x10_0000;

fn euc_byte(b: u8) -> bool {
    (0xa1..=0xfe).contains(&b)
}

fn sjis_trail(b: u8) -> bool {
    matches!(b, 0x40..=0x7e | 0x80..=0xfc)
}

fn big5_trail(b: u8) -> bool {
    matches!(b, 0x40..=0x7e | 0xa1..=0xfe)
}

fn cp949_trail(b: u8) -> bool {
    matches!(b, 0x41..=0x5a | 0x61..=0x7a | 0x81..=0xfe)
}

fn gbk_trail(b: u8) -> bool {
    matches!(b, 0x40..=0x7e | 0x80..=0xfe)
}

fn gb_lead(b: u8) -> bool {
    (0x81..=0xfe).contains(&b)
}

fn gb_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

fn mule_byte(b: u8) -> bool {
    b >= 0xa0
}

/// One run of an encoding's codes in rank order: the codes whose bytes are
/// one of each position's ranges, in turn. Every code in a run is below every
/// code in the next.
type Run = &'static [&'static [(u8, u8)]];

const ANY_EUC: &[(u8, u8)] = &[(0xa1, 0xfe)];
const ANY_MULE: &[(u8, u8)] = &[(0xa0, 0xff)];
const GB_LEAD: &[(u8, u8)] = &[(0x81, 0xfe)];
const GB_DIGIT: &[(u8, u8)] = &[(0x30, 0x39)];

/// EUC-TW in code order: two bytes, then `8E` and three.
const EUC_TW: &[Run] = &[
    &[ANY_EUC, ANY_EUC],
    &[&[(0x8e, 0x8e)], &[(0xa1, 0xb0)], ANY_EUC, ANY_EUC],
];

/// Emacs-Mule in code order, as measured: each leading byte takes its own
/// count of trails, and `9A` to `9D` their own second byte.
const EMACS_MULE: &[Run] = &[
    &[&[(0x81, 0x8f)], ANY_MULE],
    &[&[(0x90, 0x99)], ANY_MULE, ANY_MULE],
    &[&[(0x9a, 0x9b)], &[(0xe0, 0xef)], ANY_MULE],
    &[&[(0x9c, 0x9c)], &[(0xf0, 0xf4)], ANY_MULE, ANY_MULE],
    &[&[(0x9d, 0x9d)], &[(0xf5, 0xfe)], ANY_MULE, ANY_MULE],
];

/// GB18030's two-byte codes, then its assigned four-byte ones: `81308130` to
/// `8431A439`, and `90308130` to `E3329A35`, each cut into runs at the codes
/// it ends on.
const GB18030: &[Run] = &[
    &[GB_LEAD, &[(0x40, 0x7e), (0x80, 0xfe)]],
    &[&[(0x81, 0x83)], GB_DIGIT, GB_LEAD, GB_DIGIT],
    &[&[(0x84, 0x84)], &[(0x30, 0x30)], GB_LEAD, GB_DIGIT],
    &[&[(0x84, 0x84)], &[(0x31, 0x31)], &[(0x81, 0xa3)], GB_DIGIT],
    &[&[(0x84, 0x84)], &[(0x31, 0x31)], &[(0xa4, 0xa4)], GB_DIGIT],
    &[&[(0x90, 0xe2)], GB_DIGIT, GB_LEAD, GB_DIGIT],
    &[&[(0xe3, 0xe3)], &[(0x30, 0x31)], GB_LEAD, GB_DIGIT],
    &[&[(0xe3, 0xe3)], &[(0x32, 0x32)], &[(0x81, 0x99)], GB_DIGIT],
    &[
        &[(0xe3, 0xe3)],
        &[(0x32, 0x32)],
        &[(0x9a, 0x9a)],
        &[(0x30, 0x35)],
    ],
];

/// How many values one position of a run takes.
fn width(ranges: &[(u8, u8)]) -> u32 {
    ranges.iter().map(|&(lo, hi)| u32::from(hi - lo) + 1).sum()
}

/// A byte's place among its position's values, or `None` when it is not one.
fn place(ranges: &[(u8, u8)], b: u8) -> Option<u32> {
    let mut before = 0;
    for &(lo, hi) in ranges {
        if (lo..=hi).contains(&b) {
            return Some(before + u32::from(b - lo));
        }
        before += u32::from(hi - lo) + 1;
    }
    None
}

/// The byte at `place` among a position's values.
fn at_place(ranges: &[(u8, u8)], mut place: u32) -> u8 {
    for &(lo, hi) in ranges {
        let here = u32::from(hi - lo) + 1;
        if place < here {
            return lo + place as u8;
        }
        place -= here;
    }
    0
}

fn run_size(run: Run) -> u32 {
    run.iter().map(|ranges| width(ranges)).product()
}

/// A character's rank among `runs`, or `None` when no run holds its bytes.
fn rank(runs: &[Run], bytes: &[u8]) -> Option<u32> {
    let mut before = 0;
    for &run in runs {
        if run.len() == bytes.len()
            && let Some(index) = run.iter().zip(bytes).try_fold(0, |index, (ranges, &b)| {
                place(ranges, b).map(|p| index * width(ranges) + p)
            })
        {
            return Some(before + index);
        }
        before += run_size(run);
    }
    None
}

/// The bytes of the character at `rank` among `runs`.
fn unrank(runs: &[Run], mut rank: u32) -> Option<Vec<u8>> {
    for &run in runs {
        let size = run_size(run);
        if rank < size {
            let mut bytes = vec![0; run.len()];
            for (slot, ranges) in bytes.iter_mut().zip(run.iter()).rev() {
                *slot = at_place(ranges, rank % width(ranges));
                rank /= width(ranges);
            }
            return Some(bytes);
        }
        rank -= size;
    }
    None
}

/// The scalar value a rank is carried at: `U+0080` up, past the surrogates.
fn rank_char(rank: u32) -> Option<char> {
    let n = 0x80 + rank;
    char::from_u32(if n >= 0xd800 { n + 0x800 } else { n })
}

fn char_rank(c: char) -> u32 {
    let n = u32::from(c);
    if n >= 0xe000 {
        n - 0x800 - 0x80
    } else {
        n - 0x80
    }
}

impl Encoding {
    /// How the engine reads a multibyte encoding, by its CRuby name: the
    /// variants of UTF-8, EUC-JP and Shift_JIS as those, which they measure
    /// the same as, and the rest by their own boundaries. `None` for a
    /// single-byte encoding, and for CESU-8, which is not read yet.
    #[must_use]
    pub fn multibyte(name: &str) -> Option<Encoding> {
        Some(match name {
            "UTF-8" | "UTF8-MAC" | "UTF8-DoCoMo" | "UTF8-KDDI" | "UTF8-SoftBank" => Encoding::Utf8,
            "EUC-JP" | "eucJP-ms" | "CP51932" | "EUC-JIS-2004" => Encoding::EucJp,
            "Windows-31J" | "Shift_JIS" | "MacJapanese" | "SJIS-DoCoMo" | "SJIS-KDDI"
            | "SJIS-SoftBank" => Encoding::ShiftJis,
            "Big5" | "Big5-HKSCS" | "Big5-UAO" | "CP950" | "CP951" => Encoding::Big5,
            "CP949" => Encoding::Cp949,
            "EUC-KR" | "GB2312" | "GB12345" => Encoding::EucKr,
            "GBK" => Encoding::Gbk,
            "GB18030" => Encoding::Gb18030,
            "EUC-TW" => Encoding::EucTw,
            "Emacs-Mule" | "stateless-ISO-2022-JP" | "stateless-ISO-2022-JP-KDDI" => {
                Encoding::EmacsMule
            }
            _ => return None,
        })
    }

    /// Whether a character past ASCII is carried by its multibyte code, and
    /// can take more than one byte, so that a character boundary has to be
    /// found by reading from somewhere known.
    pub(crate) fn multibyte_coded(self) -> bool {
        !matches!(
            self,
            Encoding::Utf8 | Encoding::Binary | Encoding::Single(_)
        )
    }

    /// The runs a multibyte character's rank is counted over, for the
    /// encodings that carry theirs by rank.
    fn ranked(self) -> Option<&'static [Run]> {
        match self {
            Encoding::Gb18030 => Some(GB18030),
            Encoding::EucTw => Some(EUC_TW),
            Encoding::EmacsMule => Some(EMACS_MULE),
            _ => None,
        }
    }

    /// The length of the character at the start of `bytes`, or `None` when no
    /// character starts there. Measured on ruby 4.0.9, byte by byte, into
    /// `tests/encodings.txt`'s `bytes` lines, which a unit test replays:
    ///
    /// - EUC-JP is ASCII; `8E` and one trail, half-width katakana; `8F` and
    ///   two trails, JIS X 0212; or a lead and a trail — every lead and trail
    ///   `A1`-`FE`.
    /// - Shift_JIS and Windows-31J are ASCII; half-width katakana `A1`-`DF`
    ///   alone; or a lead `81`-`9F` or `E0`-`FC` and a trail `40`-`7E` or
    ///   `80`-`FC`. The trail overlaps ASCII: `"\x82\x61"` is one character,
    ///   and there is no `a` in it.
    /// - The others are as their [`Encoding`] variants say.
    #[must_use]
    pub fn char_len(self, bytes: &[u8]) -> Option<usize> {
        let first = *bytes.first()?;
        let trail = |at: usize, ok: fn(u8) -> bool| bytes.get(at).copied().is_some_and(ok);
        let trails =
            |ok: &[fn(u8) -> bool]| ok.iter().enumerate().all(|(at, &ok)| trail(at + 1, ok));
        match self {
            Encoding::Binary | Encoding::Single(_) => Some(1),
            Encoding::Utf8 => {
                let width = match first {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                let end = width.min(bytes.len());
                std::str::from_utf8(&bytes[..end])
                    .ok()
                    .and_then(|text| text.chars().next())
                    .map(char::len_utf8)
            }
            _ if first < 0x80 => Some(1),
            Encoding::EucJp => match first {
                0x8e => trail(1, euc_byte).then_some(2),
                0x8f => trails(&[euc_byte, euc_byte]).then_some(3),
                0xa1..=0xfe => trail(1, euc_byte).then_some(2),
                _ => None,
            },
            Encoding::ShiftJis => match first {
                0xa1..=0xdf => Some(1),
                0x81..=0x9f | 0xe0..=0xfc => trail(1, sjis_trail).then_some(2),
                _ => None,
            },
            Encoding::Big5 => (euc_byte(first) && trail(1, big5_trail)).then_some(2),
            Encoding::Cp949 => match first {
                0x80 => Some(1),
                0x81..=0xfe => trail(1, cp949_trail).then_some(2),
                _ => None,
            },
            Encoding::EucKr => (euc_byte(first) && trail(1, euc_byte)).then_some(2),
            Encoding::Gbk => match first {
                0x80 => Some(1),
                0x81..=0xfe => trail(1, gbk_trail).then_some(2),
                _ => None,
            },
            Encoding::Gb18030 if !gb_lead(first) => None,
            Encoding::Gb18030 if trail(1, gb_digit) => {
                trails(&[gb_digit, gb_lead, gb_digit]).then_some(4)
            }
            Encoding::Gb18030 => trail(1, gbk_trail).then_some(2),
            Encoding::EucTw => match first {
                0x8e => trails(&[|b| (0xa1..=0xb0).contains(&b), euc_byte, euc_byte]).then_some(4),
                0xa1..=0xfe => trail(1, euc_byte).then_some(2),
                _ => None,
            },
            Encoding::EmacsMule => match first {
                0x81..=0x8f => trail(1, mule_byte).then_some(2),
                0x90..=0x99 => trails(&[mule_byte, mule_byte]).then_some(3),
                0x9a..=0x9b => trails(&[|b| (0xe0..=0xef).contains(&b), mule_byte]).then_some(3),
                0x9c => {
                    trails(&[|b| (0xf0..=0xf4).contains(&b), mule_byte, mule_byte]).then_some(4)
                }
                0x9d => {
                    trails(&[|b| (0xf5..=0xfe).contains(&b), mule_byte, mule_byte]).then_some(4)
                }
                _ => None,
            },
        }
    }

    /// Whether `bytes` holds a character this engine cannot carry: a GB18030
    /// four-byte code no character is assigned, `8431A530` to `8F39FE39` or
    /// past `E3329A35`. A pattern with one is refused, and so is a match
    /// against a subject with one, rather than read as some other character.
    ///
    /// ponytail: refusing is the ceiling here. A `char` cannot hold the 1.6
    /// million codes GB18030's bytes can spell; carrying them all means the
    /// machine comparing `u32` codes instead of `char`s.
    #[must_use]
    pub fn uncarried(self, bytes: &[u8]) -> bool {
        if self != Encoding::Gb18030 {
            return false;
        }
        let mut at = 0;
        while at < bytes.len() {
            let len = self.char_len(&bytes[at..]).unwrap_or(1);
            if len == 4 && rank(GB18030, &bytes[at..at + 4]).is_none() {
                return true;
            }
            at += len;
        }
        false
    }

    /// The width of the character at `at`, a broken byte counting as one:
    /// how far a caller steps to reach the next place a match may start.
    #[must_use]
    pub fn char_width(self, bytes: &[u8], at: usize) -> usize {
        bytes
            .get(at..)
            .and_then(|rest| self.char_len(rest))
            .unwrap_or(1)
    }

    /// The character at the start of `bytes` and how many bytes it takes. A
    /// byte that starts no character reads as one that matches nothing a
    /// pattern can spell; the VM refuses a broken subject before matching, so
    /// only a caller of this crate's own API can hand one over. In GB18030,
    /// where every scalar value carries a character, so does a code
    /// [`Encoding::uncarried`] refuses before matching.
    pub(crate) fn decode(self, bytes: &[u8]) -> Option<(char, usize)> {
        let first = *bytes.first()?;
        let Some(len) = self.char_len(bytes) else {
            return Some(('\u{fffd}', 1));
        };
        if let Some(runs) = self.ranked().filter(|_| len > 1) {
            let c = rank(runs, &bytes[..len]).and_then(rank_char);
            return Some((c.unwrap_or('\u{fffd}'), len));
        }
        let c = match (self, len) {
            (Encoding::Utf8, _) => std::str::from_utf8(&bytes[..len])
                .ok()
                .and_then(|text| text.chars().next())
                .unwrap_or('\u{fffd}'),
            (_, 1) => char::from(first),
            (_, 2) => carried(DOUBLE + (u32::from(first) << 8 | u32::from(bytes[1]))),
            _ => carried(TRIPLE + (u32::from(bytes[1]) << 8 | u32::from(bytes[2]))),
        };
        Some((c, len))
    }

    /// The bytes no trail can be, which so always start a character.
    fn starts_only(self, b: u8) -> bool {
        match self {
            Encoding::EucJp => b < 0x80,
            Encoding::ShiftJis => !(0x40..=0xfc).contains(&b),
            Encoding::Big5 => !big5_trail(b),
            Encoding::Cp949 => !cp949_trail(b),
            Encoding::EucKr | Encoding::EucTw => !euc_byte(b),
            Encoding::Gbk => !gbk_trail(b),
            Encoding::Gb18030 => !(gb_digit(b) || gbk_trail(b)),
            Encoding::EmacsMule => !mule_byte(b),
            Encoding::Utf8 | Encoding::Binary | Encoding::Single(_) => true,
        }
    }

    /// Whether a character starts at byte `at` of `bytes`.
    ///
    /// UTF-8 can tell from the byte itself. The other multibyte encodings
    /// cannot — a trail byte is also a lead, or an ASCII letter — so this
    /// reads forward from the last byte that can only start a character, or
    /// the start.
    ///
    /// ponytail: a subject that is one long run of multibyte characters reads
    /// from its start each time, which makes a lookbehind over it quadratic,
    /// and a search for `\b` or `\B` too, since `prev_char` steps back this
    /// way at every place a match may start. Keep a boundary table per
    /// subject, or the previous start in the search loop, if that is ever the
    /// hot path.
    #[must_use]
    pub fn is_char_boundary(self, bytes: &[u8], at: usize) -> bool {
        match self {
            Encoding::Binary | Encoding::Single(_) => true,
            Encoding::Utf8 => bytes.get(at).is_none_or(|&b| b & 0xc0 != 0x80),
            _ => at >= bytes.len() || self.char_start_before(bytes, at + 1) == at,
        }
    }

    /// Where the character holding byte `at - 1` starts: the start of the
    /// character before `at`, for a `at` that is itself a boundary.
    pub(crate) fn char_start_before(self, bytes: &[u8], at: usize) -> usize {
        if !self.multibyte_coded() {
            let mut from = at.saturating_sub(1);
            while from > 0 && !self.is_char_boundary(bytes, from) {
                from -= 1;
            }
            return from;
        }
        // Back to a byte that can only start a character, then forward.
        let mut safe = at.saturating_sub(1);
        while safe > 0 && !self.starts_only(bytes[safe - 1]) {
            safe -= 1;
        }
        let mut from = safe;
        loop {
            let next = from + self.char_width(bytes, from);
            if next >= at {
                return from;
            }
            from = next;
        }
    }

    /// A pattern's source, from the bytes it is written in, as the `char`s the
    /// parser reads; `None` when it holds a byte that starts no character.
    #[must_use]
    pub fn read_source(self, bytes: &[u8]) -> Option<String> {
        let mut text = String::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            self.char_len(&bytes[at..])?;
            let (c, len) = self.decode(&bytes[at..])?;
            text.push(c);
            at += len;
        }
        Some(text)
    }

    /// A character's code, as Onigmo numbers it: a codepoint in UTF-8, the
    /// byte in a single-byte encoding, and the bytes as one big-endian number
    /// in the multibyte ones. What `Regexp#inspect` escapes one as.
    #[must_use]
    pub fn code(self, c: char) -> u32 {
        let n = u32::from(c);
        if !self.multibyte_coded() || n < 0x80 {
            return n;
        }
        if let Some(runs) = self.ranked() {
            return unrank(runs, char_rank(c)).map_or(n, |bytes| {
                bytes.iter().fold(0, |code, &b| code << 8 | u32::from(b))
            });
        }
        if n < DOUBLE {
            n
        } else if n >= TRIPLE {
            0x8F_0000 | (n - TRIPLE)
        } else {
            n - DOUBLE
        }
    }

    /// How the engine reads a single-byte encoding, by its CRuby name: as
    /// BINARY when Onigmo classes and folds nothing past ASCII in it — IBM437,
    /// macRoman and the other code pages — else by its table. `None` for an
    /// encoding that is not single-byte.
    #[must_use]
    pub fn single_byte(name: &str) -> Option<Encoding> {
        crate::single_byte::reading(name)
    }

    /// A character's bytes in this encoding: what `Regexp#to_s` writes back.
    #[must_use]
    pub fn char_bytes(self, c: char) -> Vec<u8> {
        if self == Encoding::Utf8 {
            return c.to_string().into_bytes();
        }
        let code = self.code(c);
        let used = match code {
            0..=0xff => 1,
            0x100..=0xffff => 2,
            0x1_0000..=0xff_ffff => 3,
            _ => 4,
        };
        code.to_be_bytes()[4 - used..].to_vec()
    }

    /// The character a `\x` escape's bytes spell, for the multibyte encodings:
    /// `\xa4\xa2` is `あ` in EUC-JP. `None` when they are not one character.
    pub(crate) fn spelled_by(self, bytes: &[u8]) -> Option<char> {
        (self.char_len(bytes)? == bytes.len())
            .then(|| self.decode(bytes).map(|(c, _)| c))
            .flatten()
    }

    /// How many bytes a character that starts with `prefix` takes, for the
    /// multibyte encodings, as far as its first bytes tell; `None` for a byte
    /// that leads nothing. GB18030's second byte says whether it is two or
    /// four.
    pub(crate) fn lead_len(self, prefix: &[u8]) -> Option<usize> {
        let lead = *prefix.first()?;
        match (self, lead) {
            (_, 0x00..=0x7f) => Some(1),
            (Encoding::EucJp, 0x8f) => Some(3),
            (Encoding::EucJp, 0x8e | 0xa1..=0xfe) => Some(2),
            (Encoding::ShiftJis, 0xa1..=0xdf) => Some(1),
            (Encoding::ShiftJis, 0x81..=0x9f | 0xe0..=0xfc) => Some(2),
            (Encoding::Big5 | Encoding::EucKr, 0xa1..=0xfe) => Some(2),
            (Encoding::Cp949 | Encoding::Gbk, 0x80) => Some(1),
            (Encoding::Cp949 | Encoding::Gbk, 0x81..=0xfe) => Some(2),
            (Encoding::Gb18030, 0x81..=0xfe) => {
                Some(if prefix.get(1).copied().is_some_and(gb_digit) {
                    4
                } else {
                    2
                })
            }
            (Encoding::EucTw, 0x8e) => Some(4),
            (Encoding::EucTw, 0xa1..=0xfe) => Some(2),
            (Encoding::EmacsMule, 0x81..=0x8f) => Some(2),
            (Encoding::EmacsMule, 0x90..=0x9b) => Some(3),
            (Encoding::EmacsMule, 0x9c..=0x9d) => Some(4),
            _ => None,
        }
    }

    /// Whether `c` is GBK's or CP949's `80`, a character alone that Onigmo
    /// puts in no class.
    pub(crate) fn lone_byte(self, c: char) -> bool {
        matches!(self, Encoding::Gbk | Encoding::Cp949) && c == '\u{80}'
    }

    /// The other case of a multibyte character, by Onigmo's arithmetic: the
    /// full-width Latin letters, Greek and Cyrillic, each a block a fixed
    /// distance from its other case. Shift_JIS's Cyrillic lower case skips
    /// `847F`, which is no trail, so its second half is one further away.
    /// Measured over every two-byte code, both encodings, ruby 4.0.7. The
    /// other multibyte encodings fold no case past ASCII.
    pub(crate) fn other_case(self, c: char) -> Option<char> {
        const EUC_JP: &[(u32, u32, u32)] = &[
            (0xa3c1, 0xa3da, 0x20),
            (0xa6a1, 0xa6b8, 0x20),
            (0xa7a1, 0xa7c1, 0x30),
        ];
        const SHIFT_JIS: &[(u32, u32, u32)] = &[
            (0x8260, 0x8279, 0x21),
            (0x839f, 0x83b6, 0x20),
            (0x8440, 0x844e, 0x30),
            (0x844f, 0x8460, 0x31),
        ];
        let blocks = match self {
            Encoding::EucJp => EUC_JP,
            Encoding::ShiftJis => SHIFT_JIS,
            _ => return None,
        };
        let code = self.code(c);
        let other = blocks.iter().find_map(|&(low, high, distance)| {
            if (low..=high).contains(&code) {
                Some(code + distance)
            } else if (low + distance..=high + distance).contains(&code) {
                Some(code - distance)
            } else {
                None
            }
        })?;
        Some(carried(DOUBLE + other))
    }
}

/// A carried code as a `char`. Every value built here is in the private-use
/// planes, which are scalar values, so the fallback is never taken.
fn carried(n: u32) -> char {
    char::from_u32(n).unwrap_or('\u{fffd}')
}

/// What a single-byte encoding's table says about a byte past ASCII.
impl Table {
    fn holds(mask: u128, b: u8) -> bool {
        b >= 0x80 && mask >> (b - 0x80) & 1 == 1
    }

    /// A POSIX bracket.
    pub(crate) fn posix(&self, kind: Posix, b: u8) -> bool {
        let mask = match kind {
            Posix::Alpha => self.alpha,
            Posix::Digit => self.digit,
            Posix::Alnum => self.alnum,
            Posix::Upper => self.upper,
            Posix::Lower => self.lower,
            Posix::Space => self.space,
            Posix::Blank => self.blank,
            Posix::Punct => self.punct,
            Posix::Print => self.print,
            Posix::Graph => self.graph,
            Posix::Cntrl => self.cntrl,
            Posix::XDigit => self.xdigit,
            Posix::Word => self.word,
            Posix::Ascii => self.ascii,
        };
        Table::holds(mask, b)
    }

    /// A shorthand: ISO-8859-1's `\s` takes `A0`, though UTF-8's is ASCII.
    pub(crate) fn perl(&self, kind: Perl, b: u8) -> bool {
        let mask = match kind {
            Perl::Word => self.perl_word,
            Perl::Digit => self.perl_digit,
            Perl::Space => self.perl_space,
            Perl::Hex => self.perl_hex,
        };
        Table::holds(mask, b)
    }

    /// Whether `\b` takes the byte as a word character.
    pub(crate) fn word(&self, b: u8) -> bool {
        Table::holds(self.word_boundary, b)
    }

    /// Whether `/x/i` matches `y`, for a pair with a side past ASCII.
    pub(crate) fn literal_folds(&self, x: u8, y: u8) -> bool {
        self.folds.binary_search(&(x, y)).is_ok()
    }

    /// Whether a class holding `x` matches `y` under `/i`, for a pair with a
    /// side past ASCII and a class of more than one character.
    pub(crate) fn class_folds(&self, x: u8, y: u8) -> bool {
        self.class_folds.binary_search(&(x, y)).is_ok()
    }

    /// Every `x` a class can hold to match `y` under `/i`, past ASCII's own
    /// cases.
    pub(crate) fn class_partners(&self, y: u8) -> impl Iterator<Item = u8> + '_ {
        self.class_folds
            .iter()
            .filter(move |&&(_, to)| to == y)
            .map(|&(from, _)| from)
    }
}
