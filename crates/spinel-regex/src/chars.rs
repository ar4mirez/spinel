//! How a character is read out of bytes, encoding by encoding (#339).
//!
//! The machine compares `char`s, so every encoding's characters have to be
//! carried as one. UTF-8's are themselves. BINARY's and ISO-8859-1's are a
//! byte each, carried as the `char` with that number — which for ISO-8859-1 is
//! also the character, its bytes being Unicode's first 256 codepoints.
//!
//! EUC-JP's and Shift_JIS's multibyte characters have no Unicode number this
//! engine can compute: that mapping is a table, and it is #333's. They do not
//! need one. Onigmo itself never maps them: a character's code is its bytes
//! read as one big-endian number (`あ` is `0xA4A2` in EUC-JP), a range compares
//! those codes, and the classes and case folds are rules over them. So a
//! two-byte code is carried at `U+F0000` plus the code, and EUC-JP's three-byte
//! `8F xx yy` at `U+100000` plus `xxyy`, both in the supplementary private-use
//! planes. The order of codes survives, so `[ぁ-ん]` still means what it says,
//! and no carried character can be mistaken for one of another kind: ASCII is
//! below `0x80`, a single byte past it is below `0x100`, and the planes are
//! above everything Unicode assigns.

use crate::Encoding;

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

impl Encoding {
    /// Whether a character past ASCII can take more than one byte, so that a
    /// character boundary has to be found by reading from somewhere known.
    fn japanese(self) -> bool {
        matches!(self, Encoding::EucJp | Encoding::ShiftJis)
    }

    /// The length of the character at the start of `bytes`, or `None` when no
    /// character starts there. Measured on ruby 4.0.7, byte by byte:
    ///
    /// - EUC-JP is ASCII; `8E` and one trail, half-width katakana; `8F` and
    ///   two trails, JIS X 0212; or a lead and a trail — every lead and trail
    ///   `A1`-`FE`.
    /// - Shift_JIS and Windows-31J are ASCII; half-width katakana `A1`-`DF`
    ///   alone; or a lead `81`-`9F` or `E0`-`FC` and a trail `40`-`7E` or
    ///   `80`-`FC`. The trail overlaps ASCII: `"\x82\x61"` is one character,
    ///   and there is no `a` in it.
    #[must_use]
    pub fn char_len(self, bytes: &[u8]) -> Option<usize> {
        let first = *bytes.first()?;
        let trail = |at: usize, ok: fn(u8) -> bool| bytes.get(at).copied().is_some_and(ok);
        match self {
            Encoding::Binary | Encoding::Latin1 => Some(1),
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
            Encoding::EucJp => match first {
                0x00..=0x7f => Some(1),
                0x8e => trail(1, euc_byte).then_some(2),
                0x8f => (trail(1, euc_byte) && trail(2, euc_byte)).then_some(3),
                0xa1..=0xfe => trail(1, euc_byte).then_some(2),
                _ => None,
            },
            Encoding::ShiftJis => match first {
                0x00..=0x7f | 0xa1..=0xdf => Some(1),
                0x81..=0x9f | 0xe0..=0xfc => trail(1, sjis_trail).then_some(2),
                _ => None,
            },
        }
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
    /// only a caller of this crate's own API can hand one over.
    pub(crate) fn decode(self, bytes: &[u8]) -> Option<(char, usize)> {
        let first = *bytes.first()?;
        let Some(len) = self.char_len(bytes) else {
            return Some(('\u{fffd}', 1));
        };
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

    /// Whether a character starts at byte `at` of `bytes`.
    ///
    /// UTF-8 can tell from the byte itself. EUC-JP and Shift_JIS cannot — a
    /// trail byte is also a lead, or an ASCII letter — so this reads forward
    /// from the last byte that can only start a character: an ASCII one in
    /// EUC-JP, one no trail can be in Shift_JIS, or the start.
    ///
    /// ponytail: a subject that is one long run of multibyte characters reads
    /// from its start each time, which makes a lookbehind over it quadratic.
    /// Keep a boundary table per subject if that is ever the hot path.
    #[must_use]
    pub fn is_char_boundary(self, bytes: &[u8], at: usize) -> bool {
        match self {
            Encoding::Binary | Encoding::Latin1 => true,
            Encoding::Utf8 => bytes.get(at).is_none_or(|&b| b & 0xc0 != 0x80),
            Encoding::EucJp | Encoding::ShiftJis => {
                at >= bytes.len() || self.char_start_before(bytes, at + 1) == at
            }
        }
    }

    /// Where the character holding byte `at - 1` starts: the start of the
    /// character before `at`, for a `at` that is itself a boundary.
    pub(crate) fn char_start_before(self, bytes: &[u8], at: usize) -> usize {
        if !self.japanese() {
            let mut from = at.saturating_sub(1);
            while from > 0 && !self.is_char_boundary(bytes, from) {
                from -= 1;
            }
            return from;
        }
        // Back to a byte that can only start a character, then forward.
        let certain = |b: u8| match self {
            Encoding::EucJp => b < 0x80,
            _ => !(0x40..=0xfc).contains(&b),
        };
        let mut safe = at.saturating_sub(1);
        while safe > 0 && !certain(bytes[safe - 1]) {
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
    /// in EUC-JP and Shift_JIS. What `Regexp#inspect` escapes one as.
    #[must_use]
    pub fn code(self, c: char) -> u32 {
        let c = u32::from(c);
        if !self.japanese() || c < DOUBLE {
            c
        } else if c >= TRIPLE {
            0x8F_0000 | (c - TRIPLE)
        } else {
            c - DOUBLE
        }
    }

    /// A character's bytes in this encoding: what `Regexp#to_s` writes back.
    #[must_use]
    pub fn char_bytes(self, c: char) -> Vec<u8> {
        if self == Encoding::Utf8 {
            return c.to_string().into_bytes();
        }
        let code = self.code(c).to_be_bytes();
        let used = match self.code(c) {
            0..=0xff => 1,
            0x100..=0xffff => 2,
            _ => 3,
        };
        code[4 - used..].to_vec()
    }

    /// The character a `\x` escape's bytes spell, for the multibyte encodings:
    /// `\xa4\xa2` is `あ` in EUC-JP. `None` when they are not one character.
    pub(crate) fn spelled_by(self, bytes: &[u8]) -> Option<char> {
        (self.char_len(bytes)? == bytes.len())
            .then(|| self.decode(bytes).map(|(c, _)| c))
            .flatten()
    }

    /// How many bytes a character that starts with `lead` takes, for the
    /// multibyte encodings; `None` for a byte that leads nothing.
    pub(crate) fn lead_len(self, lead: u8) -> Option<usize> {
        match (self, lead) {
            (_, 0x00..=0x7f) => Some(1),
            (Encoding::EucJp, 0x8f) => Some(3),
            (Encoding::EucJp, 0x8e | 0xa1..=0xfe) => Some(2),
            (Encoding::ShiftJis, 0xa1..=0xdf) => Some(1),
            (Encoding::ShiftJis, 0x81..=0x9f | 0xe0..=0xfc) => Some(2),
            _ => None,
        }
    }

    /// The other case of a multibyte character, by Onigmo's arithmetic: the
    /// full-width Latin letters, Greek and Cyrillic, each a block a fixed
    /// distance from its other case. Shift_JIS's Cyrillic lower case skips
    /// `847F`, which is no trail, so its second half is one further away.
    /// Measured over every two-byte code, both encodings, ruby 4.0.7.
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

/// Onigmo's ISO-8859-1 classes for the bytes past ASCII, which are not
/// Unicode's for the same codepoints: `\s` takes `A0` though UTF-8's is ASCII,
/// `[[:space:]]` does not take `85`, `§` and `¶` are no punctuation and the
/// soft hyphen is, and the superscripts and fractions are word characters.
/// Measured into `tests/encodings.txt`, which a unit test replays.
pub(crate) mod latin1 {
    use crate::parse::{Perl, Posix};

    const LETTER: &[(u8, u8)] = &[
        (0xaa, 0xaa),
        (0xb5, 0xb5),
        (0xba, 0xba),
        (0xc0, 0xd6),
        (0xd8, 0xf6),
        (0xf8, 0xff),
    ];
    const UPPER: &[(u8, u8)] = &[(0xc0, 0xd6), (0xd8, 0xde)];
    const LOWER: &[(u8, u8)] = &[
        (0xaa, 0xaa),
        (0xb5, 0xb5),
        (0xba, 0xba),
        (0xdf, 0xf6),
        (0xf8, 0xff),
    ];
    const PUNCT: &[(u8, u8)] = &[
        (0xa1, 0xa1),
        (0xab, 0xab),
        (0xad, 0xad),
        (0xb7, 0xb7),
        (0xbb, 0xbb),
        (0xbf, 0xbf),
    ];
    const WORD: &[(u8, u8)] = &[
        (0xaa, 0xaa),
        (0xb2, 0xb3),
        (0xb5, 0xb5),
        (0xb9, 0xba),
        (0xbc, 0xbe),
        (0xc0, 0xd6),
        (0xd8, 0xf6),
        (0xf8, 0xff),
    ];

    fn within(ranges: &[(u8, u8)], b: u8) -> bool {
        ranges.iter().any(|&(low, high)| (low..=high).contains(&b))
    }

    /// A POSIX bracket, for a byte `80`-`FF`.
    pub fn posix(kind: Posix, b: u8) -> bool {
        match kind {
            Posix::Alpha | Posix::Alnum => within(LETTER, b),
            Posix::Upper => within(UPPER, b),
            Posix::Lower => within(LOWER, b),
            Posix::Space | Posix::Blank => b == 0xa0,
            Posix::Print => b >= 0xa0,
            Posix::Graph => b >= 0xa1,
            Posix::Punct => within(PUNCT, b),
            Posix::Cntrl => b <= 0x9f,
            Posix::Word => within(WORD, b),
            Posix::Digit | Posix::XDigit | Posix::Ascii => false,
        }
    }

    /// A shorthand, for a byte `80`-`FF`: only `\s`, and only for `A0`.
    pub fn perl(kind: Perl, b: u8) -> bool {
        kind == Perl::Space && b == 0xa0
    }
}
