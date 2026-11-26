//! The primitives' method tables, built once per process (#652).
//!
//! Every heap's bootstrap gives the builtin classes the same few hundred
//! primitive methods: the same names, the same [`Native`]s, the same
//! visibilities. Building that answer means interning each name and inserting
//! it into a class's method table, and under Miri that was a third of a
//! bootstrap after the first. What no heap needs twice is the building, so the
//! first heap builds it and every heap copies the finished tables in.
//!
//! # Why this is allowed here
//!
//! `CLAUDE.md` forbids process-global mutable VM state and names this directory
//! as the exception: immutable, append-only tables. This one is written once and
//! never changed afterwards, and it holds no heap object. A body is the fixnum
//! index of a definition, which `install_primitives` adds to each heap's
//! definitions in the same order, so the index names the same [`Native`] in
//! every heap. A name is a `SymbolId` from the append-only symbol table, so it is
//! the same for every heap in the process. No Ractor can reach another's objects
//! through this.
//!
//! What the table contains is `install_primitives`'s business, in `interp.rs`,
//! next to the list of primitives. This module only keeps the answer.

use std::sync::OnceLock;

use crate::class::{ClassId, MethodEntry};
use crate::method::Native;
use crate::value::{SymbolId, SymbolMap};

/// The primitives as one heap's bootstrap installs them.
#[derive(Debug)]
pub struct Primitives {
    /// One definition per row of the primitive list, in the list's order, so
    /// row `i`'s body is the fixnum `i` in a heap whose definitions start empty.
    pub natives: Vec<Native>,
    /// Each class the list defines methods on, in the order it first does, with
    /// the method table those definitions leave it: the order is the order its
    /// method caches are invalidated in.
    pub methods: Vec<(ClassId, SymbolMap<MethodEntry>)>,
    /// The names of the numeric operators defined after the list, interned in
    /// the order they are defined.
    pub operators: Vec<SymbolId>,
    /// The names of `Kernel`'s module functions.
    pub module_functions: Vec<SymbolId>,
}

static PRIMITIVES: OnceLock<Primitives> = OnceLock::new();

/// The primitives, building them with `build` on the first call.
///
/// `build` runs at most once per process: a heap that asks while another is
/// building waits for [`OnceLock::get_or_init`] to finish, and both see the
/// same tables.
pub fn get_or_build(build: impl FnOnce() -> Primitives) -> &'static Primitives {
    PRIMITIVES.get_or_init(build)
}
