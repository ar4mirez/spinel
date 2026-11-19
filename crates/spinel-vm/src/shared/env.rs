//! The process environment's lock, as CRuby's `env_lock`.
//!
//! The C environment (`environ`) is the process's, not a Ractor's, so with
//! Ractors running in parallel (#119) two of them can write it at once. Every
//! `ENV` read and write the VM makes (`Kernel#__getenv__`, `__setenv__`,
//! `__environ__`) takes this one lock around its `std::env` call. It holds no
//! VM object: it guards no data of its own, only the order of calls into the
//! C library.

use std::sync::{Mutex, MutexGuard};

static ENV: Mutex<()> = Mutex::new(());

/// Hold the environment's lock for as long as the guard lives. A panic while
/// it was held left no state of ours half-written, so a poisoned lock is
/// taken all the same.
pub fn lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
