//! Where a compiled app's payload sits on macOS (docs/build.md, "Format"; #99).
//!
//! Appending to a signed Mach-O invalidates its signature, so on macOS the
//! payload and its trailer go inside the binary, in a `__SPINEL,__payload`
//! section, and the result is signed again ad hoc. The section's bytes are
//! exactly what Linux appends — payload, then the 48-byte trailer — so one
//! parser, `trailer::read`, reads both.
//!
//! The section is reserved when `spinel` is compiled, by `RESERVED` below:
//! a few zero bytes, which hold no trailer, so a plain `spinel` is no app.
//! The linker puts the `__SPINEL` segment where every custom segment goes:
//! after `__DATA` and directly before `__LINKEDIT`, which is last. Writing a payload grows
//! that segment and moves `__LINKEDIT`, and every file offset that points into
//! it, along by the same whole number of pages. Nothing outside `__LINKEDIT`
//! moves, so no code or data address changes and nothing needs relocating.
//!
//! Only thin 64-bit little-endian Mach-O is handled: what `cargo build` makes
//! for `aarch64-apple-darwin` and `x86_64-apple-darwin`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use crate::trailer::Trailer;

/// `MH_MAGIC_64`, as its little-endian bytes start the file.
const MAGIC_64: u32 = 0xfeed_facf;
/// The `mach_header_64` that precedes the load commands.
const HEADER_LEN: usize = 32;

const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const LC_DYSYMTAB: u32 = 0xb;
const LC_DYLD_INFO: u32 = 0x22;
const LC_DYLD_INFO_ONLY: u32 = 0x8000_0022;
const LC_NOTE: u32 = 0x31;

/// The `linkedit_data_command`s: `dataoff` at byte 8 is a file offset into
/// `__LINKEDIT`.
const LINKEDIT_DATA: [u32; 10] = [
    0x1d,        // LC_CODE_SIGNATURE
    0x1e,        // LC_SEGMENT_SPLIT_INFO
    0x26,        // LC_FUNCTION_STARTS
    0x29,        // LC_DATA_IN_CODE
    0x2b,        // LC_DYLIB_CODE_SIGN_DRS
    0x2e,        // LC_LINKER_OPTIMIZATION_HINT
    0x8000_0033, // LC_DYLD_EXPORTS_TRIE
    0x8000_0034, // LC_DYLD_CHAINED_FIXUPS
    0x36,        // LC_ATOMIC_INFO
    0x37,        // LC_FUNCTION_VARIANTS
];

/// The section a payload is written into, reserved in every macOS `spinel`.
/// `#[used]` keeps it through `-dead_strip`, though nothing reads it from
/// memory: the payload is read from the file, where `place` finds it.
#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__SPINEL,__payload")]
static RESERVED: [u8; 16] = [0; 16];

/// The segment and section `RESERVED` is in.
pub const SEGMENT: &str = "__SPINEL";
pub const SECTION: &str = "__payload";
const LINKEDIT: &str = "__LINKEDIT";

/// The page the segment is rounded to: arm64's 16 KiB, a whole number of
/// x86_64's 4 KiB pages too.
const PAGE: u64 = 0x4000;

/// What a file holds where a payload would be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Not a Mach-O: the payload, if any, is appended (Linux).
    NotMachO,
    /// A Mach-O other than thin 64-bit little-endian: universal (fat),
    /// 32-bit, or big-endian. It holds no payload, and none can be written.
    Unsupported,
    /// A Mach-O with no `__SPINEL,__payload` section, so no payload.
    NoSection,
    /// The section's bytes are at `offset` in the file, `size` of them.
    Section { offset: u64, size: u64 },
}

/// Where in `file` the payload section is. Reads the header and the load
/// commands, a few kilobytes, and nothing of the payload itself.
pub fn place<F: Read + Seek>(file: &mut F) -> io::Result<Place> {
    let Some(image) = Image::read(file)? else {
        return other_macho(file).map(|other| {
            if other {
                Place::Unsupported
            } else {
                Place::NotMachO
            }
        });
    };
    Ok(match image.section() {
        Ok(found) => Place::Section {
            offset: u64::from(found.offset),
            size: found.size,
        },
        Err(_) => Place::NoSection,
    })
}

/// Whether `file` starts with a Mach-O magic `Image::read` does not take:
/// universal (`FAT_MAGIC`, `FAT_MAGIC_64`), 32-bit (`MH_MAGIC`), or
/// big-endian (`MH_CIGAM`, `MH_CIGAM_64`).
fn other_macho<F: Read + Seek>(file: &mut F) -> io::Result<bool> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0; 4];
    if let Err(err) = file.read_exact(&mut magic) {
        return match err.kind() {
            io::ErrorKind::UnexpectedEof => Ok(false),
            _ => Err(err),
        };
    }
    Ok(matches!(
        magic,
        [0xca, 0xfe, 0xba, 0xbe]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xce]
            | [0xfe, 0xed, 0xfa, 0xcf]
    ))
}

/// Write a compiled app for macOS: `binary`, a Mach-O with the reserved
/// section, with `payload` and its trailer in that section, to `out`, then
/// signed again ad hoc with `codesign`, keeping `binary`'s permissions.
pub fn write(binary: &Path, payload: &[u8], out: &Path) -> io::Result<()> {
    embed(binary, payload, out)?;
    sign_ad_hoc(out)
}

/// `write` without the signing: the bytes of the app, whose signature, if
/// `binary` had one, no longer matches them.
pub fn embed(binary: &Path, payload: &[u8], out: &Path) -> io::Result<()> {
    let mut input = File::open(binary)?;
    let permissions = input.metadata()?.permissions();
    let file_len = input.seek(SeekFrom::End(0))?;
    let Some(mut image) = Image::read(&mut input)? else {
        return Err(invalid(format!(
            "`{}` is not a 64-bit Mach-O executable",
            binary.display()
        )));
    };
    let plan = image.plan(file_len, payload.len() as u64)?;
    image.apply(&plan)?;

    // A new file rather than the old one rewritten: macOS caches what it
    // checked of a signed executable by file, and a binary changed in place
    // after it has run is killed at its next start.
    match std::fs::remove_file(out) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)?;
    // The header and load commands, rewritten.
    output.write_all(&image.head)?;
    // Everything up to the section, as it was.
    input.seek(SeekFrom::Start(image.head.len() as u64))?;
    copy_exact(
        &mut input,
        &mut output,
        plan.section_offset - image.head.len() as u64,
    )?;
    // The section: the payload, then its trailer, as Linux appends them.
    output.write_all(payload)?;
    output.write_all(&Trailer::for_payload(payload).to_bytes())?;
    // Zeros to the end of the segment, where `__LINKEDIT` now starts.
    let written = plan.section_offset + plan.section_size;
    copy_exact(
        &mut io::repeat(0),
        &mut output,
        plan.new_linkedit_offset - written,
    )?;
    // `__LINKEDIT`, as it was, at its new offset.
    input.seek(SeekFrom::Start(plan.old_linkedit_offset))?;
    copy_exact(&mut input, &mut output, file_len - plan.old_linkedit_offset)?;
    output.sync_all()?;
    drop(output);
    std::fs::set_permissions(out, permissions)
}

/// Sign `path` ad hoc, replacing whatever signature it has, as Bun does.
// ponytail: shells out to Apple's `codesign`, so an app for macOS is built on
// macOS. Cross-building one from Linux (`--target`, docs/build.md) needs an
// in-process ad hoc signer: a code directory of the pages' sha256s in an
// `LC_CODE_SIGNATURE` blob.
pub fn sign_ad_hoc(path: &Path) -> io::Result<()> {
    let out = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .output()
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("cannot run `codesign` to sign the app: {err}"),
            )
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "`codesign --force --sign - {}` failed: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Copy exactly `len` bytes, failing if `from` ends first.
fn copy_exact<R: Read, W: Write>(from: &mut R, to: &mut W, len: u64) -> io::Result<()> {
    let copied = io::copy(&mut from.take(len), to)?;
    if copied == len {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the binary ended while it was being copied",
        ))
    }
}

/// A Mach-O's header and load commands, as bytes to read and patch.
struct Image {
    /// `mach_header_64` then `sizeofcmds` bytes of load commands.
    head: Vec<u8>,
    /// Each load command: where it starts in `head`, and its `cmd`.
    commands: Vec<(usize, u32)>,
}

/// A segment's `segment_command_64`, by where it starts in `head`.
#[derive(Clone, Copy, Debug)]
struct Segment {
    at: usize,
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    nsects: u32,
}

/// A `section_64`, by where it starts in `head`.
#[derive(Clone, Copy, Debug)]
struct Section {
    at: usize,
    offset: u32,
    size: u64,
}

/// What `embed` changes, worked out before anything is written.
struct Plan {
    segment: Segment,
    linkedit: Segment,
    section_at: usize,
    section_offset: u64,
    section_size: u64,
    segment_size: u64,
    old_linkedit_offset: u64,
    new_linkedit_offset: u64,
}

impl Image {
    /// The header and load commands of `file`, or `None` when it does not
    /// start with the 64-bit Mach-O magic.
    fn read<F: Read + Seek>(file: &mut F) -> io::Result<Option<Image>> {
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_LEN];
        if let Err(err) = file.read_exact(&mut header) {
            return match err.kind() {
                io::ErrorKind::UnexpectedEof => Ok(None),
                _ => Err(err),
            };
        }
        if u32_at(&header, 0) != MAGIC_64 {
            return Ok(None);
        }
        let ncmds = u32_at(&header, 16) as usize;
        let sizeofcmds = u32_at(&header, 20) as usize;
        let mut head = header.to_vec();
        head.resize(HEADER_LEN + sizeofcmds, 0);
        file.read_exact(&mut head[HEADER_LEN..])?;
        let mut commands = Vec::with_capacity(ncmds);
        let mut at = HEADER_LEN;
        for _ in 0..ncmds {
            if at + 8 > head.len() {
                return Err(invalid("a load command runs past `sizeofcmds`".into()));
            }
            let cmd = u32_at(&head, at);
            let size = u32_at(&head, at + 4) as usize;
            if size < 8 || at + size > head.len() {
                return Err(invalid(format!("load command {cmd:#x} has a bad size")));
            }
            commands.push((at, cmd));
            at += size;
        }
        Ok(Some(Image { head, commands }))
    }

    fn segments(&self) -> impl Iterator<Item = (&[u8], Segment)> + '_ {
        self.commands
            .iter()
            .filter(|&&(_, cmd)| cmd == LC_SEGMENT_64)
            .map(|&(at, _)| {
                let head = &self.head;
                (
                    name(&head[at + 8..at + 24]),
                    Segment {
                        at,
                        vmaddr: u64_at(head, at + 24),
                        vmsize: u64_at(head, at + 32),
                        fileoff: u64_at(head, at + 40),
                        filesize: u64_at(head, at + 48),
                        nsects: u32_at(head, at + 64),
                    },
                )
            })
    }

    fn segment(&self, wanted: &str) -> Option<Segment> {
        self.segments()
            .find(|(segname, _)| *segname == wanted.as_bytes())
            .map(|(_, segment)| segment)
    }

    /// The reserved section: the only one in the `__SPINEL` segment.
    fn section(&self) -> Result<Section, String> {
        let segment = self
            .segment(SEGMENT)
            .ok_or_else(|| format!("it has no `{SEGMENT}` segment to hold a payload"))?;
        if segment.nsects != 1 {
            return Err(format!(
                "its `{SEGMENT}` segment has {} sections, not just `{SECTION}`",
                segment.nsects
            ));
        }
        let at = segment.at + 72;
        if at + 80 > self.head.len() {
            return Err(format!("its `{SEGMENT}` segment's section is cut short"));
        }
        if name(&self.head[at..at + 16]) != SECTION.as_bytes() {
            return Err(format!(
                "its `{SEGMENT}` segment has no `{SECTION}` section"
            ));
        }
        let section = Section {
            at,
            size: u64_at(&self.head, at + 40),
            offset: u32_at(&self.head, at + 48),
        };
        let offset = u64::from(section.offset);
        if offset < segment.fileoff || offset + section.size > segment.fileoff + segment.filesize {
            return Err(format!("its `{SECTION}` section lies outside its segment"));
        }
        Ok(section)
    }

    /// Check the layout `embed` relies on and work out the new one.
    fn plan(&self, file_len: u64, payload_len: u64) -> io::Result<Plan> {
        let refuse = |why: String| invalid(format!("cannot add a payload to this binary: {why}"));
        let section = self.section().map_err(refuse)?;
        let segment = self.segment(SEGMENT).expect("section() found it");
        let linkedit = self
            .segment(LINKEDIT)
            .ok_or_else(|| refuse(format!("it has no `{LINKEDIT}` segment")))?;
        if linkedit.fileoff + linkedit.filesize != file_len {
            return Err(refuse(format!(
                "`{LINKEDIT}` does not end the file, so it is a universal binary or has \
                 bytes after it"
            )));
        }
        if segment.fileoff + segment.filesize > linkedit.fileoff
            || segment.vmaddr + segment.vmsize > linkedit.vmaddr
        {
            return Err(refuse(format!("`{SEGMENT}` is not before `{LINKEDIT}`")));
        }
        // Nothing but `__SPINEL` may lie between the end of the rest and
        // `__LINKEDIT`, in the file or in memory, since both are moved.
        for (segname, other) in self.segments() {
            if other.at == segment.at || other.at == linkedit.at {
                continue;
            }
            let name = String::from_utf8_lossy(segname);
            if other.filesize > 0 && other.fileoff + other.filesize > segment.fileoff {
                return Err(refuse(format!(
                    "`{name}` lies after `{SEGMENT}` in the file"
                )));
            }
            if other.vmsize > 0 && other.vmaddr + other.vmsize > segment.vmaddr {
                return Err(refuse(format!("`{name}` lies after `{SEGMENT}` in memory")));
            }
        }
        let section_offset = u64::from(section.offset);
        let section_size = payload_len + crate::trailer::LEN as u64;
        let segment_size = (section_offset - segment.fileoff + section_size).div_ceil(PAGE) * PAGE;
        let new_linkedit_offset = segment.fileoff + segment_size;
        // Load commands hold file offsets as 32 bits.
        if new_linkedit_offset + linkedit.filesize > u64::from(u32::MAX) {
            return Err(invalid(format!(
                "a {payload_len} byte payload would make the app larger than the 4 GiB a \
                 Mach-O can address"
            )));
        }
        Ok(Plan {
            segment,
            linkedit,
            section_at: section.at,
            section_offset,
            section_size,
            segment_size,
            old_linkedit_offset: linkedit.fileoff,
            new_linkedit_offset,
        })
    }

    /// Rewrite the load commands for `plan`.
    fn apply(&mut self, plan: &Plan) -> io::Result<()> {
        let head = &mut self.head;
        // The segment grows to hold the section, in the file and in memory.
        put_u64(head, plan.segment.at + 32, plan.segment_size);
        put_u64(head, plan.segment.at + 48, plan.segment_size);
        put_u64(head, plan.section_at + 40, plan.section_size);
        // `__LINKEDIT` starts where it now ends.
        put_u64(
            head,
            plan.linkedit.at + 24,
            plan.segment.vmaddr + plan.segment_size,
        );
        put_u64(head, plan.linkedit.at + 40, plan.new_linkedit_offset);

        // Every file offset into `__LINKEDIT` moves with it. An offset of 0
        // is "none", and one before `__LINKEDIT` is not its to move.
        let old = plan.old_linkedit_offset;
        let new = plan.new_linkedit_offset;
        let shift32 = |head: &mut Vec<u8>, at: usize| -> io::Result<()> {
            let value = u64::from(u32_at(head, at));
            if value != 0 && value >= old {
                let moved = u32::try_from(value - old + new)
                    .map_err(|_| invalid("a moved file offset passes 4 GiB".into()))?;
                head[at..at + 4].copy_from_slice(&moved.to_le_bytes());
            }
            Ok(())
        };
        for &(at, cmd) in &self.commands {
            let fields: &[usize] = match cmd {
                LC_SYMTAB => &[8, 16],
                LC_DYSYMTAB => &[32, 40, 48, 56, 64, 72],
                LC_DYLD_INFO | LC_DYLD_INFO_ONLY => &[8, 16, 24, 32, 40],
                cmd if LINKEDIT_DATA.contains(&cmd) => &[8],
                LC_SEGMENT_64 => {
                    // A section's relocations (`reloff`) live in `__LINKEDIT`.
                    let nsects = u32_at(head, at + 64) as usize;
                    for i in 0..nsects {
                        shift32(head, at + 72 + i * 80 + 56)?;
                    }
                    &[]
                }
                LC_NOTE => {
                    let value = u64_at(head, at + 24);
                    if value != 0 && value >= old {
                        put_u64(head, at + 24, value - old + new);
                    }
                    &[]
                }
                _ => &[],
            };
            for &field in fields {
                shift32(head, at + field)?;
            }
        }
        Ok(())
    }
}

/// A fixed-width name field, without its trailing NULs.
fn name(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    &field[..end]
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// A small Mach-O laid out as the linker lays out `spinel`: `__PAGEZERO`,
    /// `__TEXT`, the reserved `__SPINEL` segment, then `__LINKEDIT` holding a
    /// symbol table, chained fixups and a code signature, each recognisable.
    pub(crate) fn fake_spinel() -> Vec<u8> {
        let page = PAGE as usize;
        let linkedit_off = 2 * page;
        let mut cmds = Vec::new();
        segment(&mut cmds, "__PAGEZERO", 0, 0x1_0000_0000, 0, 0, None);
        segment(
            &mut cmds,
            "__TEXT",
            0x1_0000_0000,
            PAGE,
            0,
            PAGE,
            Some(("__text", 0x1_0000_1000, 16, 0x1000)),
        );
        segment(
            &mut cmds,
            SEGMENT,
            0x1_0000_4000,
            PAGE,
            PAGE,
            PAGE,
            Some((SECTION, 0x1_0000_4000, 16, page as u32)),
        );
        segment(
            &mut cmds,
            LINKEDIT,
            0x1_0000_8000,
            PAGE,
            linkedit_off as u64,
            300,
            None,
        );
        // LC_SYMTAB: 2 symbols at +0, strings at +100.
        command(
            &mut cmds,
            LC_SYMTAB,
            &[linkedit_off as u32, 2, (linkedit_off + 100) as u32, 40],
        );
        // LC_DYLD_CHAINED_FIXUPS at +150, LC_CODE_SIGNATURE at +200.
        command(&mut cmds, 0x8000_0034, &[(linkedit_off + 150) as u32, 50]);
        command(&mut cmds, 0x1d, &[(linkedit_off + 200) as u32, 100]);
        // LC_MAIN: an offset into `__TEXT`, which nothing moves.
        command(&mut cmds, 0x8000_0028, &[0x1000, 0, 0, 0]);

        let mut file = Vec::new();
        file.extend_from_slice(&MAGIC_64.to_le_bytes());
        file.extend_from_slice(&0x0100_000c_u32.to_le_bytes()); // arm64
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&2u32.to_le_bytes()); // MH_EXECUTE
        file.extend_from_slice(&8u32.to_le_bytes()); // ncmds
        file.extend_from_slice(&(cmds.len() as u32).to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&cmds);
        file.resize(0x1000, 0);
        file.extend_from_slice(b"__text contents");
        file.resize(page, 0xcc);
        // What `RESERVED` holds: zeros, no magic.
        file.resize(linkedit_off, 0);
        for i in 0..300u32 {
            file.push((i % 251) as u8);
        }
        file
    }

    fn segment(
        cmds: &mut Vec<u8>,
        segname: &str,
        vmaddr: u64,
        vmsize: u64,
        fileoff: u64,
        filesize: u64,
        section: Option<(&str, u64, u64, u32)>,
    ) {
        let nsects = u32::from(section.is_some());
        cmds.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
        cmds.extend_from_slice(&(72 + 80 * nsects).to_le_bytes());
        cmds.extend_from_slice(&fixed(segname));
        for value in [vmaddr, vmsize, fileoff, filesize] {
            cmds.extend_from_slice(&value.to_le_bytes());
        }
        cmds.extend_from_slice(&[1, 0, 0, 0, 1, 0, 0, 0]); // maxprot, initprot
        cmds.extend_from_slice(&nsects.to_le_bytes());
        cmds.extend_from_slice(&0u32.to_le_bytes());
        if let Some((sectname, addr, size, offset)) = section {
            cmds.extend_from_slice(&fixed(sectname));
            cmds.extend_from_slice(&fixed(segname));
            cmds.extend_from_slice(&addr.to_le_bytes());
            cmds.extend_from_slice(&size.to_le_bytes());
            cmds.extend_from_slice(&offset.to_le_bytes());
            cmds.extend_from_slice(&[0; 28]);
        }
    }

    fn command(cmds: &mut Vec<u8>, cmd: u32, fields: &[u32]) {
        cmds.extend_from_slice(&cmd.to_le_bytes());
        cmds.extend_from_slice(&(8 + 4 * fields.len() as u32).to_le_bytes());
        for field in fields {
            cmds.extend_from_slice(&field.to_le_bytes());
        }
    }

    fn fixed(text: &str) -> [u8; 16] {
        let mut out = [0; 16];
        out[..text.len()].copy_from_slice(text.as_bytes());
        out
    }

    pub(crate) fn embedded(binary: &[u8], payload: &[u8]) -> Vec<u8> {
        // One directory per test thread, since tests run side by side.
        let thread = format!("{:?}", std::thread::current().id());
        let dir = std::env::temp_dir().join(format!(
            "spinel-macho-unit-{}-{}-{}",
            std::process::id(),
            thread.trim_start_matches("ThreadId(").trim_end_matches(')'),
            payload.len()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let input = dir.join("spinel");
        let out = dir.join("app");
        std::fs::write(&input, binary).expect("write");
        embed(&input, payload, &out).expect("embed");
        let written = std::fs::read(&out).expect("read");
        let _ = std::fs::remove_dir_all(&dir);
        written
    }

    #[test]
    fn a_plain_spinel_has_the_reserved_section() {
        let file = fake_spinel();
        let place = place(&mut Cursor::new(&file)).expect("reads");
        assert_eq!(
            place,
            Place::Section {
                offset: PAGE,
                size: 16
            }
        );
    }

    #[test]
    fn elf_and_short_files_are_not_macho() {
        let elf = b"\x7fELF\x02\x01\x01\0".repeat(8);
        assert_eq!(
            place(&mut Cursor::new(elf)).expect("reads"),
            Place::NotMachO
        );
        assert_eq!(
            place(&mut Cursor::new(b"\xcf\xfa")).expect("reads"),
            Place::NotMachO
        );
    }

    #[test]
    fn fat_32_bit_and_big_endian_macho_are_unsupported() {
        for magic in [
            [0xca, 0xfe, 0xba, 0xbe],
            [0xca, 0xfe, 0xba, 0xbf],
            [0xce, 0xfa, 0xed, 0xfe],
            [0xfe, 0xed, 0xfa, 0xce],
            [0xfe, 0xed, 0xfa, 0xcf],
        ] {
            let mut file = magic.to_vec();
            file.resize(64, 0);
            assert_eq!(
                place(&mut Cursor::new(&file)).expect("reads"),
                Place::Unsupported,
                "{magic:x?}"
            );
        }
    }

    #[test]
    fn a_macho_without_the_segment_has_no_section() {
        let mut file = fake_spinel();
        // Rename `__SPINEL` to `__SPINEX`.
        let at = file
            .windows(8)
            .position(|w| w == b"__SPINEL")
            .expect("segname");
        file[at + 7] = b'X';
        assert_eq!(
            place(&mut Cursor::new(&file)).expect("reads"),
            Place::NoSection
        );
    }

    #[test]
    fn the_section_holds_payload_then_trailer_and_linkedit_moves_whole() {
        let original = fake_spinel();
        let payload = vec![0x5a; 40_000];
        let app = embedded(&original, &payload);

        let Place::Section { offset, size } = place(&mut Cursor::new(&app)).expect("reads") else {
            panic!("no section");
        };
        assert_eq!(offset, PAGE);
        assert_eq!(size, 40_000 + 48);
        let section = &app[offset as usize..(offset + size) as usize];
        assert_eq!(&section[..40_000], &payload[..]);
        assert_eq!(section[40_000..], Trailer::for_payload(&payload).to_bytes());

        // 40,048 bytes round up to three 16 KiB pages.
        let image = Image::read(&mut Cursor::new(&app))
            .expect("reads")
            .expect("macho");
        let segment = image.segment(SEGMENT).expect("segment");
        assert_eq!((segment.filesize, segment.vmsize), (3 * PAGE, 3 * PAGE));
        let linkedit = image.segment(LINKEDIT).expect("linkedit");
        assert_eq!(linkedit.fileoff, 4 * PAGE);
        assert_eq!(linkedit.vmaddr, 0x1_0000_4000 + 3 * PAGE);
        assert_eq!(linkedit.filesize, 300);
        assert_eq!(app.len() as u64, 4 * PAGE + 300);
        assert_eq!(&app[4 * PAGE as usize..], &original[2 * PAGE as usize..]);

        // Offsets into `__LINKEDIT` moved by two pages; `LC_MAIN`'s did not.
        let moved = |cmd: u32, field: usize| {
            let &(at, _) = image
                .commands
                .iter()
                .find(|&&(_, c)| c == cmd)
                .expect("command");
            u32_at(&image.head, at + field)
        };
        let base = 4 * PAGE as u32;
        assert_eq!(moved(LC_SYMTAB, 8), base);
        assert_eq!(moved(LC_SYMTAB, 16), base + 100);
        assert_eq!(moved(0x8000_0034, 8), base + 150);
        assert_eq!(moved(0x1d, 8), base + 200);
        assert_eq!(moved(0x8000_0028, 8), 0x1000);

        // Everything before the section is as it was, load commands aside.
        assert_eq!(
            &app[0x1000..PAGE as usize],
            &original[0x1000..PAGE as usize]
        );
    }

    #[test]
    fn a_small_payload_keeps_the_segment_one_page() {
        let original = fake_spinel();
        let app = embedded(&original, b"hello");
        assert_eq!(app.len(), original.len());
        let image = Image::read(&mut Cursor::new(&app))
            .expect("reads")
            .expect("macho");
        assert_eq!(image.segment(LINKEDIT).expect("linkedit").fileoff, 2 * PAGE);
    }

    #[test]
    fn an_app_can_be_packed_again() {
        let once = embedded(&fake_spinel(), &vec![1; 50_000]);
        let twice = embedded(&once, b"smaller");
        let Place::Section { offset, size } = place(&mut Cursor::new(&twice)).expect("reads")
        else {
            panic!("no section");
        };
        assert_eq!(size, 7 + 48);
        assert_eq!(&twice[offset as usize..offset as usize + 7], b"smaller");
        assert_eq!(twice.len(), fake_spinel().len());
    }

    #[test]
    fn a_binary_without_the_segment_is_refused() {
        let mut file = fake_spinel();
        let at = file
            .windows(8)
            .position(|w| w == b"__SPINEL")
            .expect("segname");
        file[at + 7] = b'X';
        let dir = std::env::temp_dir().join(format!("spinel-macho-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let input = dir.join("spinel");
        std::fs::write(&input, &file).expect("write");
        let err = embed(&input, b"x", &dir.join("app")).expect_err("refused");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.to_string().contains("no `__SPINEL` segment"), "{err}");
    }
}
