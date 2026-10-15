//! The vendored pure-Ruby standard library, at runtime (#47).
//!
//! `build.rs` packs `stdlib/` into the binary. The first run extracts it to
//! `~/.spinel/stdlib/<version>-<digest>/` (`$SPINEL_HOME` in place of
//! `~/.spinel` when set), and every run puts that directory on `$LOAD_PATH`
//! after the program's own `-I` and `RUBYLIB` ones, where CRuby has its
//! `rubylibdir`. The digest is the archive's, so a binary never reads a copy
//! another build extracted.
//!
//! First runs may race, so a copy is written to a directory of its own and
//! renamed into place: `rename(2)` makes it appear whole or not at all, and
//! the run that loses the race uses the winner's and removes its own.

use std::io::Write as _;
use std::path::{Path, PathBuf};

const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/stdlib.zst"));
const DIGEST: &str = include_str!(concat!(env!("OUT_DIR"), "/stdlib.digest"));

/// Where Spinel keeps what it writes for the user.
fn spinel_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("SPINEL_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(home));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".spinel"))
}

/// The extracted standard library, extracting it first if no copy is there.
/// None when it can be neither found nor written; a program that requires
/// none of it runs as before.
pub fn directory() -> Option<PathBuf> {
    let root = spinel_home()?.join("stdlib");
    let target = root.join(format!("{}-{DIGEST}", env!("CARGO_PKG_VERSION")));
    if target.is_dir() {
        return Some(target);
    }
    match extract(&root, &target) {
        Ok(()) => Some(target),
        Err(_) => target.is_dir().then_some(target),
    }
}

fn extract(root: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let staging = root.join(format!(".extract-{}-{nanos}", std::process::id()));
    let written = write_archive(&staging);
    let placed = written.and_then(|()| std::fs::rename(&staging, target));
    if placed.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        // Another run placed its copy first: that one is as good.
        if target.is_dir() {
            return Ok(());
        }
    }
    placed
}

fn write_archive(into: &Path) -> std::io::Result<()> {
    let archive = zstd::decode_all(ARCHIVE)?;
    let mut rest: &[u8] = &archive;
    std::fs::create_dir_all(into)?;
    while !rest.is_empty() {
        let len = take(&mut rest, 4)?;
        let name_len = u32::from_le_bytes(len.try_into().expect("four bytes")) as usize;
        let name = std::str::from_utf8(take(&mut rest, name_len)?).map_err(|_| corrupt())?;
        let size =
            u64::from_le_bytes(take(&mut rest, 8)?.try_into().expect("eight bytes")) as usize;
        let bytes = take(&mut rest, size)?;
        // The archive is the build's own, but a path that leaves the
        // directory is refused all the same.
        let relative = Path::new(name);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(corrupt());
        }
        let path = into.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create(&path)?.write_all(bytes)?;
    }
    Ok(())
}

fn corrupt() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "a truncated stdlib archive",
    )
}

/// The next `n` bytes of `rest`, which then starts after them.
fn take<'a>(rest: &mut &'a [u8], n: usize) -> std::io::Result<&'a [u8]> {
    if rest.len() < n {
        return Err(corrupt());
    }
    let (bytes, after) = rest.split_at(n);
    *rest = after;
    Ok(bytes)
}
