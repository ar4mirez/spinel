//! The keys `Object#hash` is computed with: random for each process, the
//! same for every Ractor in it (CVE-2011-4815, which is why CRuby seeds its
//! hash too).
//!
//! A `RandomState` made once and never changed, so it is immutable shared
//! state of the kind this directory exists for: every heap that reads it
//! reads the same keys, and a `hash` answered in one Ractor agrees with the
//! same object's in another, as CRuby's does within a process.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher as _;
use std::sync::OnceLock;

static SEED: OnceLock<RandomState> = OnceLock::new();

/// A hasher with this process's keys.
pub fn hasher() -> impl std::hash::Hasher {
    SEED.get_or_init(RandomState::new).build_hasher()
}
