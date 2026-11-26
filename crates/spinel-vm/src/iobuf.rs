//! Mapped memory for `IO::Buffer` (#797).
//!
//! `IO::Buffer` is Ruby, in `core/io_buffer.rb`; the one thing it cannot
//! hold in a String is memory `mmap(2)` made: an anonymous mapping a
//! `MAPPED` buffer is born with — shared with a forked child when it is
//! `SHARED` — or a file `IO::Buffer.map` maps. Those live here, one
//! [`Mappings`] per heap, so per Ractor, keyed by the address the mapping
//! starts at.
//!
//! Ruby names a mapping by that address, which `IO::Buffer#to_s` shows, as
//! CRuby's does. Every read and write names the mapping and an offset into
//! it, and is checked here against the length the mapping was made with —
//! and a write against whether it was made writable — so an address Ruby
//! got wrong is a refusal rather than a read of memory that is not there.
//! That check is the whole of what keeps the unsafe code below sound: each
//! block reads or writes inside `[address, address + len)` of a live
//! mapping this table made and has not yet unmapped.
//!
//! The heap does not trace a mapping. What frees one Ruby has dropped is
//! the finalizer `core/io_buffer.rb` gives the object that owns it; what is
//! left when the heap goes is unmapped by [`Mappings`]'s `Drop`.

use std::collections::BTreeMap;

/// One live mapping: how long it is, and whether it was mapped writable.
#[derive(Debug, Clone, Copy)]
struct Region {
    len: usize,
    writable: bool,
}

/// The mappings of one heap, by the address each starts at.
#[derive(Debug, Default)]
pub struct Mappings {
    regions: BTreeMap<usize, Region>,
}

/// What `mmap(2)` is asked for: the file and where in it, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// The descriptor to map, or `None` for anonymous memory, zero-filled.
    pub fd: Option<i32>,
    /// Bytes, more than zero.
    pub len: usize,
    /// Where in the file, a multiple of the page size.
    pub offset: i64,
    /// `MAP_SHARED`, rather than `MAP_PRIVATE`.
    pub shared: bool,
    /// `PROT_WRITE` as well as `PROT_READ`.
    pub writable: bool,
}

fn errno() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL)
}

/// `sysconf(_SC_PAGESIZE)`: `IO::Buffer::PAGE_SIZE`.
#[must_use]
pub fn page_size() -> usize {
    // SAFETY: `sysconf` reads a constant of the system.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size)
        .ok()
        .filter(|&n| n > 0)
        .unwrap_or(4096)
}

impl Mappings {
    /// Map what `request` asks for: its address, or the errno `mmap`
    /// failed with.
    pub fn map(&mut self, request: Request) -> Result<usize, i32> {
        if request.len == 0 {
            return Err(libc::EINVAL);
        }
        let prot = if request.writable {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        let mut flags = if request.shared {
            libc::MAP_SHARED
        } else {
            libc::MAP_PRIVATE
        };
        let fd = match request.fd {
            Some(fd) => fd,
            None => {
                flags |= libc::MAP_ANONYMOUS;
                -1
            }
        };
        let offset = libc::off_t::try_from(request.offset).map_err(|_| libc::EINVAL)?;
        // SAFETY: a fresh mapping at an address the kernel picks, which
        // aliases nothing Rust holds.
        let address =
            unsafe { libc::mmap(std::ptr::null_mut(), request.len, prot, flags, fd, offset) };
        if address == libc::MAP_FAILED {
            return Err(errno());
        }
        let address = address as usize;
        self.regions.insert(
            address,
            Region {
                len: request.len,
                writable: request.writable,
            },
        );
        Ok(address)
    }

    /// Unmap the mapping at `address`: false when there is none.
    pub fn unmap(&mut self, address: usize) -> bool {
        let Some(region) = self.regions.remove(&address) else {
            return false;
        };
        // SAFETY: a mapping `map` made, of that length, now out of the table
        // so nothing reads it again.
        unsafe { libc::munmap(address as *mut libc::c_void, region.len) };
        true
    }

    /// The length of the mapping at `address`, if there is one.
    #[must_use]
    pub fn len(&self, address: usize) -> Option<usize> {
        self.regions.get(&address).map(|region| region.len)
    }

    /// Whether the heap holds no mapping.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Make the mapping at `address` `len` bytes long, keeping what fits,
    /// as `mremap(2)` does: the new address, which may be the old one, or
    /// the errno. Linux alone has `mremap`; elsewhere this is `ENOSYS`, and
    /// `core/io_buffer.rb` makes a fresh buffer instead, as CRuby does.
    pub fn remap(&mut self, address: usize, len: usize) -> Result<usize, i32> {
        let Some(region) = self.regions.get(&address).copied() else {
            return Err(libc::EINVAL);
        };
        if len == 0 {
            return Err(libc::EINVAL);
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: a mapping `map` made, of that length; the kernel moves
            // it or grows it in place, and the table follows below.
            let moved = unsafe {
                libc::mremap(
                    address as *mut libc::c_void,
                    region.len,
                    len,
                    libc::MREMAP_MAYMOVE,
                )
            };
            if moved == libc::MAP_FAILED {
                return Err(errno());
            }
            self.regions.remove(&address);
            let moved = moved as usize;
            self.regions.insert(
                moved,
                Region {
                    len,
                    writable: region.writable,
                },
            );
            Ok(moved)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = region;
            Err(libc::ENOSYS)
        }
    }

    /// A copy of the `len` bytes at `offset` in the mapping at `address`, or
    /// `None` when there is no such mapping or they do not all lie in it.
    ///
    /// A copy, never a reference: a `SHARED` mapping is memory another
    /// process may write at any moment, which a `&[u8]` would promise does
    /// not change.
    #[must_use]
    pub fn read(&self, address: usize, offset: usize, len: usize) -> Option<Vec<u8>> {
        let region = self.regions.get(&address)?;
        let end = offset.checked_add(len)?;
        if end > region.len {
            return None;
        }
        let mut bytes = vec![0; len];
        // SAFETY: `[offset, end)` lies inside a live mapping this table made
        // readable, and `bytes` is a fresh allocation of `len` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping((address + offset) as *const u8, bytes.as_mut_ptr(), len);
        }
        Some(bytes)
    }

    /// Write `bytes` at `offset` in the mapping at `address`: false, and
    /// nothing written, when there is no such mapping, it is not writable,
    /// or they do not all fit.
    pub fn write(&mut self, address: usize, offset: usize, bytes: &[u8]) -> bool {
        let Some(at) = self.writable_range(address, offset, bytes.len()) else {
            return false;
        };
        // SAFETY: `writable_range` found `bytes.len()` writable bytes at
        // `at`, in a mapping no Rust reference points into, and `bytes` is
        // the caller's own memory, so the two do not overlap.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len()) };
        true
    }

    /// Set `len` bytes at `offset` in the mapping at `address` to `byte`,
    /// under [`Mappings::write`]'s rules.
    pub fn fill(&mut self, address: usize, offset: usize, len: usize, byte: u8) -> bool {
        let Some(at) = self.writable_range(address, offset, len) else {
            return false;
        };
        // SAFETY: as in `write`.
        unsafe { std::ptr::write_bytes(at, byte, len) };
        true
    }

    fn writable_range(&self, address: usize, offset: usize, len: usize) -> Option<*mut u8> {
        let region = self.regions.get(&address)?;
        let end = offset.checked_add(len)?;
        (region.writable && end <= region.len).then_some((address + offset) as *mut u8)
    }
}

impl Drop for Mappings {
    fn drop(&mut self) {
        for (&address, region) in &self.regions {
            // SAFETY: a mapping `map` made and never unmapped; the table goes
            // with it.
            unsafe { libc::munmap(address as *mut libc::c_void, region.len) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mappings, Request};

    fn anonymous(len: usize) -> Request {
        Request {
            fd: None,
            len,
            offset: 0,
            shared: false,
            writable: true,
        }
    }

    #[test]
    fn an_anonymous_mapping_reads_zero_and_keeps_what_is_written() {
        let mut maps = Mappings::default();
        let at = maps.map(anonymous(100)).expect("mapped");
        assert_eq!(maps.len(at), Some(100));
        assert_eq!(maps.read(at, 0, 4).as_deref(), Some(&[0, 0, 0, 0][..]));
        assert!(maps.write(at, 10, b"hello"));
        assert_eq!(maps.read(at, 10, 5).as_deref(), Some(&b"hello"[..]));
        assert!(maps.fill(at, 11, 3, b'z'));
        assert_eq!(maps.read(at, 10, 5).as_deref(), Some(&b"hzzzo"[..]));
        assert!(maps.unmap(at));
        assert!(maps.is_empty());
    }

    #[test]
    fn what_lies_outside_a_mapping_is_refused() {
        let mut maps = Mappings::default();
        let at = maps.map(anonymous(16)).expect("mapped");
        assert_eq!(maps.read(at, 12, 5).as_deref(), None);
        assert_eq!(maps.read(at, usize::MAX, 2).as_deref(), None);
        assert!(!maps.write(at, 16, b"x"));
        assert!(!maps.fill(at, 8, 9, 0));
        assert_eq!(
            maps.read(at + 1, 0, 1).as_deref(),
            None,
            "only a mapping's start names it"
        );
        assert!(!maps.unmap(at + 1));
        assert_eq!(
            maps.read(at, 16, 0).as_deref(),
            Some(&[][..]),
            "the empty range at the end"
        );
        assert!(maps.unmap(at));
        assert_eq!(
            maps.read(at, 0, 1).as_deref(),
            None,
            "an unmapped address names nothing"
        );
        assert!(!maps.unmap(at));
    }

    // Miri maps `PROT_READ | PROT_WRITE` and nothing else.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_read_only_mapping_refuses_writes() {
        let mut maps = Mappings::default();
        let at = maps
            .map(Request {
                writable: false,
                ..anonymous(8)
            })
            .expect("mapped");
        assert!(!maps.write(at, 0, b"x"));
        assert!(!maps.fill(at, 0, 1, 1));
        assert_eq!(maps.read(at, 0, 1).as_deref(), Some(&[0][..]));
    }

    /// A remap keeps what fits and the table follows it: a shrink bounds
    /// reads by the new length, and a grow by a page reads its new page as
    /// zero. The rest of the page a shrink cut is the kernel's to keep, so
    /// `core/io_buffer.rb` zero-fills it itself.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_remap_keeps_what_fits_and_the_table_follows() {
        let mut maps = Mappings::default();
        let page = super::page_size();
        let at = maps.map(anonymous(page)).expect("mapped");
        assert!(maps.write(at, 0, b"abcd"));
        let at = maps.remap(at, 2).expect("shrunk");
        assert_eq!(maps.read(at, 0, 2).as_deref(), Some(&b"ab"[..]));
        assert_eq!(maps.read(at, 0, 3).as_deref(), None);
        let at = maps.remap(at, page * 2).expect("grown");
        assert_eq!(maps.len(at), Some(page * 2));
        assert_eq!(maps.read(at, 0, 2).as_deref(), Some(&b"ab"[..]));
        assert_eq!(maps.read(at, page, 4).as_deref(), Some(&[0, 0, 0, 0][..]));
        assert_eq!(maps.remap(at, 0), Err(libc::EINVAL));
        assert_eq!(maps.remap(at + 1, 8), Err(libc::EINVAL));
        assert!(maps.unmap(at));
    }

    #[test]
    fn nothing_is_mapped_for_no_bytes() {
        let mut maps = Mappings::default();
        assert_eq!(maps.map(anonymous(0)), Err(libc::EINVAL));
        assert!(maps.is_empty());
    }

    // Miri runs private anonymous mappings only.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_shared_mapping_is_written_through_to_a_file() {
        use std::io::{Read, Seek, SeekFrom, Write};
        use std::os::fd::AsRawFd;
        let dir = std::env::temp_dir().join(format!("spinel-iobuf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("shared");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("a temp file");
        file.write_all(b"abcdef").expect("written");
        let mut maps = Mappings::default();
        let at = maps
            .map(Request {
                fd: Some(file.as_raw_fd()),
                len: 6,
                offset: 0,
                shared: true,
                writable: true,
            })
            .expect("mapped");
        assert_eq!(maps.read(at, 0, 6).as_deref(), Some(&b"abcdef"[..]));
        assert!(maps.write(at, 0, b"XY"));
        file.seek(SeekFrom::Start(0)).expect("rewound");
        let mut seen = String::new();
        file.read_to_string(&mut seen).expect("read");
        assert_eq!(seen, "XYcdef");
        assert!(maps.unmap(at));
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_read_only_descriptor_cannot_be_mapped_shared_and_writable() {
        use std::os::fd::AsRawFd;
        let dir = std::env::temp_dir().join(format!("spinel-iobuf-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("ro");
        std::fs::write(&path, b"abc").expect("written");
        let file = std::fs::File::open(&path).expect("opened");
        let mut maps = Mappings::default();
        let shared = maps.map(Request {
            fd: Some(file.as_raw_fd()),
            len: 3,
            offset: 0,
            shared: true,
            writable: true,
        });
        assert_eq!(shared, Err(libc::EACCES));
        let private = maps
            .map(Request {
                fd: Some(file.as_raw_fd()),
                len: 3,
                offset: 0,
                shared: false,
                writable: true,
            })
            .expect("a private mapping of a read-only file");
        assert!(maps.write(private, 0, b"z"));
        assert_eq!(maps.read(private, 0, 3).as_deref(), Some(&b"zbc"[..]));
        assert_eq!(std::fs::read(&path).expect("read"), b"abc");
        drop(maps);
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
