//! The require map (#66): `.spinel/require_map.bin`, what every directory
//! under every installed gem's `require_paths` and the standard library held
//! when `spinel install` last ran, so `require` can tell that a load path
//! entry does not have a file without asking the file system.
//!
//! # What the map answers, and why it is never wrong
//!
//! The map is a cache of directory listings, not of `require`'s answers.
//! `require` still walks `$LOAD_PATH` in its order, as it is when it looks,
//! and asks the map only "does this entry have this file?" ([`Probe`]). An
//! entry the map does not list — the program's own directories, `-I`, a gem
//! installed some other way — is looked at on disk as before. So the map
//! cannot change which entry wins, and a program that edits `$LOAD_PATH`
//! gets what it asked for.
//!
//! Each directory is listed with a [`Stamp`]: its device, inode, and
//! modification and change times. Adding, removing or renaming anything in
//! a directory changes its times, and replacing the directory changes its
//! inode, so the directory is looked at the first time a process asks about
//! it, and again every time the map would answer that it does not have a
//! file, and the answer is given only if the stamp is the same. A directory
//! that changed, or is gone, answers [`Probe::Unknown`], a normal look on
//! disk, from then on.
//!
//! A file system keeps times in ticks, so a change in the same tick as the
//! listing can leave the times as they were: git's "racily clean" index
//! entries. A directory whose times are within [`racy_margin`] of when it was
//! listed is therefore listed again when the map is first asked about it,
//! and answers only if the listing is the same. `build` waits out a short
//! margin rather than leave a just-made directory racy.
//!
//! The map answers only for ASCII names: a file system may normalize the
//! rest, and macOS is handed a path transcoded from its String's encoding.
//! And a name the map does not have is "absent" only if no entry has it in
//! another case: on a case-insensitive file system (macOS's default, a
//! `casefold` directory on Linux) `Foo.rb` opens `foo.rb`. Any other name
//! is a look on disk.
//!
//! The file is checked as a bytecode cache file is: magic, format, payload
//! length and SHA-256, and then every length, kind and index as it is read.
//! A map that fails any of them is not used.

use std::collections::HashMap;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The map's name in `.spinel/`, beside `gem_home/`.
pub const FILE_NAME: &str = "require_map.bin";

/// The first bytes of every map.
const MAGIC: &[u8; 8] = b"SPINELRM";

/// The layout below. A map of another format is not read.
const FORMAT: u32 = 1;

/// How long `build` waits for directories made just before it to stop being
/// racy, at most. Longer than that and they are left racy, listed again by
/// the first process that asks about them.
const MAX_WAIT: Duration = Duration::from_millis(250);

/// What a directory was when it was listed. Two stamps are equal only if
/// nothing was added to, removed from or renamed in the directory between
/// them, or the directory replaced — up to the file system's tick, which
/// [`racy_margin`] covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    /// The stamp of the directory at `path`, not following a symlink; None
    /// if it is not a directory.
    fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.file_type().is_dir() {
            return None;
        }
        Some(Stamp {
            dev: meta.dev(),
            ino: meta.ino(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        })
    }

    /// The later of the two times.
    fn changed(&self) -> (i64, i64) {
        self.mtime.max(self.ctime)
    }

    /// Whether a change after `listed` could have left this stamp as it is.
    fn racy(&self, listed: (i64, i64)) -> bool {
        let margin = racy_margin(self);
        let (secs, nanos) = self.changed();
        let until = (
            secs + margin.as_secs() as i64
                + (nanos + i64::from(margin.subsec_nanos())) / 1_000_000_000,
            (nanos + i64::from(margin.subsec_nanos())) % 1_000_000_000,
        );
        until >= listed
    }
}

/// How far apart a change and the time it is stamped with can be: a tick of
/// the kernel's coarse clock where times have nanoseconds (Linux's is at
/// most 10 ms; this is ten of them), and two seconds where they do not,
/// which is FAT's tick and more than ext3's or HFS+'s.
fn racy_margin(stamp: &Stamp) -> Duration {
    if stamp.mtime.1 == 0 && stamp.ctime.1 == 0 {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(100)
    }
}

fn now() -> (i64, i64) {
    let since = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (since.as_secs() as i64, i64::from(since.subsec_nanos()))
}

/// What a directory entry is, as `readdir` says without following a
/// symlink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    /// A symlink, a device, a socket: something `File.file?` must be asked
    /// about, since the map cannot see what it leads to.
    Other,
}

impl Kind {
    fn byte(self) -> u8 {
        match self {
            Kind::File => 0,
            Kind::Dir => 1,
            Kind::Other => 2,
        }
    }

    fn from_byte(byte: u8) -> Option<Kind> {
        match byte {
            0 => Some(Kind::File),
            1 => Some(Kind::Dir),
            2 => Some(Kind::Other),
            _ => None,
        }
    }
}

/// The entries of the directory at `path`, sorted by name.
fn list(path: &Path) -> io::Result<Vec<(Vec<u8>, Kind)>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = match entry.file_type()? {
            t if t.is_file() => Kind::File,
            t if t.is_dir() => Kind::Dir,
            _ => Kind::Other,
        };
        entries.push((entry.file_name().as_bytes().to_vec(), kind));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// One directory as `build` lists it.
struct Listed {
    stamp: Stamp,
    listed: (i64, i64),
    /// Name, kind, and for a directory the index of its own listing.
    entries: Vec<(Vec<u8>, Kind, u32)>,
}

struct Builder {
    nodes: Vec<Listed>,
    index: HashMap<PathBuf, u32>,
}

impl Builder {
    /// List the directory at `path` and everything under it, answering its
    /// index; None if it is not a directory or cannot be read, and then its
    /// parent lists it as something to look at on disk.
    fn walk(&mut self, path: &Path) -> Option<u32> {
        if let Some(&at) = self.index.get(path) {
            return Some(at);
        }
        // The time is taken before the stamp, and the stamp before the
        // listing: a change after the time either moves the stamp or makes
        // the directory racy, and one between the stamp and the listing
        // moves the stamp, so the directory is checked on disk.
        let listed = now();
        let stamp = Stamp::of(path)?;
        let names = list(path).ok()?;
        let at = u32::try_from(self.nodes.len()).ok()?;
        self.index.insert(path.to_owned(), at);
        self.nodes.push(Listed {
            stamp,
            listed,
            entries: Vec::new(),
        });
        let mut entries = Vec::with_capacity(names.len());
        for (name, kind) in names {
            let (kind, child) = match kind {
                Kind::Dir => match self.walk(&path.join(std::ffi::OsStr::from_bytes(&name))) {
                    Some(child) => (Kind::Dir, child),
                    None => (Kind::Other, 0),
                },
                kind => (kind, 0),
            };
            entries.push((name, kind, child));
        }
        self.nodes[at as usize].entries = entries;
        Some(at)
    }

    /// The latest time a racy directory's stamp could be in the same tick
    /// as a change, if any directory is racy.
    fn racy_until(&self) -> Option<SystemTime> {
        self.nodes
            .iter()
            .filter(|node| node.stamp.racy(node.listed))
            .map(|node| {
                let (secs, nanos) = node.stamp.changed();
                let at = Duration::new(secs.max(0) as u64, nanos.clamp(0, 999_999_999) as u32);
                SystemTime::UNIX_EPOCH + at + racy_margin(&node.stamp)
            })
            .max()
    }
}

/// The roots as they are on disk: each one resolved through its symlinks,
/// as `require` resolves its load path entries, and the ones that are not
/// directories left out.
fn listed_roots(roots: &[PathBuf]) -> (Builder, Vec<(Vec<u8>, u32)>) {
    let mut builder = Builder {
        nodes: Vec::new(),
        index: HashMap::new(),
    };
    let mut listed = Vec::new();
    for root in roots {
        let Ok(real) = std::fs::canonicalize(root) else {
            continue;
        };
        if let Some(at) = builder.walk(&real) {
            let bytes = real.as_os_str().as_bytes().to_vec();
            if !listed.iter().any(|(path, _)| *path == bytes) {
                listed.push((bytes, at));
            }
        }
    }
    (builder, listed)
}

/// The map of every directory under `roots`, encoded. Directories changed
/// just before this are waited for, up to [`MAX_WAIT`], and listed again.
pub fn build(roots: &[PathBuf]) -> Vec<u8> {
    let (mut builder, mut listed) = listed_roots(roots);
    if let Some(until) = builder.racy_until() {
        // A margin already past needs no wait, only the second listing.
        let wait = until
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        if wait <= MAX_WAIT {
            std::thread::sleep(wait + Duration::from_millis(1));
            (builder, listed) = listed_roots(roots);
        }
    }
    encode(&builder, &listed)
}

fn encode(builder: &Builder, roots: &[(Vec<u8>, u32)]) -> Vec<u8> {
    let mut payload = Vec::new();
    put_u32(&mut payload, roots.len() as u32);
    for (path, node) in roots {
        put_bytes(&mut payload, path);
        put_u32(&mut payload, *node);
    }
    put_u32(&mut payload, builder.nodes.len() as u32);
    for node in &builder.nodes {
        put_u64(&mut payload, node.stamp.dev);
        put_u64(&mut payload, node.stamp.ino);
        put_time(&mut payload, node.stamp.mtime);
        put_time(&mut payload, node.stamp.ctime);
        put_time(&mut payload, node.listed);
        put_u32(&mut payload, node.entries.len() as u32);
        for (name, kind, child) in &node.entries {
            put_bytes(&mut payload, name);
            payload.push(kind.byte());
            if *kind == Kind::Dir {
                put_u32(&mut payload, *child);
            }
        }
    }
    let mut out = Vec::with_capacity(payload.len() + 52);
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, FORMAT);
    put_u64(&mut out, payload.len() as u64);
    out.extend_from_slice(&Sha256::digest(&payload));
    out.extend_from_slice(&payload);
    out
}

/// Build the map of `roots` and write it to `path`, through a temporary
/// file renamed into place, so a program starting meanwhile reads the old
/// map, the new one, or none.
pub fn write(path: &Path, roots: &[PathBuf]) -> io::Result<()> {
    let bytes = build(roots);
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let temporary = dir.join(format!(".{FILE_NAME}.{}.{nanos}.tmp", std::process::id()));
    let written =
        std::fs::write(&temporary, &bytes).and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_time(out: &mut Vec<u8>, (secs, nanos): (i64, i64)) {
    out.extend_from_slice(&secs.to_le_bytes());
    out.extend_from_slice(&nanos.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// What the map says about a file in a load path entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// The entry has it, a regular file: `File.file?` would say true.
    Present,
    /// The entry does not have it: `File.file?` would say false.
    Absent,
    /// The map does not know: look on disk.
    Unknown,
}

/// Whether a directory's listing has been checked against the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    Unchecked,
    Fresh,
    Stale,
}

/// One directory of a read map. Its entries are ranges of the map's bytes.
struct Node {
    stamp: Stamp,
    listed: (i64, i64),
    /// Name (start, length), kind, and a directory's node, sorted by name.
    entries: Vec<(u32, u32, Kind, u32)>,
    /// The names ASCII-folded to lower case, sorted: what a case-insensitive
    /// file system could take each name for.
    folded: Vec<Vec<u8>>,
    check: Check,
}

/// A map read back, as one process uses it.
pub struct RequireMap {
    data: Vec<u8>,
    roots: HashMap<Vec<u8>, u32>,
    nodes: Vec<Node>,
}

/// Read the map at `path`. None if there is none, or it is not one this
/// build wrote in a form it reads, or it is damaged.
pub fn load(path: &Path) -> Option<RequireMap> {
    decode(std::fs::read(path).ok()?)
}

fn decode(bytes: Vec<u8>) -> Option<RequireMap> {
    let header = MAGIC.len() + 4 + 8 + 32;
    if bytes.len() < header || &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    let mut reader = Reader {
        bytes: &bytes,
        at: MAGIC.len(),
    };
    if reader.u32()? != FORMAT {
        return None;
    }
    let length = usize::try_from(reader.u64()?).ok()?;
    let digest = reader.take(32)?.to_vec();
    if bytes.len() - header != length || Sha256::digest(&bytes[header..])[..] != digest[..] {
        return None;
    }
    let count = reader.u32()? as usize;
    let mut root_list = Vec::new();
    for _ in 0..count {
        let path = reader.bytes()?.to_vec();
        let node = reader.u32()?;
        root_list.push((path, node));
    }
    let count = reader.u32()?;
    let mut nodes = Vec::new();
    for _ in 0..count {
        let stamp = Stamp {
            dev: reader.u64()?,
            ino: reader.u64()?,
            mtime: reader.time()?,
            ctime: reader.time()?,
        };
        let listed = reader.time()?;
        let entry_count = reader.u32()?;
        let mut entries = Vec::new();
        let mut folded = Vec::new();
        for _ in 0..entry_count {
            let length = reader.u32()?;
            let start = u32::try_from(reader.at).ok()?;
            let name = reader.take(length as usize)?;
            folded.push(name.to_ascii_lowercase());
            let kind = Kind::from_byte(reader.u8()?)?;
            let child = if kind == Kind::Dir {
                let child = reader.u32()?;
                if child >= count {
                    return None;
                }
                child
            } else {
                0
            };
            entries.push((start, length, kind, child));
        }
        // Sorted as written, or the lookups below would miss.
        let names = |e: &(u32, u32, Kind, u32)| &bytes[e.0 as usize..(e.0 + e.1) as usize];
        if !entries
            .windows(2)
            .all(|pair| names(&pair[0]) < names(&pair[1]))
        {
            return None;
        }
        folded.sort();
        nodes.push(Node {
            stamp,
            listed,
            entries,
            folded,
            check: Check::Unchecked,
        });
    }
    if reader.at != bytes.len() || root_list.iter().any(|(_, node)| *node >= count) {
        return None;
    }
    let roots = root_list.into_iter().collect();
    Some(RequireMap {
        data: bytes,
        roots,
        nodes,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn time(&mut self) -> Option<(i64, i64)> {
        Some((self.i64()?, self.i64()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let length = self.u32()? as usize;
        self.take(length)
    }
}

impl RequireMap {
    /// Whether `dir`, an expanded load path entry, is a directory the map
    /// lists.
    pub fn covers(&self, dir: &[u8]) -> bool {
        self.roots.contains_key(dir)
    }

    /// Whether the load path entry `dir` has `file`, a name relative to it
    /// as `require` makes one: what `File.file?` of `File.expand_path(file,
    /// dir)` would say, or [`Probe::Unknown`] when the map cannot tell.
    pub fn probe(&mut self, dir: &[u8], file: &[u8]) -> Probe {
        let Some(&root) = self.roots.get(dir) else {
            return Probe::Unknown;
        };
        // Only names `expand_path` leaves as they are: no empty, `.` or `..`
        // component, and no NUL, which the look on disk refuses its own way.
        // And only ASCII ones: a file system may normalize the rest, and on
        // macOS a path is handed over transcoded from its String's encoding,
        // so its bytes there need not be the String's.
        let parts: Vec<&[u8]> = file.split(|&b| b == b'/').collect();
        if !file.is_ascii()
            || file.contains(&0)
            || parts
                .iter()
                .any(|p| p.is_empty() || *p == b"." || *p == b"..")
        {
            return Probe::Unknown;
        }
        let mut node = root;
        let mut path = dir.to_vec();
        let (last, dirs) = parts.split_last().expect("split yields one part at least");
        let answer = 'walk: {
            for part in dirs {
                if !self.fresh(node, &path) {
                    return Probe::Unknown;
                }
                match self.lookup(node, part) {
                    Some((Kind::Dir, child)) => {
                        node = child;
                        path.push(b'/');
                        path.extend_from_slice(part);
                    }
                    // A file where a directory would be: nothing under it.
                    Some((Kind::File, _)) => break 'walk Probe::Absent,
                    Some((Kind::Other, _)) => return Probe::Unknown,
                    None => break 'walk self.absent(node, part),
                }
            }
            if !self.fresh(node, &path) {
                return Probe::Unknown;
            }
            match self.lookup(node, last) {
                Some((Kind::File, _)) => Probe::Present,
                Some((Kind::Dir, _)) => Probe::Absent,
                Some((Kind::Other, _)) => Probe::Unknown,
                None => self.absent(node, last),
            }
        };
        // A file added since the directory was first looked at would be
        // missed by an absent answer alone, so the directory that gave it is
        // looked at again.
        if answer == Probe::Absent && !self.check(node, &path) {
            return Probe::Unknown;
        }
        answer
    }

    fn lookup(&self, node: u32, name: &[u8]) -> Option<(Kind, u32)> {
        let data = &self.data;
        let entries = &self.nodes[node as usize].entries;
        let at = entries
            .binary_search_by(|e| data[e.0 as usize..(e.0 + e.1) as usize].cmp(name))
            .ok()?;
        let (_, _, kind, child) = entries[at];
        Some((kind, child))
    }

    /// A name the directory does not have: absent, unless a case-insensitive
    /// file system could take it for one it does have.
    fn absent(&self, node: u32, name: &[u8]) -> Probe {
        let folded = name.to_ascii_lowercase();
        if self.nodes[node as usize]
            .folded
            .binary_search(&folded)
            .is_ok()
        {
            Probe::Unknown
        } else {
            Probe::Absent
        }
    }

    /// Whether `node`'s listing still holds for the directory at `path`,
    /// checked the first time it is asked.
    fn fresh(&mut self, node: u32, path: &[u8]) -> bool {
        match self.nodes[node as usize].check {
            Check::Fresh => true,
            Check::Stale => false,
            Check::Unchecked => self.check(node, path),
        }
    }

    /// Look at the directory at `path` now: whether `node`'s listing still
    /// holds for it. One that does not is [`Check::Stale`] for good.
    fn check(&mut self, node: u32, path: &[u8]) -> bool {
        let path = Path::new(std::ffi::OsStr::from_bytes(path));
        let stored = &self.nodes[node as usize];
        let mut fresh = Stamp::of(path) == Some(stored.stamp);
        if fresh && stored.stamp.racy(stored.listed) {
            // Listed again as of now, so a later look need not list it again
            // once a change after now could no longer keep the stamp.
            let at = now();
            fresh = self.same_listing(node, path);
            if fresh {
                self.nodes[node as usize].listed = at;
            }
        }
        self.nodes[node as usize].check = if fresh { Check::Fresh } else { Check::Stale };
        fresh
    }

    /// Whether the directory at `path` lists what `node` does.
    fn same_listing(&self, node: u32, path: &Path) -> bool {
        let Ok(names) = list(path) else {
            return false;
        };
        let entries = &self.nodes[node as usize].entries;
        names.len() == entries.len()
            && names.iter().zip(entries).all(|((name, kind), e)| {
                // A directory the map could not list is kept as `Other`.
                let same_kind = *kind == e.2 || (*kind == Kind::Dir && e.2 == Kind::Other);
                same_kind && name[..] == self.data[e.0 as usize..(e.0 + e.1) as usize]
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "spinel-require-map-{name}-{}-{}",
            std::process::id(),
            now().1
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn map_of(roots: &[PathBuf]) -> RequireMap {
        decode(build(roots)).expect("a map just built reads back")
    }

    fn real(path: &Path) -> Vec<u8> {
        std::fs::canonicalize(path)
            .unwrap()
            .as_os_str()
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn answers_for_what_it_listed() {
        let dir = scratch("listed");
        std::fs::create_dir_all(dir.join("lib/foo")).unwrap();
        std::fs::write(dir.join("lib/foo.rb"), "").unwrap();
        std::fs::write(dir.join("lib/foo/bar.rb"), "").unwrap();
        let lib = dir.join("lib");
        let mut map = map_of(std::slice::from_ref(&lib));
        let root = real(&lib);
        assert_eq!(map.probe(&root, b"foo.rb"), Probe::Present);
        assert_eq!(map.probe(&root, b"foo/bar.rb"), Probe::Present);
        assert_eq!(map.probe(&root, b"foo"), Probe::Absent);
        assert_eq!(map.probe(&root, b"baz.rb"), Probe::Absent);
        assert_eq!(map.probe(&root, b"baz/bar.rb"), Probe::Absent);
        assert_eq!(map.probe(&root, b"foo.rb/x.rb"), Probe::Absent);
        assert_eq!(map.probe(&root, b"./foo.rb"), Probe::Unknown);
        assert_eq!(map.probe(&root, b"foo//bar.rb"), Probe::Unknown);
        // Another case, or a name a file system may normalize: on disk.
        assert_eq!(map.probe(&root, b"FOO.rb"), Probe::Unknown);
        assert_eq!(map.probe(&root, "fóo.rb".as_bytes()), Probe::Unknown);
        // Not a root.
        assert_eq!(
            map.probe(dir.as_os_str().as_bytes(), b"lib/foo.rb"),
            Probe::Unknown
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_changed_directory_is_looked_at_on_disk() {
        let dir = scratch("changed");
        std::fs::create_dir_all(dir.join("lib/foo")).unwrap();
        let lib = dir.join("lib");
        let bytes = build(std::slice::from_ref(&lib));
        std::fs::write(dir.join("lib/foo/new.rb"), "").unwrap();
        let mut map = decode(bytes).unwrap();
        let root = real(&lib);
        assert_eq!(map.probe(&root, b"foo/new.rb"), Probe::Unknown);
        assert_eq!(map.probe(&root, b"other.rb"), Probe::Absent);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_added_after_an_absent_answer_is_looked_at_on_disk() {
        let dir = scratch("added");
        std::fs::create_dir_all(dir.join("lib/foo")).unwrap();
        let lib = dir.join("lib");
        let mut map = map_of(std::slice::from_ref(&lib));
        let root = real(&lib);
        assert_eq!(map.probe(&root, b"new.rb"), Probe::Absent);
        assert_eq!(map.probe(&root, b"foo/new.rb"), Probe::Absent);
        std::fs::write(lib.join("foo/new.rb"), "").unwrap();
        assert_eq!(map.probe(&root, b"foo/new.rb"), Probe::Unknown);
        assert_eq!(map.probe(&root, b"foo/other.rb"), Probe::Unknown);
        assert_eq!(map.probe(&root, b"other.rb"), Probe::Absent);
        std::fs::write(lib.join("new.rb"), "").unwrap();
        assert_eq!(map.probe(&root, b"new.rb"), Probe::Unknown);
        assert_eq!(map.probe(&root, b"other.rb"), Probe::Unknown);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_racy_directory_is_listed_again() {
        let dir = scratch("racy");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        let lib = dir.join("lib");
        // A stamp as if listed in the same tick as the last change.
        let (mut builder, roots) = listed_roots(std::slice::from_ref(&lib));
        builder.nodes[0].listed = builder.nodes[0].stamp.changed();
        let mut map = decode(encode(&builder, &roots)).unwrap();
        let root = real(&lib);
        assert_eq!(map.probe(&root, b"a.rb"), Probe::Absent);
        let (mut builder, roots) = listed_roots(std::slice::from_ref(&lib));
        builder.nodes[0].listed = builder.nodes[0].stamp.changed();
        let bytes = encode(&builder, &roots);
        // Same stamp, as a change in the same tick would leave it, but a
        // different listing.
        let stamp = Stamp::of(&lib).unwrap();
        std::fs::write(lib.join("a.rb"), "").unwrap();
        let mut map = decode(bytes).unwrap();
        map.nodes[0].stamp = Stamp::of(&lib).unwrap();
        assert_ne!(map.nodes[0].stamp, stamp);
        assert_eq!(map.probe(&root, b"a.rb"), Probe::Unknown);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_map_is_not_read() {
        let dir = scratch("damaged");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/a.rb"), "").unwrap();
        let bytes = build(&[dir.join("lib")]);
        assert!(decode(bytes.clone()).is_some());
        for at in [0, 9, 20, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            assert!(decode(bad).is_none(), "byte {at} flipped");
        }
        assert!(decode(bytes[..bytes.len() - 1].to_vec()).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
