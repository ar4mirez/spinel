//! The Spinel home: `$SPINEL_HOME` when set, else `~/.spinel`. The stdlib
//! copy, the compact index cache and (later) the gem store live under it.

use std::path::PathBuf;

/// Where Spinel keeps what it writes for the user.
pub fn spinel_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("SPINEL_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(home));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".spinel"))
}
