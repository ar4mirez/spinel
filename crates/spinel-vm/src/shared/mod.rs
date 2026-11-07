//! The only process-global mutable state in the VM, and the reason it is allowed.
//!
//! `CLAUDE.md` forbids `static mut`, `lazy_static` with interior mutability, and
//! thread-locals holding VM objects, because one `Heap` per Ractor is what makes
//! Ractors parallel. The exception it names is this directory: **immutable,
//! append-only tables** — symbols, and later frozen string literals — and
//! values made once and never changed, as the hash seed is.
//!
//! Append-only is what makes them safe to share. An entry is never mutated and
//! never removed, so an index handed out at any moment stays valid forever and
//! no Ractor can observe another's write as a *change*. The lock protects the
//! append, not the reads that follow it.
//!
//! One more is here because the operating system puts it here: the signals
//! that arrived and wait to be handled (`signals`). A signal is delivered to
//! the process, on any OS thread, so its count cannot live on a heap; it is a
//! number per signal and holds no VM object, and only the main Ractor takes
//! from it.

pub mod core_image;
pub mod hash_seed;
pub mod signals;
pub mod symbols;
