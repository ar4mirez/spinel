//! The lock around the services database's non-reentrant calls.
//!
//! `getservbyname(3)` and `getservbyport(3)` answer in one static buffer of
//! the C library's, which a call on any OS thread overwrites. Where the
//! platform has the reentrant `getservbyname_r` and `getservbyport_r` (Linux)
//! `socket.rs` calls those and never takes this; elsewhere (macOS) it holds
//! this lock from the call until the answer is copied out, so Ractors running
//! in parallel (#119) cannot overwrite each other's. It holds no VM object.

use std::sync::{Mutex, MutexGuard};

static SERVICES: Mutex<()> = Mutex::new(());

/// Hold the services database's lock for as long as the guard lives. Its
/// state is the C library's, so a poisoned lock is taken all the same.
pub fn lock() -> MutexGuard<'static, ()> {
    SERVICES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
