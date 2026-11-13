//! The compiled-app trailer (docs/build.md, "Format"; #98).
//!
//! A compiled app is a copy of the `spinel` binary with a payload after it,
//! then 48 bytes that say where the payload is and what it hashes to:
//!
//! ```text
//! [ spinel ][ payload ][ sha256 of payload: 32 ][ payload length: 8, u64 LE ][ "SPINEL01": 8 ]
//! ```
//!
//! On Linux the payload is a plain append: the ELF loader maps what the
//! program headers name and ignores anything past them. Reading a plain
//! `spinel` costs one seek and one 48-byte read; the payload is hashed only
//! once the magic says one is there.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// The last 8 bytes of every compiled app. The `01` is the trailer version: a
/// change to this layout is a new magic, so an old `spinel` reading a new app
/// sees no app rather than misreading one.
pub const MAGIC: [u8; 8] = *b"SPINEL01";

/// Bytes in the trailer: the sha256, the length, the magic.
pub const LEN: usize = 48;

/// The 48 bytes at the end of a compiled app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trailer {
    /// The sha256 of the payload bytes, and nothing else.
    pub sha256: [u8; 32],
    /// How many payload bytes sit directly before the trailer.
    pub payload_len: u64,
}

impl Trailer {
    /// The trailer that vouches for `payload`.
    pub fn for_payload(payload: &[u8]) -> Trailer {
        Trailer {
            sha256: Sha256::digest(payload).into(),
            payload_len: payload.len() as u64,
        }
    }

    /// The trailer as it is written: sha256, length little-endian, magic.
    pub fn to_bytes(self) -> [u8; LEN] {
        let mut bytes = [0; LEN];
        bytes[..32].copy_from_slice(&self.sha256);
        bytes[32..40].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[40..].copy_from_slice(&MAGIC);
        bytes
    }

    /// The trailer these 48 bytes hold, or `None` when they do not end in the
    /// magic, which is every `spinel` that is not a compiled app.
    pub fn from_bytes(bytes: &[u8; LEN]) -> Option<Trailer> {
        if bytes[40..] != MAGIC {
            return None;
        }
        let mut sha256 = [0; 32];
        sha256.copy_from_slice(&bytes[..32]);
        let mut length = [0; 8];
        length.copy_from_slice(&bytes[32..40]);
        Some(Trailer {
            sha256,
            payload_len: u64::from_le_bytes(length),
        })
    }
}

/// A compiled app's payload, located and verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    /// Where the payload starts in the file.
    pub payload_offset: u64,
    pub trailer: Trailer,
}

/// Why a file that ends in the magic is not a usable compiled app.
#[derive(Debug)]
pub enum Error {
    /// The trailer claims more payload than the file holds before it.
    Truncated { declared: u64, available: u64 },
    /// The payload's bytes do not hash to what the trailer says.
    Corrupt {
        expected: [u8; 32],
        actual: [u8; 32],
    },
    /// Reading the file failed.
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated {
                declared,
                available,
            } => write!(
                f,
                "the compiled-app payload is truncated: the trailer declares {declared} bytes \
                 but the file holds only {available} before it"
            ),
            Error::Corrupt { expected, actual } => write!(
                f,
                "the compiled-app payload is corrupt: its sha256 is {} but the trailer says {}",
                hex(actual),
                hex(expected)
            ),
            Error::Io(err) => write!(f, "cannot read the compiled-app payload: {err}"),
        }
    }
}

/// Lowercase hex, as `sha256sum` prints a digest.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The compiled app `file` holds, if it holds one: `Ok(None)` when it does not
/// end in the magic, an error when it does but the payload is short or does not
/// match its sha256.
pub fn read<F: Read + Seek>(file: &mut F) -> Result<Option<App>, Error> {
    let len = file.seek(SeekFrom::End(0))?;
    if len < LEN as u64 {
        return Ok(None);
    }
    let available = len - LEN as u64;
    file.seek(SeekFrom::Start(available))?;
    let mut bytes = [0; LEN];
    file.read_exact(&mut bytes)?;
    let Some(trailer) = Trailer::from_bytes(&bytes) else {
        return Ok(None);
    };
    if trailer.payload_len > available {
        return Err(Error::Truncated {
            declared: trailer.payload_len,
            available,
        });
    }
    let payload_offset = available - trailer.payload_len;
    file.seek(SeekFrom::Start(payload_offset))?;
    // Streamed, so a 150 MB payload is not read into memory to be hashed.
    let mut hasher = Sha256::new();
    let hashed = io::copy(
        &mut file.take(trailer.payload_len),
        &mut HashWriter(&mut hasher),
    )?;
    if hashed != trailer.payload_len {
        // The file shrank between the seek to its end and the read.
        return Err(Error::Truncated {
            declared: trailer.payload_len,
            available: hashed,
        });
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != trailer.sha256 {
        return Err(Error::Corrupt {
            expected: trailer.sha256,
            actual,
        });
    }
    Ok(Some(App {
        payload_offset,
        trailer,
    }))
}

/// Write `binary`, then `payload`, then the trailer for it, to `out`: a
/// compiled app on Linux, where the loader ignores bytes past the ELF.
/// The copy keeps `binary`'s permissions, so `out` is executable.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "`spinel build` (#100) is the caller; only tests call it until it lands"
    )
)]
pub fn append(binary: &Path, payload: &[u8], out: &Path) -> io::Result<()> {
    std::fs::copy(binary, out)?;
    let mut file = std::fs::OpenOptions::new().append(true).open(out)?;
    file.write_all(payload)?;
    file.write_all(&Trailer::for_payload(payload).to_bytes())?;
    file.sync_all()
}

/// Feeds what is written into a sha256.
struct HashWriter<'a>(&'a mut Sha256);

impl Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The running executable, opened for reading. On Linux `/proc/self/exe` is
/// the file the kernel loaded even if its path has since been replaced.
pub fn open_self() -> io::Result<File> {
    #[cfg(target_os = "linux")]
    if let Ok(file) = File::open("/proc/self/exe") {
        return Ok(file);
    }
    File::open(std::env::current_exe()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// The sha256 of "hello", as `printf hello | sha256sum` prints it.
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn app(binary: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut file = binary.to_vec();
        file.extend_from_slice(payload);
        file.extend_from_slice(&Trailer::for_payload(payload).to_bytes());
        file
    }

    #[test]
    fn the_trailer_is_sha256_then_length_little_endian_then_magic() {
        let bytes = Trailer::for_payload(b"hello").to_bytes();
        assert_eq!(bytes.len(), 48);
        assert_eq!(hex(&bytes[..32]), HELLO_SHA256);
        assert_eq!(bytes[32..40], [5, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[40..], b"SPINEL01");
    }

    #[test]
    fn a_length_wider_than_a_byte_is_little_endian() {
        let trailer = Trailer {
            sha256: [0xab; 32],
            payload_len: 0x0102_0304_0506_0708,
        };
        let bytes = trailer.to_bytes();
        assert_eq!(bytes[32..40], [8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(Trailer::from_bytes(&bytes), Some(trailer));
    }

    #[test]
    fn bytes_without_the_magic_are_no_trailer() {
        let mut bytes = Trailer::for_payload(b"hello").to_bytes();
        bytes[47] = b'2';
        assert_eq!(Trailer::from_bytes(&bytes), None);
    }

    #[test]
    fn a_compiled_app_reads_back_its_payload() {
        let file = app(b"\x7fELF binary", b"hello");
        let found = read(&mut Cursor::new(&file))
            .expect("reads")
            .expect("an app");
        assert_eq!(found.payload_offset, 11);
        assert_eq!(found.trailer.payload_len, 5);
        let start = usize::try_from(found.payload_offset).expect("fits");
        assert_eq!(&file[start..start + 5], b"hello");
    }

    #[test]
    fn an_empty_payload_is_still_an_app() {
        let file = app(b"binary", b"");
        let found = read(&mut Cursor::new(&file))
            .expect("reads")
            .expect("an app");
        assert_eq!(found.payload_offset, 6);
    }

    #[test]
    fn a_plain_binary_is_no_app() {
        assert!(matches!(read(&mut Cursor::new(vec![0x7f; 4096])), Ok(None)));
    }

    #[test]
    fn a_file_shorter_than_a_trailer_is_no_app() {
        assert!(matches!(read(&mut Cursor::new(b"SPINEL01")), Ok(None)));
        assert!(matches!(read(&mut Cursor::new(Vec::new())), Ok(None)));
    }

    #[test]
    fn a_flipped_payload_byte_is_corrupt() {
        let mut file = app(b"binary", b"hello");
        file[7] ^= 0x01;
        let err = read(&mut Cursor::new(&file)).expect_err("corrupt");
        assert!(matches!(err, Error::Corrupt { .. }), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("corrupt"), "{message}");
        assert!(message.contains(HELLO_SHA256), "{message}");
    }

    #[test]
    fn a_payload_with_bytes_missing_is_corrupt() {
        // Bytes lost from the payload shift its start into the binary, so
        // what is hashed is no longer the payload.
        let mut file = app(b"binary", b"hello");
        file.remove(8);
        let err = read(&mut Cursor::new(&file)).expect_err("corrupt");
        assert!(matches!(err, Error::Corrupt { .. }), "{err:?}");
    }

    #[test]
    fn a_length_past_the_start_of_the_file_is_truncated() {
        let mut file = b"bin".to_vec();
        file.extend_from_slice(&Trailer::for_payload(b"hello").to_bytes());
        let err = read(&mut Cursor::new(&file)).expect_err("truncated");
        assert!(
            matches!(
                err,
                Error::Truncated {
                    declared: 5,
                    available: 3
                }
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("truncated"), "{err}");
    }

    #[test]
    fn append_writes_binary_payload_and_trailer() {
        let dir = std::env::temp_dir().join(format!("spinel-trailer-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let binary = dir.join("spinel");
        let out = dir.join("app");
        std::fs::write(&binary, b"binary").expect("write");
        append(&binary, b"hello", &out).expect("append");
        let written = std::fs::read(&out).expect("read");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(written, app(b"binary", b"hello"));
    }
}
