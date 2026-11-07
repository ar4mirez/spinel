//! EUC-KR for transcoding (#725). Reading a character is arithmetic, as
//! EUC-JP's is: ASCII is itself, and past it a lead byte 0xA1 to 0xFE and one
//! more in that range spell a *cell* of KS X 1001, its row and column from 0
//! in a plane of 94 columns. What a cell means in Unicode is a table,
//! generated from CRuby into `euckr_table.rs` by `scripts/euckr-table.rb`,
//! which also checks everything this file assumes about the bytes against
//! CRuby. The plane is read and inverted at compile time by `jis.rs`'s
//! builders.

use crate::euckr_table;
use crate::jis::{plane, writer};
use crate::strings::EUC_KR;

const CELLS: usize = 94 * 94;

static KSX1001: [u16; CELLS] = plane(euckr_table::KSX1001);

static EUC_KR_WRITE: [(u16, u16); euckr_table::EUC_KR_WRITTEN] = writer(
    &KSX1001,
    &[],
    euckr_table::EUC_KR_UNCHOSEN,
    euckr_table::EUC_KR_ONE_WAY,
);

/// Whether `encoding` is EUC-KR.
#[must_use]
pub fn supported(encoding: u8) -> bool {
    encoding == EUC_KR
}

/// One character read, or why there is none — CRuby's shapes: a bad lead
/// byte is the error alone; a bad byte after a lead is given back to be read
/// again. A character is its codepoint, or `None` for a cell CRuby has no
/// character for.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    Char(Option<u32>, usize),
    Invalid { error: usize, readagain: usize },
    Incomplete(usize),
}

/// Reads the character at `at`.
#[must_use]
pub fn read(bytes: &[u8], at: usize) -> Read {
    let rest = &bytes[at..];
    let lead = rest[0];
    if lead < 0x80 {
        return Read::Char(Some(u32::from(lead)), 1);
    }
    let follows = |b: u8| (0xa1..=0xfe).contains(&b);
    if !follows(lead) {
        return Read::Invalid {
            error: 1,
            readagain: 0,
        };
    }
    match rest.get(1) {
        None => Read::Incomplete(1),
        Some(&trail) if follows(trail) => {
            let cell = usize::from(lead - 0xa1) * 94 + usize::from(trail - 0xa1);
            let code = KSX1001[cell];
            Read::Char((code != 0).then_some(u32::from(code)), 2)
        }
        Some(_) => Read::Invalid {
            error: 1,
            readagain: 1,
        },
    }
}

/// `code`'s bytes in EUC-KR, or `None` when CRuby writes no character for it.
#[must_use]
pub fn encode(code: u32) -> Option<Vec<u8>> {
    if code < 0x80 {
        return Some(vec![code as u8]);
    }
    let code = u16::try_from(code).ok()?;
    let at = EUC_KR_WRITE.binary_search_by_key(&code, |&(c, _)| c).ok()?;
    let cell = EUC_KR_WRITE[at].1;
    Some(vec![0xa1 + (cell / 94) as u8, 0xa1 + (cell % 94) as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn back(bytes: &[u8]) -> Option<u32> {
        match read(bytes, 0) {
            Read::Char(code, len) if len == bytes.len() => code,
            _ => None,
        }
    }

    // Measured on ruby 4.0.9 (#725).
    #[test]
    fn writes_and_reads_as_cruby_does() {
        assert_eq!(encode(0xac00), Some(vec![0xb0, 0xa1]));
        assert_eq!(encode(0xd79d), Some(vec![0xc8, 0xfe]));
        assert_eq!(encode(0x203b), Some(vec![0xa1, 0xd8]));
        assert_eq!(encode(0x61), Some(vec![0x61]));
        assert_eq!(encode(0xe9), None);
        assert_eq!(back(&[0xb0, 0xa1]), Some(0xac00));
        assert_eq!(back(&[0xc7, 0xd1]), Some(0xd55c));
        assert_eq!(read(&[0xa2, 0xe8], 0), Read::Char(None, 2));
        assert_eq!(read(&[0xb0], 0), Read::Incomplete(1));
        assert_eq!(
            read(&[0xb0, 0x41], 0),
            Read::Invalid {
                error: 1,
                readagain: 1
            }
        );
        assert_eq!(
            read(&[0xff], 0),
            Read::Invalid {
                error: 1,
                readagain: 0
            }
        );
    }

    /// Every character EUC-KR writes reads back as itself: the table has no
    /// one-way characters, which `scripts/euckr-table.rb` checks. One quarter
    /// at a time, as `jis.rs` does, so that no one test holds miri's wall time.
    fn every_written_cell_reads_back(quarter: usize) {
        let start = EUC_KR_WRITE.len() * quarter / 4;
        let end = EUC_KR_WRITE.len() * (quarter + 1) / 4;
        for &(code, _) in &EUC_KR_WRITE[start..end] {
            let bytes = encode(u32::from(code)).unwrap();
            assert_eq!(back(&bytes), Some(u32::from(code)), "U+{code:04X}");
        }
    }

    #[test]
    fn every_written_cell_reads_back_first_quarter() {
        every_written_cell_reads_back(0);
    }

    #[test]
    fn every_written_cell_reads_back_second_quarter() {
        every_written_cell_reads_back(1);
    }

    #[test]
    fn every_written_cell_reads_back_third_quarter() {
        every_written_cell_reads_back(2);
    }

    #[test]
    fn every_written_cell_reads_back_fourth_quarter() {
        every_written_cell_reads_back(3);
    }
}
