//! The single-byte code pages for transcoding (#41): ISO-8859-2 to -16,
//! KOI8-R and -U, the Windows and IBM code pages, the Mac ones, TIS-620 —
//! every encoding CRuby's `enc/trans/single_byte.trans` converts to and from
//! UTF-8 but ISO-8859-1, which `transcode.rs` does by arithmetic. Every byte
//! is a character on its own: ASCII is itself, and what a byte past it means
//! in Unicode is a table, generated from CRuby into `codepage_table.rs` by
//! `scripts/codepage-table.rb`, which also checks everything this file
//! assumes against CRuby. The table is read and inverted at compile time.

use crate::codepage_table::PAGES;
use crate::jis::plane;

/// One code page, as `codepage_table.rs` gives it.
pub struct Page {
    /// Its encoding's index.
    pub index: u8,
    /// What the bytes 0x80 to 0xFF read as, in order, as text: `\0` for a
    /// byte with no character.
    pub upper: &'static str,
    /// The characters it writes as more than one byte, sorted, and their
    /// bytes: Windows-1255's Hebrew presentation forms, written decomposed.
    pub wide: &'static [(u16, &'static [u8])],
}

const COUNT: usize = PAGES.len();

/// Where each encoding's page is in [`PAGES`], by encoding index, or
/// `u8::MAX` for an encoding that is no code page.
static SLOTS: [u8; 256] = {
    let mut slots = [u8::MAX; 256];
    let mut i = 0;
    while i < COUNT {
        slots[PAGES[i].index as usize] = i as u8;
        i += 1;
    }
    slots
};

/// Each page's upper half, read: the codepoint each byte from 0x80 reads as,
/// or 0 for none.
static READ: [[u16; 128]; COUNT] = {
    let mut read = [[0u16; 128]; COUNT];
    let mut i = 0;
    while i < COUNT {
        read[i] = plane(PAGES[i].upper);
        i += 1;
    }
    read
};

/// Each page's upper half inverted, sorted by codepoint: every character a
/// byte reads as and the byte, which is the one CRuby writes it as. The
/// bytes with no character sort last, as `u16::MAX`, which no byte reads as.
static WRITE: [[(u16, u8); 128]; COUNT] = {
    let mut write = [[(u16::MAX, 0u8); 128]; COUNT];
    let mut i = 0;
    while i < COUNT {
        let mut n = 0;
        let mut byte = 0;
        while byte < 128 {
            let code = READ[i][byte];
            assert!(code != u16::MAX, "a code page reads U+FFFF");
            if code != 0 {
                // An insertion sort, at compile time.
                let mut at = n;
                while at > 0 && write[i][at - 1].0 > code {
                    write[i][at] = write[i][at - 1];
                    at -= 1;
                }
                assert!(
                    at == 0 || write[i][at - 1].0 != code,
                    "two bytes read as one character"
                );
                write[i][at] = (code, 0x80 + byte as u8);
                n += 1;
            }
            byte += 1;
        }
        i += 1;
    }
    write
};

fn slot(encoding: u8) -> Option<usize> {
    let slot = SLOTS[usize::from(encoding)];
    (slot != u8::MAX).then_some(usize::from(slot))
}

/// Whether `encoding` is a code page.
#[must_use]
pub fn supported(encoding: u8) -> bool {
    slot(encoding).is_some()
}

/// What `byte` reads as in `encoding`, a code page, or `None` for a byte
/// CRuby has no character for.
#[must_use]
pub fn read(encoding: u8, byte: u8) -> Option<u32> {
    if byte < 0x80 {
        return Some(u32::from(byte));
    }
    let code = READ[slot(encoding)?][usize::from(byte - 0x80)];
    (code != 0).then_some(u32::from(code))
}

/// `code`'s bytes in `encoding`, a code page, or `None` when CRuby writes no
/// character for it.
#[must_use]
pub fn encode(encoding: u8, code: u32) -> Option<Vec<u8>> {
    if code < 0x80 {
        return Some(vec![code as u8]);
    }
    let page = slot(encoding)?;
    let code = u16::try_from(code).ok().filter(|&code| code != u16::MAX)?;
    if let Ok(at) = WRITE[page].binary_search_by_key(&code, |&(c, _)| c) {
        return Some(vec![WRITE[page][at].1]);
    }
    let wide = PAGES[page].wide;
    let at = wide.binary_search_by_key(&code, |&(c, _)| c).ok()?;
    Some(wide[at].1.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IBM437: u8 = 46;
    const WINDOWS_1252: u8 = 42;
    const WINDOWS_1255: u8 = 90;
    const KOI8_R: u8 = 37;

    // Measured on ruby 4.0.9 (#41).
    #[test]
    fn reads_and_writes_as_cruby_does() {
        assert_eq!(encode(IBM437, 0x192), Some(vec![0x9f]));
        assert_eq!(read(IBM437, 0x82), Some(0xe9));
        assert_eq!(read(WINDOWS_1252, 0x80), Some(0x20ac));
        assert_eq!(encode(WINDOWS_1252, 0x20ac), Some(vec![0x80]));
        assert_eq!(read(WINDOWS_1252, 0x81), None);
        assert_eq!(encode(WINDOWS_1252, 0x3042), None);
        assert_eq!(encode(WINDOWS_1252, 0x1_0000), None);
        assert_eq!(encode(WINDOWS_1252, 0x41), Some(vec![0x41]));
        assert_eq!(read(KOI8_R, 0xc1), Some(0x430));
        assert_eq!(encode(WINDOWS_1255, 0xfb2c), Some(vec![0xf9, 0xcc, 0xd1]));
        assert_eq!(encode(WINDOWS_1255, 0x5d0), Some(vec![0xe0]));
        assert!(!supported(1));
        assert!(!supported(22));
        assert_eq!(encode(1, 0xe9), None);
    }

    /// Every byte a page reads as a character is the byte it writes that
    /// character as, which `scripts/codepage-table.rb` checks against CRuby.
    #[test]
    fn every_byte_reads_back() {
        for page in PAGES {
            for byte in 0..=0xffu8 {
                if let Some(code) = read(page.index, byte) {
                    assert_eq!(
                        encode(page.index, code),
                        Some(vec![byte]),
                        "{} {byte:#x}",
                        page.index
                    );
                }
            }
        }
    }
}
