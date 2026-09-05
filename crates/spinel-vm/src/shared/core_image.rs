//! The compiled core library, built once per process.
//!
//! `core/*.rb` is Ruby, and every heap needs its methods defined in it: a method
//! table belongs to a heap. What no heap needs twice is the *parse and compile*.
//! The spec harness makes one heap per example and there are 25,624 of them, so
//! compiling per heap would multiply a corpus run by the cost of Prism.
//!
//! # Why this is allowed here
//!
//! `CLAUDE.md` forbids process-global mutable VM state and names this directory
//! as the exception: immutable, append-only tables. An [`Iseq`] qualifies more
//! cleanly than the symbol table does. It is written once, never mutated, and
//! holds no [`Value`] — a `Literal` is a *description* of a value, materialised
//! into whichever heap is running, which is the same property that lets phase
//! 3 cache bytecode on disk and phase 5 share it between Ractors. No Ractor can
//! reach another's objects through this.
//!
//! The compiling itself lives in `spinel-core`, because it needs a parser and
//! this crate does not depend on one. This is only where the answer is kept.
//!
//! [`Value`]: crate::Value

use std::sync::{Arc, OnceLock};

use crate::bytecode::Iseq;

static IMAGE: OnceLock<Vec<Arc<Iseq>>> = OnceLock::new();

/// The compiled core library, compiling it with `build` on the first call.
///
/// `build` runs at most once per process even under a race: the loser of
/// [`OnceLock::get_or_init`] discards its work and both callers see the winner's
/// slice, so the `Iseq`s a heap is loaded from are the same objects every time.
pub fn get_or_compile(build: impl FnOnce() -> Vec<Arc<Iseq>>) -> &'static [Arc<Iseq>] {
    IMAGE.get_or_init(build)
}

/// Whether the image has been compiled yet, for tests that care which call paid
/// for it.
#[must_use]
pub fn is_compiled() -> bool {
    IMAGE.get().is_some()
}
