//! The interpreter's side of the JIT (phase 6, #120).
//!
//! The JIT lowers the same bytecode the interpreter runs; there is no second
//! IR and no second semantics. This module is the boundary: what compiled
//! code is handed ([`Activation`]), how a backend is installed ([`Compiler`],
//! [`HandleScope::set_jit`]), and the per-heap table of what it made. The
//! loop runs it from `interp::jit_enter`. The backend itself is `spinel-jit`,
//! which this crate does not depend on, as it does not depend on Prism:
//! `spinel run` installs it when `SPINEL_JIT=force` is set (`docs/engine.md`,
//! "JIT").
//!
//! # The one way out
//!
//! Compiled code never leaves its frame, calls, allocates, raises or roots
//! anything. It runs the body from its first instruction for as long as each
//! instruction is one it lowers and its operands are ones it answers, then
//! *bails out*: it writes the operand stack it holds to
//! [`Activation::operands`] and answers the `pc` of the first instruction it
//! did not run. The interpreter goes on from there with the stack and locals
//! exactly as it would have had them. `Leave` and `Return` are bail-outs too,
//! so every rule about leaving a frame stays the interpreter's.
//!
//! Since nothing in compiled code can collect, the values it moves between
//! the frame's locals, the enclosing scopes' locals, `self`'s instance
//! variables and its operand stack stay valid while it runs: each is an
//! immediate, the frame's `self`, or a value one of those held on entry.
//! Afterwards a local may hold the only reference to a value read from
//! `self`'s instance variables or an enclosing scope, the place it came from
//! having been overwritten, and no handle names it; so after code that reads
//! either, `jit_enter` has the loop root the frame's locals again, as it
//! roots the operands the code left, before the next instruction can
//! collect.
//!
//! What compiled code reads beyond the frame, `jit_enter` resolves for it on
//! entry with the heap's own accessors, as [`Compiled`] asks: the enclosing
//! scopes' locals ([`Activation::envs`]) and `self`'s instance variables
//! beside the frame's sites' caches ([`Activation::ivars`]). So the heap's
//! layout stays this crate's, and compiled code indexes only memory whose
//! length it is handed, bailing out where the interpreter would look further.
//!
//! # The budget
//!
//! Compiled code is given as fuel the instructions the interpreter would run
//! before its next slow step — a turn check, a signal, a refusal boundary's
//! deadline, a traced instruction — and charges one per instruction, the
//! unfused count. Out of fuel is a bail-out like any other, so that step falls
//! on the instruction it would have fallen on without the JIT. While tracing
//! pins the loop's limit at its maximum, the fuel is zero and nothing runs
//! compiled.
//!
//! [`HandleScope::set_jit`]: crate::heap::HandleScope::set_jit

use std::collections::HashMap;

use crate::bytecode::Iseq;
use crate::value::Value;

/// What compiled code reads and writes, at the offsets `repr(C)` gives it.
///
/// `spinel-jit` reads the fields by [`std::mem::offset_of!`], so the two
/// sides agree by construction rather than by a comment.
#[repr(C)]
#[derive(Debug)]
pub struct Activation {
    /// The frame's locals, [`Iseq::locals`]`.len()` of them: read, and
    /// written only with a value from a local, an enclosing scope's local,
    /// [`Activation::receiver`], an instance variable of it, or an
    /// immediate. `jit_enter` roots them again after code that reads an
    /// enclosing scope or an instance variable.
    pub locals: *mut Value,
    /// Where the operand stack the code leaves is written, from the bottom,
    /// [`Iseq::max_stack`] long.
    pub operands: *mut Value,
    /// The frame's `self`, for `PushSelf`.
    pub receiver: Value,
    /// Which of `Integer`'s operators answer here, by [`crate::BinOp`]
    /// discriminant, with `-@` at [`NEG_OPERATOR`]: the mask the light loops
    /// ask, for this frame.
    pub ops: u64,
    /// How many instructions the code may run.
    pub fuel: u64,
    /// Written by the code: how many it ran, at most `fuel`.
    pub ran: u64,
    /// Written by the code: how many values it wrote to `operands`.
    pub depth: u64,
    /// [`Activation::ops`] for `Float`.
    pub float_ops: u64,
    /// The locals of the scopes around this one, [`Compiled::env_depth`] of
    /// them: the `n`-th is what `GetLocal(_, n + 1)` reads. Null when the
    /// body reads none.
    pub envs: *const Locals,
    /// `self`'s instance variables and the frame's sites' caches, when
    /// [`Compiled::ivars`] asked for them; nothing answers otherwise.
    pub ivars: Ivars,
}

/// `-@`'s bit in [`Activation::ops`] and [`Activation::float_ops`]: the one
/// after the last [`crate::BinOp`].
pub const NEG_OPERATOR: u32 = crate::BinOp::Ge as u32 + 1;

/// One enclosing scope's locals, in its heap environment: `len` values from
/// `values`, read and written in place as `GetLocal` and `SetLocal` do.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Locals {
    pub values: *mut Value,
    pub len: u64,
}

/// The view of `self` an interpreted `GetIvar` reads through
/// ([`crate::heap::IvarView`]), as compiled code reads it: a site reads
/// `values[index]` when its cache entry holds `shape` and `index` is below
/// `len`, and writes there an immediate when `writable` is set too.
///
/// It stays true for as long as nothing allocates, collects, fills a cache
/// or assigns an instance variable other than in place, which is all of a
/// run of compiled code.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ivars {
    /// `self`'s shape, or one no cache entry holds when its instance
    /// variables are not in storage of its own.
    pub shape: u64,
    pub values: *mut Value,
    pub len: u64,
    /// This frame's sites' caches, from its first: `caches_len` of them,
    /// each [`IVAR_ENTRY_SIZE`] bytes.
    pub caches: *const crate::callcache::IvarEntry,
    pub caches_len: u64,
    /// Whether `self` may be written: not frozen, nor chilled.
    pub writable: u64,
}

impl Ivars {
    /// Nothing to read or write: every site bails.
    pub const NONE: Ivars = Ivars {
        shape: 0,
        values: std::ptr::null_mut(),
        len: 0,
        caches: std::ptr::null(),
        caches_len: 0,
        writable: 0,
    };
}

/// The size of one instance-variable cache entry, and where its shape (a
/// `u16`) and index (a `u16`) are in it.
pub const IVAR_ENTRY_SIZE: usize = size_of::<crate::callcache::IvarEntry>();
pub const IVAR_ENTRY_SHAPE: usize = std::mem::offset_of!(crate::callcache::IvarEntry, shape);
pub const IVAR_ENTRY_INDEX: usize = std::mem::offset_of!(crate::callcache::IvarEntry, index);

const _: () = {
    assert!(size_of::<crate::shape::ShapeId>() == 2);
};

/// A body's compiled code: answers the `pc` to go on at.
///
/// # Safety
///
/// The caller hands it an [`Activation`] whose `locals` and `operands` are
/// valid for the lengths that type names, and aliased by nothing else for
/// the call. The code keeps to what [`Activation`] says of each field; the
/// interpreter checks the `pc`, `depth` and `ran` it answers before using
/// them.
pub type Entry = unsafe extern "C" fn(*mut Activation) -> u64;

/// What a backend made of a body: its code, and what [`Activation`] has to
/// carry for it beyond what every body gets.
#[derive(Debug, Clone, Copy)]
pub struct Compiled {
    pub entry: Entry,
    /// How many scopes out the code reads or writes a local:
    /// [`Activation::envs`]' length. A frame whose environment chain is
    /// shorter runs interpreted.
    pub env_depth: u16,
    /// Whether the code reads or writes an instance variable:
    /// [`Activation::ivars`].
    pub ivars: bool,
}

/// A backend: what makes a body's [`Compiled`] code.
///
/// One per heap, so per Ractor, as everything mutable is; the code it makes
/// lives as long as it does.
pub trait Compiler: Send {
    /// Compile `iseq`, or `None` when there is nothing to run compiled: its
    /// first instruction is one the backend does not lower, or the host has
    /// no code generator.
    fn compile(&mut self, iseq: &Iseq) -> Option<Compiled>;
}

/// Makes a heap's [`Compiler`], on first use. A function rather than a
/// compiler so that a new Ractor's heap makes its own.
pub type Factory = fn() -> Option<Box<dyn Compiler>>;

/// What the JIT has done in one heap, for tests and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Bodies compiled.
    pub compiled: u64,
    /// Bodies looked at and not compiled.
    pub declined: u64,
    /// Times compiled code was entered.
    pub entered: u64,
    /// Instructions compiled code ran, in all.
    pub ran: u64,
    /// Entries that ran nothing: out of fuel, or bailing at the first
    /// instruction on its operands.
    pub idle: u64,
}

/// A heap's JIT: its backend, and the code it made for each body.
pub(crate) struct Jit {
    factory: Factory,
    /// `None` until the first compile, and if the factory made none.
    compiler: Option<Box<dyn Compiler>>,
    started: bool,
    /// By the body's call-cache base, which is unique to it in this heap
    /// (`CallCaches::enter`): `None` for a body compiled to nothing.
    code: HashMap<u32, Option<Compiled>>,
    /// Where compiled code writes its operands, grown to the deepest body.
    pub(crate) scratch: Vec<Value>,
    /// Where the enclosing scopes' locals are put for [`Activation::envs`].
    pub(crate) envs: Vec<Locals>,
    pub(crate) stats: Stats,
}

impl Jit {
    pub(crate) fn new(factory: Factory) -> Jit {
        Jit {
            factory,
            compiler: None,
            started: false,
            code: HashMap::new(),
            scratch: Vec::new(),
            envs: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// The same backend, for a new Ractor's heap, with nothing compiled.
    pub(crate) fn fresh(&self) -> Jit {
        Jit::new(self.factory)
    }

    pub(crate) fn stats(&self) -> Stats {
        self.stats
    }

    /// The body's code, compiling it the first time it is asked for.
    pub(crate) fn entry(&mut self, base: u32, iseq: &Iseq) -> Option<Compiled> {
        if let Some(&known) = self.code.get(&base) {
            return known;
        }
        if !self.started {
            self.started = true;
            self.compiler = (self.factory)();
        }
        let entry = self.compiler.as_mut().and_then(|c| c.compile(iseq));
        if entry.is_some() {
            self.stats.compiled += 1;
        } else {
            self.stats.declined += 1;
        }
        self.code.insert(base, entry);
        entry
    }
}

impl std::fmt::Debug for Jit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jit")
            .field("bodies", &self.code.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}
