//! `Heap`: one per Ractor, with a precise non-moving mark-sweep collector.
//!
//! # The rule
//!
//! **A `Value` that Rust holds across an allocation must be in a [`HandleScope`].**
//!
//! It is not a convention. [`Heap`] has no method that allocates: allocation is
//! [`HandleScope::alloc`], which returns a [`Handle`] and never a bare pointer, and a
//! `Handle` is an index into the scope's root stack. So an object that the collector
//! cannot see is not a mistake a primitive can make — it is a program that does not
//! compile. A scope pops its own handles when it drops, and a nested scope's handle
//! cannot escape into its parent, because `Handle` borrows the scope it came from.
//!
//! # The collector
//!
//! Precise, non-moving, stop-the-world mark-sweep, as `docs/engine.md` specifies.
//! Precise because every root is known: today the root set is exactly the handle stack,
//! and the VM stack, frames, the current exception, and the per-heap tables each plug
//! into [`Heap::mark`] as their slice lands. Non-moving because it is the simplest
//! correct thing, and because handles make a moving collector a contained change later.
//! Nothing runs concurrently with Ruby code, so there is no write barrier.
//!
//! Objects up to 512 bytes come from size-classed free lists carved out of 64 KiB
//! blocks; anything larger gets its own allocation. Sweeping rebuilds the free lists
//! from every unmarked cell, so a dead object and a never-used cell take the same path.
//!
//! # Layout of a cell
//!
//! | offset | live object | free cell |
//! |---|---|---|
//! | 0 | `class` | next free cell |
//! | 8 | `len` | — |
//! | 12 | `flags` | zero: not allocated, which is what makes it free |
//! | 13 | `payload` | — |
//! | 14 | shape id | — |
//! | 16.. | `len` slots, or `len` bytes | — |

use std::alloc;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use crate::class::Classes;
use crate::shape::{ShapeId, Shapes};
use crate::value::{SymbolId, Value};

/// Cell sizes, in bytes. Powers of two so the class index is one shift.
const SIZE_CLASSES: [usize; 5] = [32, 64, 128, 256, 512];
const CLASS_COUNT: usize = SIZE_CLASSES.len();
/// The smallest cell, and the shift that turns a cell size into a class index.
const MIN_CELL_SHIFT: u32 = 5;
/// Above this, an object gets its own allocation in the large-object list.
const MAX_CELL: usize = SIZE_CLASSES[CLASS_COUNT - 1];

/// One block holds cells of a single size.
const BLOCK_SIZE: usize = 64 * 1024;
const BLOCK_ALIGN: usize = 16;

/// The first collection happens after this much allocation, and it is the floor for
/// every threshold after it. A fixed threshold would collect constantly once the live
/// set passed it.
const MIN_GC_BYTES: usize = 1024 * 1024;

/// Set while an object is reachable in the current cycle, cleared as it is swept.
///
/// Bits 1 and 2 are `docs/engine.md`'s `frozen` and `ractor-shareable`. They are not
/// defined here because nothing can set them until #8.
const MARKED: u8 = 0b1;

/// Set by `Object#freeze`, and never cleared: Ruby has no `unfreeze`.
///
/// One of the two bits `docs/engine.md` reserves in the header. It lives on the
/// object rather than in a side table because `frozen?` is asked on the hot
/// path of every mutating method, and a side table would be a lookup there.
const FROZEN: u8 = 0b10;

/// Set on an object whose instance variables live in the heap's side table
/// rather than in its first slot — a `String`, an `Array`, anything whose
/// payload is its own representation. Gates the lookup, so tracing an object
/// that holds none stays a flag test.
const SIDE_IVARS: u8 = 0b100;

/// Set on a *chilled* String (#263): a literal in a file with no
/// `frozen_string_literal` comment. `frozen?` is false, but the first mutation
/// clears it and warns that the literal will be frozen in the future.
const CHILLED: u8 = 0b1000;

/// [`CHILLED`]'s twin for what `Symbol#to_s` answers, which warns in other
/// words.
const CHILLED_SYMBOL: u8 = 0b1_0000;

/// Set on a `Hash` a `ruby2_keywords` method collected from its caller's
/// keywords (#200), so that splatting it back out passes them as keywords.
/// A header bit rather than an ivar because `Hash#inspect` must not show it
/// and `dup` must not copy it: the copy is a fresh cell, measured unmarked.
const KEYWORDS: u8 = 0b10_0000;

/// Set from allocation until the cell is swept onto a free list.
///
/// The collector never reads it. It exists so that storing a `Value` can assert, in
/// debug builds and under Miri, that the object is still alive — see
/// [`HandleScope::storable`]. A cell that has never held an object has it clear,
/// because blocks arrive zeroed.
const ALLOCATED: u8 = 0b1000_0000;

/// How to read the bytes after the header — and so, whether the collector descends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Payload {
    /// `len` [`Value`]s. The collector traces every one.
    Slots,
    /// `len` raw bytes: a `String`'s characters, a bignum's limbs. Never traced.
    Bytes,
}

/// The 16 bytes in front of every heap object.
#[repr(C)]
struct Header {
    /// The object's class. `None` until bootstrap classes land in #8; traced when it
    /// is not, because a collector that cannot follow a class pointer is one that has
    /// to be rewritten to gain one.
    class: Option<Value>,
    /// `Value` slots, or bytes.
    len: u32,
    flags: u8,
    payload: Payload,
    /// Which instance variables the object holds, and at what index. Two
    /// bytes, already paid for by the alignment of `class`. [`ShapeId::NONE`]
    /// on an object whose class has a representation of its own — see
    /// `shape.rs`.
    shape: ShapeId,
}

// R1: the header is two words. Checked by the compiler, because every size class and
// every slot offset in this file is derived from it.
const _: () = {
    assert!(size_of::<Header>() == 16);
    assert!(align_of::<Header>() == 8);
};

/// A pointer to a live object, with the header arithmetic in one place.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Obj(NonNull<Header>);

impl Obj {
    fn header(self) -> *mut Header {
        self.0.as_ptr()
    }

    /// The payload, which begins one header past the start of the cell.
    fn slots(self) -> *mut Value {
        // SAFETY: every object is allocated with at least `size_of::<Header>()` bytes,
        // so one past the header is within the same allocation.
        unsafe { self.0.as_ptr().add(1) }.cast()
    }

    fn bytes(self) -> *mut u8 {
        self.slots().cast()
    }
}

/// One block of cells, all the same size.
struct Block {
    memory: NonNull<u8>,
    cell_size: usize,
}

/// What the heap is holding, for tests and for `GC.stat` in phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Objects that survived the last collection.
    pub live_objects: usize,
    /// Bytes those objects occupy, counting whole cells.
    pub live_bytes: usize,
    /// Collections since the heap was created.
    pub collections: u64,
    /// 64 KiB blocks the heap has taken from the allocator and not given back.
    pub blocks: usize,
    /// Objects too large for a size class, each with its own allocation.
    pub large_objects: usize,
    /// Objects allocated since the heap was created, live or not. Ruby spells this
    /// `GC.stat[:total_allocated_objects]`.
    pub total_allocated: u64,
}

/// One Ractor's object graph: its memory, its roots, and its collector.
///
/// Neither `Send` nor `Sync`, because it holds raw pointers into its own blocks. That
/// is the property that keeps "one `Heap` per Ractor" a fact rather than a rule, and it
/// is a claim worth checking rather than asserting:
///
/// ```compile_fail
/// fn needs_send<T: Send>() {}
/// needs_send::<spinel_vm::Heap>();
/// ```
///
/// ```compile_fail
/// fn needs_sync<T: Sync>() {}
/// needs_sync::<spinel_vm::Heap>();
/// ```
/// Parse a string `eval` was handed: its source, the file name and first
/// line it is reported at.
///
/// The flag says whether the string runs inside a method, where a `yield`
/// at its top level is valid.
pub type Parser = fn(&str, &[u8], i64, bool) -> Result<ParsedSource, ParseFailure>;

/// The libraries shipped inside the binary, by the name `require` is given:
/// the Ruby source, or `None` for a name that is not one of them.
pub type Libraries = fn(&str) -> Option<&'static str>;

/// What a [`Parser`] produced: the tree, and what the parser had to say about
/// the source on the way (#268).
#[derive(Debug, Clone)]
pub struct ParsedSource {
    pub program: spinel_ast::Program,
    /// Each warning as the line Ruby prints — `path:line: warning: message`,
    /// newline included — and whether only `-w` prints it.
    pub warnings: Vec<(String, bool)>,
}

/// Why a [`Parser`] produced no tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseFailure {
    /// A `SyntaxError`, with its message.
    Syntax(String),
    /// The parser has no lowering for something in the string yet.
    Unsupported,
}

pub struct Heap {
    /// The root set. A [`HandleScope`] owns a contiguous run of it and truncates back
    /// to its base on drop, so this is `docs/engine.md`'s linked list of scopes with
    /// the links implied by the index.
    roots: Vec<Value>,
    blocks: Vec<Block>,
    /// Intrusive free-list heads, one per size class. The link lives in the first word
    /// of the free cell, where a live object keeps its class pointer.
    free: [*mut u8; CLASS_COUNT],
    large: Vec<(NonNull<Header>, alloc::Layout)>,
    /// Kept across collections so marking never allocates.
    mark_stack: Vec<Obj>,
    live_bytes: usize,
    live_objects: usize,
    bytes_since_gc: usize,
    next_gc: usize,
    collections: u64,
    total_allocated: u64,
    /// This Ractor's classes and modules. A root source: see [`Heap::mark`].
    classes: Classes,
    /// Per heap, like everything else here. Two heaps that build the same
    /// object hand out different ids for it, which is fine because a shape id
    /// never leaves the heap that minted it.
    shapes: Shapes,
    definitions: crate::method::Definitions,
    /// This heap's inline caches, indexed by call-site id. Not a root
    /// source: an entry is two integers and a `Method`, whose body is a
    /// fixnum definition id the collector never traces.
    call_caches: crate::callcache::CallCaches,
    regexps: crate::regexp::Regexps,
    /// Every fiber this heap has made, and the vectors of those not running
    /// (#16). Traced: a suspended fiber's frames hold values nothing else does.
    fibers: crate::interp::FiberTable,
    /// The `MatchData` the last successful match produced, which is what `$~`
    /// and `$1` read.
    ///
    /// ponytail: one per heap. Ruby scopes `$~` to the method frame and to the
    /// thread; `back-references_spec.rb` has an example for the thread half.
    /// Give it a frame slot when frames carry their own specials, and a
    /// Ractor-local when threads arrive.
    last_match: Value,
    /// Instance-variable storage for the objects flagged `SIDE_IVARS`, by
    /// address. An entry lives exactly as long as its object: traced from it
    /// in `mark`, dropped with it before `sweep`. Addresses are stable
    /// because the collector does not move.
    side_ivars: HashMap<usize, Value>,
    /// `$!`: the exception currently being handled, or nil (#206).
    ///
    /// Beside `last_match` rather than in `globals` for the same reason: it is
    /// not a value an assignment put in a map. The unwinder sets it where a
    /// handler takes an exception, the compiler saves and restores it around a
    /// protected body, and a frame restores it when it is popped — so a method
    /// called from inside a `rescue` sees it and nothing after the clause does.
    ///
    /// ponytail: one per heap, like `last_match`. Ruby scopes it to the thread;
    /// give it a Ractor-local when threads arrive.
    errinfo: Value,
    /// What the exception now on its way out is about: its message, then two
    /// values — a `NameError`'s `@name` and `@receiver` (#173), a
    /// `FrozenError`'s receiver, an `UncaughtThrowError`'s tag and value, a
    /// `LocalJumpError`'s reason and exit value (#29).
    ///
    /// An `Error` holds only `String`s, because a `Value` parked in one is
    /// unrooted for the whole unwind. So the site that raises leaves the two
    /// values here, where the collector sees them, and the one place an
    /// `Error` becomes an object takes them back. The message is the claim
    /// check: an error swallowed in Rust leaves its note behind, and a later
    /// raise with different text must not inherit it.
    name_error: Option<(String, Value, Value)>,
    /// A warning a primitive owes, said once the primitive returns (#222).
    ///
    /// Saying one is a call to `Kernel#__warning__`, which needs the frame
    /// stack a primitive does not see from deep inside a helper — the regexp
    /// matcher is reached from half a dozen of them. So the helper leaves the
    /// text here and the dispatcher, which has the stack, says it.
    pending_warning: Option<String>,
    /// `main`, the top level's `self`: one per heap, so every evaluation —
    /// the core library's and each of the program's — shares it, and the
    /// singleton methods `core/object.rb` gives it (`to_s`, `include`) are
    /// there for all of them. Nil until the first evaluation makes it.
    main: Value,
    /// How a string `eval` parses (#38), installed by whoever has a parser —
    /// `spinel_core::boot`. This crate does not depend on Prism, so without
    /// one `eval` of a String is refused.
    parser: Option<Parser>,
    /// See [`Libraries`]. Installed by `spinel_core::boot`, beside the parser.
    libraries: Option<Libraries>,
    /// Whether code this heap was meant to run did not all run — see
    /// [`HandleScope::mark_partial`].
    partial: bool,
    /// The program's name and arguments, as the embedder was given them:
    /// `$0` and `ARGV`.
    argv: Vec<String>,
    program_name: String,
    /// How many instructions one evaluation may run; `None` for no limit.
    /// See [`crate::interp::BUDGET`].
    budget: Option<u64>,
    /// This Ractor's global variables, by name (#166). A root source: see
    /// [`Heap::mark`].
    ///
    /// Per heap rather than process-global, which is `CLAUDE.md`'s rule and the
    /// way `classes` already works. Two Ractors sharing one table is the shape
    /// that makes a Ractor unsafe, and nothing in the corpus needs it.
    ///
    /// The regexp specials are deliberately absent: `$~` and its family are
    /// read off `last_match` above, and the compiler routes them to
    /// [`Insn::LastMatch`](crate::Insn::LastMatch) before this table is
    /// reached. An entry here is therefore always something an assignment put
    /// there, which is what makes presence the right answer for `defined?`.
    /// The name is the key and a *slot* is the value, so `alias $b $a` can
    /// point two names at one cell. Ruby's global alias shares storage rather
    /// than copying: a later write through either name is visible through both.
    globals: HashMap<SymbolId, usize>,
    /// The cells. Never shrinks — a global's storage outlives the name that
    /// reached it, because an alias may still hold the slot after the original
    /// name was pointed somewhere else. Measured: `$a = 1; alias $b $a; alias
    /// $a $c; $b = 5` leaves `$b` on the cell `$a` used to name.
    global_slots: Vec<Option<Value>>,
    /// `alias $x $&`. A regexp special is not in the table above — it is
    /// derived from the last match — so an alias to one records *which*
    /// derivation, and reading the name runs it. Writing raises: these are
    /// read-only, and Ruby names the alias in the message.
    global_specials: HashMap<SymbolId, crate::bytecode::MatchRef>,
    /// Globals the runtime owns and a program may not assign: `$:` and
    /// `$LOADED_FEATURES` (#39). By name, so an alias is marked separately.
    readonly_globals: std::collections::HashSet<SymbolId>,
    /// `--enable=frozen-string-literal` or `--disable=frozen-string-literal`,
    /// for every compile after it is set; `None` when neither was given.
    frozen_string_literal: Option<bool>,
    /// Globals whose assignment, or read, is a Ruby method rather than a cell
    /// (#268): bit 0 for writes, bit 1 for reads. `$,` warns when set, `$=`
    /// when read.
    hooked_globals: std::collections::HashMap<SymbolId, u8>,
    /// `GC.disable`: an allocation past the threshold does not collect. An
    /// explicit collection still does, as CRuby's `GC.start` does (#36).
    gc_disabled: bool,
    /// `GC.stress`: every allocation collects first.
    gc_stress: bool,
    /// `GC.start` asked for a collection. The interpreter loop runs it at
    /// the next call or return, where it can let go of the handles it took
    /// for instructions that have finished — collecting inside the native
    /// would keep alive everything those stale handles still name.
    gc_requested: bool,
    /// Time spent collecting, for `GC.total_time` and `GC.stat(:time)`.
    gc_time: std::time::Duration,
    /// Objects allocated as of the last collection, so `GC.stat` can count
    /// what has been allocated since without walking the blocks.
    allocated_at_gc: u64,
    /// The entries of every `ObjectSpace::WeakMap` and `WeakKeyMap`, by the
    /// map object's address (#36). Held weakly, which is what makes them
    /// weak: `mark` never traces an entry, except the value of a
    /// `WeakKeyMap` entry whose key is alive. After marking, each live map
    /// drops the entries whose referents did not survive; a table goes with
    /// its map before `sweep`, as `side_ivars` does.
    weak: HashMap<usize, WeakTable>,
    /// `ObjectSpace.define_finalizer`'s callables, by the address of the
    /// object they finalize. The callables are roots; the object is not.
    finalizers: HashMap<usize, Vec<Value>>,
    /// Finalizers whose object a collection found dead, with its
    /// `object_id`, waiting for the interpreter to run them. The object is
    /// already swept: a finalizer is handed the id and never the object, so
    /// running one cannot resurrect it.
    pending_finalizers: Vec<(i64, Vec<Value>)>,
}

/// One weak map's entries (#36): the identity of each key, its value, and
/// the `hash` a `WeakKeyMap` looked it up by.
#[derive(Debug, Default)]
pub(crate) struct WeakTable {
    /// A `WeakKeyMap`: the value is held for as long as its key lives — an
    /// ephemeron. A `WeakMap` holds both halves weakly, and an entry goes
    /// when either does.
    ephemeron: bool,
    entries: Vec<WeakEntry>,
    /// Where each key's entry is, by the key's bits: identity, which is
    /// stable because the collector does not move objects.
    by_key: HashMap<u64, usize>,
}

#[derive(Debug, Clone, Copy)]
struct WeakEntry {
    key: Value,
    value: Value,
    hash: i64,
}

impl WeakTable {
    pub(crate) fn get(&self, key: Value) -> Option<Value> {
        self.by_key
            .get(&key.to_bits())
            .map(|&index| self.entries[index].value)
    }

    pub(crate) fn insert(&mut self, key: Value, value: Value, hash: i64) {
        let entry = WeakEntry { key, value, hash };
        match self.by_key.get(&key.to_bits()) {
            Some(&index) => self.entries[index] = entry,
            None => {
                self.by_key.insert(key.to_bits(), self.entries.len());
                self.entries.push(entry);
            }
        }
    }

    pub(crate) fn remove(&mut self, key: Value) -> Option<Value> {
        let index = self.by_key.remove(&key.to_bits())?;
        let removed = self.entries.swap_remove(index);
        if let Some(moved) = self.entries.get(index) {
            self.by_key.insert(moved.key.to_bits(), index);
        }
        Some(removed.value)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.by_key.clear();
    }

    /// Every entry, in the order they were added, apart from deletions.
    pub(crate) fn pairs(&self) -> impl Iterator<Item = (Value, Value)> + '_ {
        self.entries.iter().map(|entry| (entry.key, entry.value))
    }

    /// The keys a `WeakKeyMap` stored under `hash`.
    ///
    /// ponytail: a scan of every entry, comparing integers. A bucket index
    /// by hash is the upgrade, worth it when a benchmark holds a large map.
    pub(crate) fn keys_with_hash(&self, hash: i64) -> impl Iterator<Item = Value> + '_ {
        self.entries
            .iter()
            .filter(move |entry| entry.hash == hash)
            .map(|entry| entry.key)
    }

    /// Drop the entries a collection left dangling.
    fn retain_live(&mut self) {
        let ephemeron = self.ephemeron;
        let before = self.entries.len();
        self.entries
            .retain(|entry| survived(entry.key) && (ephemeron || survived(entry.value)));
        if self.entries.len() != before {
            self.by_key.clear();
            for (index, entry) in self.entries.iter().enumerate() {
                self.by_key.insert(entry.key.to_bits(), index);
            }
        }
    }
}

/// Whether `value` came through the mark phase alive: an immediate always
/// does, and a heap object does when it is marked.
fn survived(value: Value) -> bool {
    match value.as_heap() {
        // SAFETY: called between marking and sweeping, on values the heap
        // still holds, so the cell is one this heap allocated and has not
        // freed.
        Some(pointer) => unsafe { (*pointer.cast::<Header>().as_ptr()).flags & MARKED != 0 },
        None => true,
    }
}

/// `Object#object_id` for the object at `address`: the address, shifted past
/// the alignment bits that are zero on every cell.
pub(crate) fn object_id(address: usize) -> i64 {
    i64::try_from(address >> 4).expect("a cell address shifted by four fits in an i64")
}

/// What `GC.stat` reports (#36), beside [`Stats`]: the counts it derives.
#[derive(Debug, Clone, Copy)]
pub struct GcStat {
    pub stats: Stats,
    /// Cells every block holds, used or not.
    pub available_slots: usize,
    /// Objects allocated and not yet found dead: the last collection's
    /// survivors and everything allocated since.
    pub live_slots: usize,
    pub time: std::time::Duration,
}

/// Whether the object at `address` is marked, between marking and sweeping.
fn address_marked(address: usize) -> bool {
    // SAFETY: every address a side table keys is one of this heap's live
    // objects as of the previous cycle, and nothing has been freed yet in
    // this one.
    unsafe { (*(address as *const Header)).flags & MARKED != 0 }
}

/// The class index for an object needing `bytes` in total, or `None` for large objects.
fn size_class(bytes: usize) -> Option<usize> {
    if bytes > MAX_CELL {
        return None;
    }
    let cell = bytes.max(SIZE_CLASSES[0]).next_power_of_two();
    Some(cell.trailing_zeros() as usize - MIN_CELL_SHIFT as usize)
}

const fn block_layout() -> alloc::Layout {
    match alloc::Layout::from_size_align(BLOCK_SIZE, BLOCK_ALIGN) {
        Ok(l) => l,
        Err(_) => panic!("block layout is a constant and is valid"),
    }
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            roots: Vec::new(),
            blocks: Vec::new(),
            free: [ptr::null_mut(); CLASS_COUNT],
            large: Vec::new(),
            mark_stack: Vec::new(),
            live_bytes: 0,
            live_objects: 0,
            bytes_since_gc: 0,
            next_gc: MIN_GC_BYTES,
            collections: 0,
            total_allocated: 0,
            classes: Classes::new(),
            shapes: Shapes::new(),
            definitions: crate::method::Definitions::new(),
            call_caches: crate::callcache::CallCaches::new(),
            regexps: crate::regexp::Regexps::new(),
            fibers: crate::interp::FiberTable::default(),
            last_match: Value::NIL,
            side_ivars: HashMap::new(),
            name_error: None,
            pending_warning: None,
            errinfo: Value::NIL,
            main: Value::NIL,
            parser: None,
            libraries: None,
            partial: false,
            argv: Vec::new(),
            program_name: String::new(),
            budget: Some(crate::interp::BUDGET),
            globals: HashMap::new(),
            global_slots: Vec::new(),
            global_specials: HashMap::new(),
            readonly_globals: std::collections::HashSet::new(),
            frozen_string_literal: None,
            hooked_globals: std::collections::HashMap::new(),
            gc_disabled: false,
            gc_stress: false,
            gc_requested: false,
            gc_time: std::time::Duration::ZERO,
            allocated_at_gc: 0,
            weak: HashMap::new(),
            finalizers: HashMap::new(),
            pending_finalizers: Vec::new(),
        }
    }

    /// This heap's classes and modules. [`HandleScope::bootstrap`] fills it.
    pub fn classes(&self) -> &Classes {
        &self.classes
    }

    /// The method bodies this heap's class table points at. Not traced: a
    /// definition id is a fixnum, which is the point of it being one.
    pub fn definitions(&self) -> &crate::method::Definitions {
        &self.definitions
    }

    /// The compiled patterns and the literal cache. Unlike `definitions`, the
    /// cache *is* traced: a cached literal is reachable from nothing else, and
    /// Ruby hands the same object back every time the literal is evaluated.
    pub fn regexps(&self) -> &crate::regexp::Regexps {
        &self.regexps
    }

    pub fn regexps_mut(&mut self) -> &mut crate::regexp::Regexps {
        &mut self.regexps
    }

    pub(crate) fn fibers(&self) -> &crate::interp::FiberTable {
        &self.fibers
    }

    pub(crate) fn fibers_mut(&mut self) -> &mut crate::interp::FiberTable {
        &mut self.fibers
    }

    /// The last successful match, or nil.
    pub fn last_match(&self) -> Value {
        self.last_match
    }

    pub fn set_last_match(&mut self, value: Value) {
        self.last_match = value;
    }

    pub fn set_name_error(&mut self, message: &str, name: Value, receiver: Value) {
        self.name_error = Some((message.to_owned(), name, receiver));
    }

    /// The note for `message`, if the last one left was for it. Cleared either
    /// way.
    pub fn take_name_error(&mut self, message: &str) -> Option<(Value, Value)> {
        let (text, name, receiver) = self.name_error.take()?;
        (text == message).then_some((name, receiver))
    }

    pub fn set_pending_warning(&mut self, message: String) {
        self.pending_warning = Some(message);
    }

    pub fn take_pending_warning(&mut self) -> Option<String> {
        self.pending_warning.take()
    }

    /// `$!`: the exception being handled, or nil.
    pub fn errinfo(&self) -> Value {
        self.errinfo
    }

    pub fn set_errinfo(&mut self, value: Value) {
        self.errinfo = value;
    }

    pub fn main(&self) -> Value {
        self.main
    }

    pub fn set_main(&mut self, value: Value) {
        self.main = value;
    }

    /// A global's value, or `None` for a name nothing has assigned.
    ///
    /// `None` rather than `NIL`, because `$a = nil` is a global that exists and
    /// `defined?` has to tell the two apart.
    pub fn global(&self, name: SymbolId) -> Option<Value> {
        let slot = *self.globals.get(&name)?;
        self.global_slots[slot]
    }

    pub fn set_global(&mut self, name: SymbolId, value: Value) {
        let slot = self.global_slot(name);
        self.global_slots[slot] = Some(value);
    }

    /// The cell `name` reads and writes, created empty if it has none.
    ///
    /// A name that has never been assigned still gets a cell once something
    /// asks for one, because `alias $b $a` on an unassigned `$a` still has to
    /// bind the two together — measured, a later `$a = 1` is visible as `$b`.
    fn global_slot(&mut self, name: SymbolId) -> usize {
        if let Some(slot) = self.globals.get(&name) {
            return *slot;
        }
        let slot = self.global_slots.len();
        self.global_slots.push(None);
        self.globals.insert(name, slot);
        slot
    }

    /// `alias $new $old`: point `new` at the cell `old` names.
    pub fn alias_global(&mut self, new: SymbolId, old: SymbolId) {
        let slot = self.global_slot(old);
        self.globals.insert(new, slot);
        // An alias replaces any read-only derivation the name had.
        self.global_specials.remove(&new);
    }

    /// `alias $new $&`: `new` reads the derivation and refuses writes.
    pub fn alias_global_special(&mut self, new: SymbolId, which: crate::bytecode::MatchRef) {
        self.global_specials.insert(new, which);
        self.globals.remove(&new);
    }

    /// Which regexp special `name` was aliased to, if it was.
    pub fn global_special(&self, name: SymbolId) -> Option<crate::bytecode::MatchRef> {
        self.global_specials.get(&name).copied()
    }

    /// Make `name` refuse assignment from now on.
    pub fn freeze_global(&mut self, name: SymbolId) {
        self.readonly_globals.insert(name);
    }

    pub fn global_is_readonly(&self, name: SymbolId) -> bool {
        self.readonly_globals.contains(&name)
    }

    /// Route `name` through `Kernel#__global_assign__`, and through
    /// `__global_read__` too when `reads`.
    pub fn hook_global(&mut self, name: SymbolId, reads: bool) {
        self.hooked_globals.insert(name, if reads { 3 } else { 1 });
    }

    /// Which of `name`'s accesses are hooked: bit 0 writes, bit 1 reads.
    pub fn global_hooks(&self, name: SymbolId) -> u8 {
        self.hooked_globals.get(&name).copied().unwrap_or(0)
    }

    pub fn definitions_mut(&mut self) -> &mut crate::method::Definitions {
        &mut self.definitions
    }

    /// What each call site last resolved to. Read on every `Send`, so the
    /// accessors are the whole interface: see [`crate::callcache`].
    pub fn call_caches(&self) -> &crate::callcache::CallCaches {
        &self.call_caches
    }

    pub fn call_caches_mut(&mut self) -> &mut crate::callcache::CallCaches {
        &mut self.call_caches
    }

    pub fn classes_mut(&mut self) -> &mut Classes {
        &mut self.classes
    }

    /// This heap's shape tree — what an object's instance variables are called
    /// and where they live. Not a root source: it holds symbols and indices,
    /// never a [`Value`].
    pub fn shapes(&self) -> &Shapes {
        &self.shapes
    }

    pub fn shapes_mut(&mut self) -> &mut Shapes {
        &mut self.shapes
    }

    /// Open the outermost handle scope. Everything that allocates goes through one.
    pub fn scope(&mut self) -> HandleScope<'_> {
        HandleScope {
            base: self.roots.len(),
            heap: self,
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            live_objects: self.live_objects,
            live_bytes: self.live_bytes,
            collections: self.collections,
            blocks: self.blocks.len(),
            large_objects: self.large.len(),
            total_allocated: self.total_allocated,
        }
    }

    /// Total bytes for an object with this payload, header included, rounded so the
    /// next cell still starts eight-byte aligned.
    fn object_size(payload: Payload, len: u32) -> usize {
        let body = match payload {
            Payload::Slots => len as usize * size_of::<Value>(),
            Payload::Bytes => (len as usize).next_multiple_of(size_of::<Value>()),
        };
        size_of::<Header>() + body
    }

    /// Allocate, collecting first if this allocation crosses the threshold.
    ///
    /// Collection happens *before* the object exists, never after: a fresh object has
    /// no handle yet, so a collection between allocating it and rooting it would free
    /// the thing the caller is about to be handed.
    fn allocate(&mut self, class: Option<Value>, payload: Payload, len: u32) -> Value {
        let size = Heap::object_size(payload, len);
        if !self.gc_disabled && (self.gc_stress || self.bytes_since_gc >= self.next_gc) {
            self.collect();
        }
        self.bytes_since_gc += size;
        self.total_allocated += 1;

        let cell = match size_class(size) {
            Some(class_index) => self.cell_from_free_list(class_index),
            None => self.large_object(size),
        };

        let obj = Obj(cell.cast());
        // SAFETY: `cell` is at least `size` bytes, eight-byte aligned, and owned by
        // this heap. Writing the whole header initialises it, including over a free
        // cell's link in the first word.
        unsafe {
            ptr::write(
                obj.header(),
                Header {
                    class,
                    len,
                    flags: ALLOCATED,
                    payload,
                    shape: ShapeId::NONE,
                },
            );
            match payload {
                // A zeroed slot is not a `Value` — #6 spent the niche on exactly that —
                // so the payload is filled before anything can trace it.
                Payload::Slots => {
                    let slots = obj.slots();
                    for i in 0..len as usize {
                        ptr::write(slots.add(i), Value::NIL);
                    }
                }
                // A reused cell still holds the dead object's bytes.
                Payload::Bytes => ptr::write_bytes(obj.bytes(), 0, len as usize),
            }
        }
        Value::heap(obj.0.cast())
    }

    fn cell_from_free_list(&mut self, class_index: usize) -> NonNull<u8> {
        if self.free[class_index].is_null() {
            self.add_block(class_index);
        }
        let cell = self.free[class_index];
        // SAFETY: `add_block` leaves a non-null head, and every cell on the list holds
        // its successor in the first word.
        self.free[class_index] = unsafe { ptr::read(cell.cast::<*mut u8>()) };
        NonNull::new(cell).expect("a free cell is never null")
    }

    /// Carve a fresh block into cells and push all of them onto the free list.
    fn add_block(&mut self, class_index: usize) {
        let cell_size = SIZE_CLASSES[class_index];
        // SAFETY: `block_layout` is a valid non-zero layout. Zeroed because `sweep`
        // reads the flags of every cell in a block, including cells that have never
        // held an object; zero reads as unmarked, which is what a free cell is.
        let memory = unsafe { alloc::alloc_zeroed(block_layout()) };
        let Some(memory) = NonNull::new(memory) else {
            alloc::handle_alloc_error(block_layout());
        };
        self.blocks.push(Block { memory, cell_size });

        // Backwards, so the head of the list is the lowest address and a run of
        // allocations walks the block forwards.
        let mut head = self.free[class_index];
        for i in (0..BLOCK_SIZE / cell_size).rev() {
            // SAFETY: `i * cell_size` stays inside the block by construction.
            let cell = unsafe { memory.as_ptr().add(i * cell_size) };
            unsafe { ptr::write(cell.cast::<*mut u8>(), head) };
            head = cell;
        }
        self.free[class_index] = head;
    }

    fn large_object(&mut self, size: usize) -> NonNull<u8> {
        let layout = alloc::Layout::from_size_align(size, BLOCK_ALIGN)
            .expect("an object size is a multiple of eight and far below isize::MAX");
        // SAFETY: `size` is non-zero, since it includes the header.
        let memory = unsafe { alloc::alloc_zeroed(layout) };
        let Some(memory) = NonNull::new(memory) else {
            alloc::handle_alloc_error(layout);
        };
        self.large.push((memory.cast(), layout));
        memory
    }

    /// Mark every object reachable from the roots, then reclaim the rest.
    pub fn collect(&mut self) {
        let started = std::time::Instant::now();
        self.mark();
        self.trace_weak();
        self.queue_finalizers();
        self.sweep();
        self.gc_time += started.elapsed();
        self.allocated_at_gc = self.total_allocated;
        self.collections += 1;
        self.bytes_since_gc = 0;
        self.next_gc = self.live_bytes.saturating_mul(2).max(MIN_GC_BYTES);
    }

    fn mark(&mut self) {
        debug_assert!(self.mark_stack.is_empty());
        for i in 0..self.roots.len() {
            let root = self.roots[i];
            Heap::shade(&mut self.mark_stack, root);
        }
        // The second root source: a class object is reachable from its table
        // entry and a method body from its method table, and neither is on the
        // handle stack. `shade` takes the mark stack rather than the whole heap
        // so that this is two disjoint field borrows and not a table moved out
        // and put back — a walk that panicked halfway through the second shape
        // would leave the heap with no classes at all.
        let (classes, mark_stack) = (&self.classes, &mut self.mark_stack);
        classes.each_root(|value| Heap::shade(mark_stack, value));

        // Third root source: the regexp literal cache. `/foo/` evaluated twice
        // answers one object, so that object outlives every handle to it.
        let (regexps, mark_stack) = (&self.regexps, &mut self.mark_stack);
        regexps.each_root(|value| Heap::shade(mark_stack, value));
        // Sixth: the `Proc`s `define_method` turned into method bodies.
        let (definitions, mark_stack) = (&self.definitions, &mut self.mark_stack);
        definitions.each_root(|value| Heap::shade(mark_stack, value));
        // Fifth: the vectors of every fiber that is not running.
        let (fibers, mark_stack) = (&self.fibers, &mut self.mark_stack);
        fibers.each_root(|value| Heap::shade(mark_stack, value));
        Heap::shade(&mut self.mark_stack, self.last_match);
        if let Some((_, name, receiver)) = self.name_error {
            Heap::shade(&mut self.mark_stack, name);
            Heap::shade(&mut self.mark_stack, receiver);
        }
        // `$!` outlives every handle to it the same way, and for the whole time
        // a handler is running.
        Heap::shade(&mut self.mark_stack, self.errinfo);
        Heap::shade(&mut self.mark_stack, self.main);
        // Fourth root source: the global table. A global outlives every handle
        // to what it holds, by definition — that is what a global is.
        let (slots, mark_stack) = (&self.global_slots, &mut self.mark_stack);
        for value in slots.iter().flatten() {
            Heap::shade(mark_stack, *value);
        }
        // Seventh: finalizers, registered and queued. A finalizer that holds
        // its own object keeps it alive, which is CRuby's rule too; it runs
        // at exit instead.
        let (finalizers, mark_stack) = (&self.finalizers, &mut self.mark_stack);
        for value in finalizers.values().flatten() {
            Heap::shade(mark_stack, *value);
        }
        let (pending, mark_stack) = (&self.pending_finalizers, &mut self.mark_stack);
        for value in pending.iter().flat_map(|(_, callables)| callables) {
            Heap::shade(mark_stack, *value);
        }
        self.drain();
    }

    /// Trace everything the mark stack reaches.
    fn drain(&mut self) {
        // R3: a worklist, not recursion. A Ruby program can build a chain a million
        // objects deep, and a recursive tracer turns that into a stack overflow inside
        // the collector, with no Ruby frame to blame it on.
        while let Some(obj) = self.mark_stack.pop() {
            // SAFETY: only live objects of this heap are ever shaded, and `shade` marked
            // this one before pushing it.
            let header = unsafe { &*obj.header() };
            // Everything is read before anything is shaded: shading writes a
            // header's flags, and this object may be the one shaded.
            let (class, len, payload) = (header.class, header.len, header.payload);
            let side = header.flags & SIDE_IVARS != 0;
            if let Some(class) = class {
                Heap::shade(&mut self.mark_stack, class);
            }
            if side && let Some(storage) = self.side_ivars.get(&(obj.0.as_ptr() as usize)) {
                Heap::shade(&mut self.mark_stack, *storage);
            }
            if payload == Payload::Slots {
                let slots = obj.slots();
                for i in 0..len as usize {
                    // SAFETY: the payload holds `len` initialised `Value`s; `allocate`
                    // writes every one before the object is reachable.
                    let slot = unsafe { ptr::read(slots.add(i)) };
                    Heap::shade(&mut self.mark_stack, slot);
                }
            }
        }
    }

    /// The second root pass weak maps need (#36), after the strong one has
    /// drained.
    ///
    /// First the ephemerons: a `WeakKeyMap` value is reachable through its
    /// key, so one whose key survived is marked and traced — and tracing it
    /// may bring another map's key to life, which is why this repeats until
    /// a pass marks nothing. Then every surviving map lets go of the entries
    /// whose referents did not survive, so nothing reads a cell `sweep` is
    /// about to free.
    fn trace_weak(&mut self) {
        loop {
            let mut shaded = false;
            let (weak, mark_stack) = (&self.weak, &mut self.mark_stack);
            for (&owner, table) in weak {
                if !table.ephemeron || !address_marked(owner) {
                    continue;
                }
                for entry in &table.entries {
                    if survived(entry.key) && !survived(entry.value) {
                        Heap::shade(mark_stack, entry.value);
                        shaded = true;
                    }
                }
            }
            if !shaded {
                break;
            }
            self.drain();
        }
        for (&owner, table) in &mut self.weak {
            if address_marked(owner) {
                table.retain_live();
            }
        }
    }

    /// Move the finalizers of every object that did not survive onto the
    /// queue the interpreter drains, with the id the object answered to.
    fn queue_finalizers(&mut self) {
        let dead: Vec<usize> = self
            .finalizers
            .keys()
            .copied()
            .filter(|&address| !address_marked(address))
            .collect();
        for address in dead {
            if let Some(callables) = self.finalizers.remove(&address) {
                self.pending_finalizers
                    .push((object_id(address), callables));
            }
        }
    }

    /// Mark `value` if it is an unmarked heap object, and queue it for tracing.
    ///
    /// Takes the mark stack rather than `&mut self`, so a root source held
    /// behind `&self` — the class table — can be walked without moving it out.
    fn shade(mark_stack: &mut Vec<Obj>, value: Value) {
        let Some(pointer) = value.as_heap() else {
            return;
        };
        let obj = Obj(pointer.cast());
        // SAFETY: precise collection — a `Value` with the heap tag in a root or a slot
        // is an object this heap allocated.
        let flags = unsafe { &mut (*obj.header()).flags };
        if *flags & MARKED == 0 {
            *flags |= MARKED;
            mark_stack.push(obj);
        }
    }

    /// Rebuild every free list from the unmarked cells, and free unmarked large objects.
    fn sweep(&mut self) {
        // Before any cell is relinked or freed: an entry whose object did not
        // survive goes with it, so a later object at the same address does
        // not inherit its instance variables.
        self.side_ivars.retain(|&address, _| {
            // SAFETY: an entry is only ever made for a live object of this
            // heap, and nothing has been freed yet in this cycle.
            unsafe { (*(address as *const Header)).flags & MARKED != 0 }
        });
        // A weak map's table goes with the map, for the same reason.
        self.weak.retain(|&address, _| address_marked(address));
        self.free = [ptr::null_mut(); CLASS_COUNT];
        let mut live_bytes = 0;
        let mut live_objects = 0;

        for block_index in 0..self.blocks.len() {
            let Block { memory, cell_size } = self.blocks[block_index];
            let class_index = size_class(cell_size).expect("a block holds one size class");
            // Backwards, so the rebuilt list runs forwards through the block.
            for i in (0..BLOCK_SIZE / cell_size).rev() {
                // SAFETY: `i * cell_size` stays inside the block by construction.
                let cell = unsafe { memory.as_ptr().add(i * cell_size) };
                let header = cell.cast::<Header>();
                // SAFETY: the block was zeroed, so a cell that never held an object
                // reads as unmarked rather than as uninitialised memory.
                let flags = unsafe { (*header).flags };
                if flags & MARKED != 0 {
                    unsafe { (*header).flags = flags & !MARKED };
                    live_bytes += cell_size;
                    live_objects += 1;
                } else {
                    // A finalizer was queued before this loop began
                    // (`queue_finalizers`), so relinking the cell is all
                    // that is left: nothing runs here.
                    unsafe { (*header).flags = 0 };
                    unsafe { ptr::write(cell.cast::<*mut u8>(), self.free[class_index]) };
                    self.free[class_index] = cell;
                }
            }
        }

        self.large.retain(|&(pointer, layout)| {
            // SAFETY: every entry is an allocation this heap made and has not freed.
            let flags = unsafe { (*pointer.as_ptr()).flags };
            if flags & MARKED != 0 {
                unsafe { (*pointer.as_ptr()).flags = flags & !MARKED };
                live_bytes += layout.size();
                live_objects += 1;
                true
            } else {
                // SAFETY: unreachable, so nothing can read it after this point, and the
                // layout is the one it was allocated with.
                unsafe {
                    (*pointer.as_ptr()).flags = 0;
                    alloc::dealloc(pointer.as_ptr().cast(), layout);
                }
                false
            }
        });

        // ponytail: a block whose cells are all free is kept rather than returned to
        // the allocator. Giving one back needs a per-block live count; worth writing
        // when a benchmark shows the heap staying large after an allocation spike.
        self.live_bytes = live_bytes;
        self.live_objects = live_objects;
    }
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

/// R7: the heap frees everything it allocated. A leak here is invisible to a test that
/// only checks liveness, so it is Miri that checks this one.
impl Drop for Heap {
    fn drop(&mut self) {
        for block in &self.blocks {
            // SAFETY: allocated by `add_block` with this exact layout, and the heap is
            // the only owner.
            unsafe { alloc::dealloc(block.memory.as_ptr(), block_layout()) };
        }
        for &(pointer, layout) in &self.large {
            // SAFETY: allocated by `large_object` with this exact layout.
            unsafe { alloc::dealloc(pointer.as_ptr().cast(), layout) };
        }
    }
}

impl fmt::Debug for Heap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Heap")
            .field("roots", &self.roots.len())
            .field("stats", &self.stats())
            .field("classes", &self.classes)
            .finish()
    }
}

/// A rooted [`Value`]: an index into the heap's root stack, tied to the scope that
/// pushed it.
///
/// Covariant in `'h`, which is what makes the two directions differ. A parent scope's
/// handle is usable inside a nested scope, because the parent outlives it; a nested
/// scope's handle is not usable in the parent, because it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle<'h> {
    index: usize,
    scope: PhantomData<&'h ()>,
}

/// A run of the root stack. Everything that allocates does so through one.
///
/// Dropping the scope pops the handles it pushed. Nesting a scope reborrows the heap,
/// so the borrow checker does the rest: a nested scope's handles cannot outlive it, and
/// the parent cannot be touched while it is open.
///
/// A parent's handle works inside a nested scope, because the parent outlives it:
///
/// ```
/// use spinel_vm::{Heap, Payload};
///
/// let mut heap = Heap::new();
/// let mut outer = heap.scope();
/// let object = outer.alloc(None, Payload::Slots, 1);
/// {
///     let mut inner = outer.nested();
///     assert!(inner.get(object).as_heap().is_some());
/// }
/// ```
///
/// A nested scope's handle does not survive it. This is R4, and it is the whole point
/// of the type — so it is checked by the compiler rather than asserted in prose:
///
/// ```compile_fail
/// use spinel_vm::{Heap, Payload};
///
/// let mut heap = Heap::new();
/// let mut outer = heap.scope();
/// let escaped = {
///     let mut inner = outer.nested();
///     inner.alloc(None, Payload::Slots, 1)
/// };
/// outer.get(escaped);
/// ```
pub struct HandleScope<'h> {
    heap: &'h mut Heap,
    base: usize,
}

impl<'h> HandleScope<'h> {
    /// Root a `Value` that is already reachable, so it survives the next collection.
    pub fn root(&mut self, value: Value) -> Handle<'h> {
        self.storable(value);
        let index = self.heap.roots.len();
        self.heap.roots.push(value);
        Handle {
            index,
            scope: PhantomData,
        }
    }

    /// Allocate an object with `len` slots or bytes, rooted in this scope.
    ///
    /// `class` is a handle rather than a `Value` because this call can collect: an
    /// unrooted class passed by value would be a class pointer into a free cell.
    pub fn alloc(&mut self, class: Option<Handle<'h>>, payload: Payload, len: u32) -> Handle<'h> {
        let class = class.map(|handle| self.get(handle));
        let value = self.heap.allocate(class, payload, len);
        self.root(value)
    }

    /// A scope whose handles are popped when it drops, and cannot escape into this one.
    pub fn nested(&mut self) -> HandleScope<'_> {
        HandleScope {
            base: self.heap.roots.len(),
            heap: self.heap,
        }
    }

    pub fn get(&self, handle: Handle<'h>) -> Value {
        self.heap.roots[handle.index]
    }

    /// Repoint an object's class. The header field the collector traces.
    ///
    /// Two callers, both of which Ruby has and neither of which is "reopening a
    /// class": bootstrap, which allocates `Class` before it can point anything at
    /// it, and `singleton_class_of`, where an object's class *becoming* its
    /// singleton is the whole mechanism.
    pub fn set_class(&mut self, handle: Handle<'h>, class: Value) {
        self.storable(class);
        let object = self.object(handle);
        // SAFETY: the handle roots a live object of this heap, and `storable`
        // checked that the class it is about to point at is live too.
        unsafe { (*object.header()).class = Some(class) }
    }

    /// This heap's classes and modules.
    pub fn classes(&self) -> &Classes {
        self.heap.classes()
    }

    pub fn classes_mut(&mut self) -> &mut Classes {
        self.heap.classes_mut()
    }

    /// This heap's shape tree.
    pub fn shapes(&self) -> &Shapes {
        self.heap.shapes()
    }

    pub fn shapes_mut(&mut self) -> &mut Shapes {
        self.heap.shapes_mut()
    }

    /// The method bodies this heap's class table points at.
    pub fn definitions(&self) -> &crate::method::Definitions {
        self.heap.definitions()
    }

    /// What each call site last resolved to. See [`crate::callcache`].
    pub fn call_caches(&self) -> &crate::callcache::CallCaches {
        self.heap.call_caches()
    }

    pub fn call_caches_mut(&mut self) -> &mut crate::callcache::CallCaches {
        self.heap.call_caches_mut()
    }

    pub fn definitions_mut(&mut self) -> &mut crate::method::Definitions {
        self.heap.definitions_mut()
    }

    /// The heap's compiled patterns and literal cache.
    pub fn regexps(&self) -> &crate::regexp::Regexps {
        self.heap.regexps()
    }

    pub fn regexps_mut(&mut self) -> &mut crate::regexp::Regexps {
        self.heap.regexps_mut()
    }

    pub(crate) fn fibers(&self) -> &crate::interp::FiberTable {
        self.heap.fibers()
    }

    pub(crate) fn fibers_mut(&mut self) -> &mut crate::interp::FiberTable {
        self.heap.fibers_mut()
    }

    pub fn last_match(&self) -> Value {
        self.heap.last_match()
    }

    pub fn set_last_match(&mut self, value: Value) {
        self.heap.set_last_match(value);
    }

    pub fn set_name_error(&mut self, message: &str, name: Value, receiver: Value) {
        self.heap.set_name_error(message, name, receiver);
    }

    pub fn take_name_error(&mut self, message: &str) -> Option<(Value, Value)> {
        self.heap.take_name_error(message)
    }

    pub fn set_pending_warning(&mut self, message: String) {
        self.heap.set_pending_warning(message);
    }

    pub fn take_pending_warning(&mut self) -> Option<String> {
        self.heap.take_pending_warning()
    }

    pub fn errinfo(&self) -> Value {
        self.heap.errinfo()
    }

    /// Install what string `eval` parses with (#38).
    /// `--enable=frozen-string-literal`: every file and `eval` compiled from
    /// now on freezes the literals it does not say anything about;
    /// `--disable=frozen-string-literal` leaves them plain, not chilled.
    pub fn set_frozen_string_literal(&mut self, setting: Option<bool>) {
        self.heap.frozen_string_literal = setting;
    }

    pub fn frozen_string_literal(&self) -> Option<bool> {
        self.heap.frozen_string_literal
    }

    pub fn set_parser(&mut self, parser: Parser) {
        self.heap.parser = Some(parser);
    }

    pub(crate) fn parser(&self) -> Option<Parser> {
        self.heap.parser
    }

    /// Install the table of libraries shipped inside the binary.
    pub fn set_libraries(&mut self, libraries: Libraries) {
        self.heap.libraries = Some(libraries);
    }

    pub(crate) fn library(&self, name: &str) -> Option<&'static str> {
        self.heap.libraries.and_then(|find| find(name))
    }

    /// Record that a file this heap was meant to load did not finish, so a
    /// name it would have defined may be missing. `spec/spinel/runner.rb`
    /// marks a heap whose fixture failed, through `__mark_partial__`;
    /// `defined?` then refuses rather than answer `nil`.
    pub fn mark_partial(&mut self) {
        self.heap.partial = true;
    }

    pub(crate) fn partial(&self) -> bool {
        self.heap.partial
    }

    /// The program name and arguments `$0` and `ARGV` start as. Set before
    /// `spinel_core::boot`, which reads them.
    pub fn set_argv(&mut self, program_name: &str, argv: &[String]) {
        self.heap.program_name = program_name.to_owned();
        self.heap.argv = argv.to_vec();
    }

    /// Limit each evaluation to `budget` instructions, or lift the limit.
    /// A spec runner keeps the default, so a loop that never ends is a
    /// reported failure; a program runs without one.
    pub fn set_budget(&mut self, budget: Option<u64>) {
        self.heap.budget = budget;
    }

    pub(crate) fn budget(&self) -> Option<u64> {
        self.heap.budget
    }

    pub(crate) fn argv(&self) -> (String, Vec<String>) {
        (self.heap.program_name.clone(), self.heap.argv.clone())
    }

    pub fn set_errinfo(&mut self, value: Value) {
        self.heap.set_errinfo(value);
    }

    pub fn main(&self) -> Value {
        self.heap.main()
    }

    pub fn set_main(&mut self, value: Value) {
        self.heap.set_main(value);
    }

    /// This heap's global of that name, or `None` if nothing assigned it.
    pub fn global(&self, name: SymbolId) -> Option<Value> {
        self.heap.global(name)
    }

    pub fn set_global(&mut self, name: SymbolId, value: Value) {
        self.storable(value);
        self.heap.set_global(name, value);
    }

    pub fn alias_global(&mut self, new: SymbolId, old: SymbolId) {
        self.heap.alias_global(new, old);
    }

    pub fn alias_global_special(&mut self, new: SymbolId, which: crate::bytecode::MatchRef) {
        self.heap.alias_global_special(new, which);
    }

    pub fn global_special(&self, name: SymbolId) -> Option<crate::bytecode::MatchRef> {
        self.heap.global_special(name)
    }

    pub fn freeze_global(&mut self, name: SymbolId) {
        self.heap.freeze_global(name);
    }

    pub fn global_is_readonly(&self, name: SymbolId) -> bool {
        self.heap.global_is_readonly(name)
    }

    pub fn hook_global(&mut self, name: SymbolId, reads: bool) {
        self.heap.hook_global(name, reads);
    }

    pub fn global_hooks(&self, name: SymbolId) -> u8 {
        self.heap.global_hooks(name)
    }

    /// Point a handle at a different object. The old one loses this root.
    pub fn set(&mut self, handle: Handle<'h>, value: Value) {
        self.storable(value);
        self.heap.roots[handle.index] = value;
    }

    /// Assert that a `Value` about to be stored still refers to a live object.
    ///
    /// Reading a `Value` out of the heap is always safe; the object is live at the
    /// moment it is read. Keeping that bare `Value` across a collection and storing it
    /// afterwards is not, and it is the one way to reach a dangling pointer without
    /// writing `unsafe` — the mistake this catches. Debug builds and Miri pay one
    /// masked load for it; release builds pay nothing.
    ///
    /// It is `debug_assert`-shaped on purpose. A release build cannot afford a check on
    /// every store, and the discipline that prevents the mistake is the borrow checker
    /// on [`HandleScope`], not this.
    #[inline]
    fn storable(&self, value: Value) {
        if cfg!(debug_assertions) {
            if let Some(pointer) = value.as_heap() {
                // SAFETY: reading one byte of a header. The API gives no way to make a
                // heap `Value` except from this heap, so the pointer is a cell of it.
                let flags = unsafe { (*pointer.cast::<Header>().as_ptr()).flags };
                assert!(
                    flags & ALLOCATED != 0,
                    "storing {value:?}, which was collected"
                );
            }
        }
    }

    /// Force a collection. Everything not reachable from a live handle is freed.
    pub fn collect(&mut self) {
        self.heap.collect();
    }

    pub fn stats(&self) -> Stats {
        self.heap.stats()
    }

    /// `GC.stat`'s numbers (#36).
    pub fn gc_stat(&self) -> GcStat {
        let heap = &*self.heap;
        let since = heap.total_allocated - heap.allocated_at_gc;
        GcStat {
            stats: heap.stats(),
            available_slots: heap
                .blocks
                .iter()
                .map(|block| BLOCK_SIZE / block.cell_size)
                .sum(),
            live_slots: heap.live_objects + usize::try_from(since).unwrap_or(usize::MAX),
            time: heap.gc_time,
        }
    }

    /// `GC.start`: collect at the interpreter's next call or return, where
    /// the loop can first let go of handles it no longer needs.
    pub(crate) fn request_collection(&mut self) {
        self.heap.gc_requested = true;
    }

    /// Whether a collection was asked for, forgetting that it was.
    pub(crate) fn take_collection_request(&mut self) -> bool {
        std::mem::take(&mut self.heap.gc_requested)
    }

    /// `GC.disable` with `true`, `GC.enable` with `false`; answers whether
    /// collection was disabled before.
    pub(crate) fn set_gc_disabled(&mut self, disabled: bool) -> bool {
        std::mem::replace(&mut self.heap.gc_disabled, disabled)
    }

    pub(crate) fn set_gc_stress(&mut self, stress: bool) {
        self.heap.gc_stress = stress;
    }

    pub(crate) fn gc_stress(&self) -> bool {
        self.heap.gc_stress
    }

    /// The weak table of the map object `owner`, made empty on first use.
    pub(crate) fn weak_table(&mut self, owner: Handle<'h>, ephemeron: bool) -> &mut WeakTable {
        let address = self.address(owner);
        self.heap.weak.entry(address).or_insert_with(|| WeakTable {
            ephemeron,
            ..WeakTable::default()
        })
    }

    /// The callables finalizing `object`, oldest first; empty when it has none.
    pub(crate) fn finalizers(&self, object: Handle<'h>) -> &[Value] {
        self.heap
            .finalizers
            .get(&self.address(object))
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn add_finalizer(&mut self, object: Handle<'h>, callable: Value) {
        let address = self.address(object);
        self.heap
            .finalizers
            .entry(address)
            .or_default()
            .push(callable);
    }

    /// `ObjectSpace.undefine_finalizer`.
    pub(crate) fn remove_finalizers(&mut self, object: Handle<'h>) {
        let address = self.address(object);
        self.heap.finalizers.remove(&address);
    }

    pub(crate) fn has_pending_finalizers(&self) -> bool {
        !self.heap.pending_finalizers.is_empty()
    }

    /// The finalizers a collection queued, with the id each is handed —
    /// and, when `everything` is set, every one still registered, which is
    /// what the program's end runs. Either way the heap forgets them.
    pub(crate) fn take_finalizers(&mut self, everything: bool) -> Vec<(i64, Vec<Value>)> {
        let mut taken = std::mem::take(&mut self.heap.pending_finalizers);
        if everything {
            let mut registered: Vec<_> = self.heap.finalizers.drain().collect();
            // By address, so a run at exit is the same from one run to the next.
            registered.sort_by_key(|&(address, _)| address);
            taken.extend(
                registered
                    .into_iter()
                    .map(|(address, callables)| (object_id(address), callables)),
            );
        }
        taken
    }

    /// Every object this heap holds that Ruby can see — one with a class —
    /// in no promised order, for `ObjectSpace.each_object` (#36).
    ///
    /// Not only the live ones: an object that became garbage after the last
    /// collection is still in its cell, intact, and is reported, as CRuby's
    /// walk reports one. The caller roots what it keeps before allocating,
    /// since a collection would free those.
    pub(crate) fn objects(&self) -> Vec<Value> {
        let heap = &*self.heap;
        let mut out = Vec::new();
        let mut visit = |header: *mut Header| {
            // SAFETY: a block is zeroed on arrival and a swept cell has its
            // flags cleared, so `ALLOCATED` says the header was written by
            // `allocate` and the cell has not been freed since.
            let header = unsafe { &*header };
            if header.flags & ALLOCATED != 0 && header.class.is_some() {
                out.push(Value::heap(NonNull::from(header).cast::<()>()));
            }
        };
        for block in &heap.blocks {
            for i in 0..BLOCK_SIZE / block.cell_size {
                // SAFETY: `i * cell_size` stays inside the block by construction.
                visit(unsafe { block.memory.as_ptr().add(i * block.cell_size) }.cast());
            }
        }
        for &(pointer, _) in &heap.large {
            visit(pointer.as_ptr());
        }
        out
    }

    /// Forget every handle past the first `len`, as dropping a nested scope
    /// would. The interpreter loop uses it between instructions, where no
    /// handle it took is still in use, and roots what its stack and frames
    /// hold again straight after.
    ///
    /// # Panics
    ///
    /// If `len` is below this scope's own base: those handles are a parent's.
    pub(crate) fn release_roots(&mut self, len: usize) {
        assert!(len >= self.base, "releasing a parent scope's handles");
        self.heap.roots.truncate(len);
    }

    /// How many handles are rooted, across every open scope.
    pub fn rooted(&self) -> usize {
        self.heap.roots.len()
    }

    fn object(&self, handle: Handle<'h>) -> Obj {
        let value = self.get(handle);
        Obj(value
            .as_heap()
            .expect("a handle from `alloc` is a heap object")
            .cast())
    }

    /// The slot count, or byte count, the object was allocated with.
    pub fn len(&self, handle: Handle<'h>) -> u32 {
        // SAFETY: the handle roots a live object of this heap.
        unsafe { (*self.object(handle).header()).len }
    }

    pub fn payload(&self, handle: Handle<'h>) -> Payload {
        // SAFETY: as above.
        unsafe { (*self.object(handle).header()).payload }
    }

    pub fn class(&self, handle: Handle<'h>) -> Option<Value> {
        // SAFETY: as above.
        unsafe { (*self.object(handle).header()).class }
    }

    /// Which instance variables the object holds. [`ShapeId::NONE`] when it
    /// cannot hold any — see `shape.rs`, and `interp.rs`'s `ivar_get`.
    pub fn shape(&self, handle: Handle<'h>) -> ShapeId {
        // SAFETY: as above.
        unsafe { (*self.object(handle).header()).shape }
    }

    /// Whether the object's instance variables are kept in the side table.
    pub(crate) fn has_side_ivars(&self, handle: Handle<'h>) -> bool {
        // SAFETY: as above.
        unsafe { (*self.object(handle).header()).flags & SIDE_IVARS != 0 }
    }

    /// The storage object behind a side-table object's instance variables,
    /// or nil before the first is written.
    pub(crate) fn side_ivars(&self, handle: Handle<'h>) -> Value {
        let address = self.object(handle).0.as_ptr() as usize;
        self.heap
            .side_ivars
            .get(&address)
            .copied()
            .unwrap_or(Value::NIL)
    }

    /// Give the object side-table storage. Cannot collect.
    pub(crate) fn set_side_ivars(&mut self, handle: Handle<'h>, storage: Value) {
        self.storable(storage);
        let object = self.object(handle);
        // SAFETY: as above.
        unsafe { (*object.header()).flags |= SIDE_IVARS }
        self.heap
            .side_ivars
            .insert(object.0.as_ptr() as usize, storage);
    }

    /// Record a transition. Cannot collect; the caller has already written the
    /// value the new shape describes.
    pub fn set_shape(&mut self, handle: Handle<'h>, shape: ShapeId) {
        let object = self.object(handle);
        // SAFETY: as above.
        unsafe { (*object.header()).shape = shape }
    }

    /// # Panics
    ///
    /// If the object holds bytes, or `index` is past its length.
    pub fn slot(&self, handle: Handle<'h>, index: usize) -> Value {
        let object = self.object(handle);
        self.check_slot(object, index);
        // SAFETY: bounds and payload kind checked; `allocate` initialised every slot.
        unsafe { ptr::read(object.slots().add(index)) }
    }

    /// Store into a slot. Cannot collect, so a bare `Value` is safe to pass.
    ///
    /// # Panics
    ///
    /// If the object holds bytes, or `index` is past its length.
    pub fn set_slot(&mut self, handle: Handle<'h>, index: usize, value: Value) {
        let object = self.object(handle);
        self.check_slot(object, index);
        self.storable(value);
        // SAFETY: as above. No write barrier: the collector is stop-the-world and
        // non-generational, so nothing depends on noticing this store.
        unsafe { ptr::write(object.slots().add(index), value) }
    }

    fn check_slot(&self, object: Obj, index: usize) {
        // SAFETY: the handle roots a live object of this heap.
        let header = unsafe { &*object.header() };
        assert_eq!(
            header.payload,
            Payload::Slots,
            "object holds bytes, not slots"
        );
        assert!(
            index < header.len as usize,
            "slot {index} is past the object's {} slots",
            header.len
        );
    }

    /// # Panics
    ///
    /// If the object holds slots.
    pub fn bytes(&self, handle: Handle<'h>) -> &[u8] {
        let object = self.object(handle);
        // SAFETY: the handle roots a live object of this heap.
        let header = unsafe { &*object.header() };
        assert_eq!(
            header.payload,
            Payload::Bytes,
            "object holds slots, not bytes"
        );
        // SAFETY: `len` bytes were allocated and zeroed after the header.
        unsafe { std::slice::from_raw_parts(object.bytes(), header.len as usize) }
    }

    /// # Panics
    ///
    /// If the object holds slots.
    /// The cell's address, which is what `Object#object_id` is derived from.
    ///
    /// Stable only while the collector does not move objects. Phase 6's moving
    /// GC replaces this with a side table keyed by the id already handed out.
    #[must_use]
    pub fn address(&self, handle: Handle<'h>) -> usize {
        self.object(handle).header() as usize
    }

    /// Mark the object frozen. Idempotent, and one-way: Ruby has no `unfreeze`.
    /// A frozen String is no longer chilled.
    pub fn freeze(&mut self, handle: Handle<'h>) {
        let object = self.object(handle);
        // SAFETY: `handle` is rooted, so the cell it names is live.
        let flags = unsafe { &mut (*object.header()).flags };
        *flags = (*flags | FROZEN) & !(CHILLED | CHILLED_SYMBOL);
    }

    /// Mark a String chilled (#263): by a literal, or by `Symbol#to_s` when
    /// `symbol` is set.
    pub fn chill(&mut self, handle: Handle<'h>, symbol: bool) {
        let object = self.object(handle);
        let bit = if symbol { CHILLED_SYMBOL } else { CHILLED };
        // SAFETY: `handle` is rooted, so the cell it names is live.
        unsafe { (*object.header()).flags |= bit };
    }

    /// Clear the object's chilled mark, answering what it was: `None` for an
    /// object that was not chilled, `Some(true)` for one `Symbol#to_s` made.
    pub fn unchill(&mut self, handle: Handle<'h>) -> Option<bool> {
        let object = self.object(handle);
        // SAFETY: as above.
        let flags = unsafe { &mut (*object.header()).flags };
        let was = *flags & (CHILLED | CHILLED_SYMBOL);
        *flags &= !(CHILLED | CHILLED_SYMBOL);
        (was != 0).then_some(was & CHILLED_SYMBOL != 0)
    }

    /// Whether the object is chilled, and so `+@` copies it: `None` when it
    /// is not, `Some(true)` when `Symbol#to_s` made it.
    #[must_use]
    pub fn chilled(&self, handle: Handle<'h>) -> Option<bool> {
        let object = self.object(handle);
        // SAFETY: as above.
        let flags = unsafe { (*object.header()).flags };
        (flags & (CHILLED | CHILLED_SYMBOL) != 0).then_some(flags & CHILLED_SYMBOL != 0)
    }

    /// Mark a `Hash` as a keywords hash (#200). One-way: a marked hash is
    /// only ever unmarked by being copied.
    pub fn mark_keywords(&mut self, handle: Handle<'h>) {
        let object = self.object(handle);
        // SAFETY: `handle` is rooted, so the cell it names is live.
        unsafe { (*object.header()).flags |= KEYWORDS };
    }

    /// Whether [`HandleScope::mark_keywords`] has been called on this object.
    #[must_use]
    pub fn is_keywords(&self, handle: Handle<'h>) -> bool {
        let object = self.object(handle);
        // SAFETY: as above.
        unsafe { (*object.header()).flags & KEYWORDS != 0 }
    }

    /// Whether [`HandleScope::freeze`] has been called on this object.
    #[must_use]
    pub fn is_frozen(&self, handle: Handle<'h>) -> bool {
        let object = self.object(handle);
        // SAFETY: as above.
        unsafe { (*object.header()).flags & FROZEN != 0 }
    }

    pub fn bytes_mut(&mut self, handle: Handle<'h>) -> &mut [u8] {
        let object = self.object(handle);
        // SAFETY: the handle roots a live object of this heap.
        let header = unsafe { &*object.header() };
        assert_eq!(
            header.payload,
            Payload::Bytes,
            "object holds slots, not bytes"
        );
        // SAFETY: as above, and `&mut self` makes this the only reference.
        unsafe { std::slice::from_raw_parts_mut(object.bytes(), header.len as usize) }
    }
}

impl Drop for HandleScope<'_> {
    fn drop(&mut self) {
        self.heap.roots.truncate(self.base);
    }
}

impl fmt::Debug for HandleScope<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandleScope")
            .field("base", &self.base)
            .field("handles", &(self.heap.roots.len() - self.base))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain of `len` one-slot objects, rooted only at its head. Each link is
    /// allocated in a nested scope that immediately drops, so the only thing keeping
    /// it alive is the slot of the link before it — which is what makes this a test of
    /// tracing rather than of the root stack.
    fn chain<'h>(scope: &mut HandleScope<'h>, len: usize) -> (Handle<'h>, Handle<'h>) {
        let head = scope.alloc(None, Payload::Slots, 1);
        let cursor = scope.alloc(None, Payload::Slots, 1);
        let start = scope.get(head);
        scope.set(cursor, start);
        for _ in 0..len {
            let mut inner = scope.nested();
            let next = inner.alloc(None, Payload::Slots, 1);
            let next_value = inner.get(next);
            inner.set_slot(cursor, 0, next_value);
            inner.set(cursor, next_value);
        }
        (head, cursor)
    }

    /// Walk from `head`, counting the objects still readable.
    fn chain_len<'h>(scope: &mut HandleScope<'h>, head: Handle<'h>, cursor: Handle<'h>) -> usize {
        let start = scope.get(head);
        scope.set(cursor, start);
        let mut count = 1;
        loop {
            let next = scope.slot(cursor, 0);
            if next.is_nil() {
                return count;
            }
            scope.set(cursor, next);
            count += 1;
        }
    }

    // T1
    #[test]
    fn the_header_is_two_words() {
        assert_eq!(size_of::<Header>(), 16);
        assert_eq!(align_of::<Header>(), 8);
        // Every size class holds a header and at least one slot, or it is a class an
        // object can ask for and never fit in.
        for cell in SIZE_CLASSES {
            assert!(cell >= size_of::<Header>() + size_of::<Value>(), "{cell}");
            assert_eq!(cell % 16, 0, "{cell} would misalign the next cell");
        }
    }

    /// Side-table instance variables live exactly as long as their object: the
    /// storage is traced from an owner that holds bytes and has no slot to
    /// reach it by, and the entry goes when the owner does — so the next
    /// object at that address starts with none.
    #[test]
    fn side_ivars_are_traced_from_their_object_and_dropped_with_it() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let kept = scope.alloc(None, Payload::Bytes, 8);
        {
            let mut inner = scope.nested();
            let storage = inner.alloc(None, Payload::Slots, 2);
            inner.set_slot(storage, 0, Value::TRUE);
            let storage = inner.get(storage);
            inner.set_side_ivars(kept, storage);
            // A second owner that nothing keeps.
            let lost = inner.alloc(None, Payload::Bytes, 8);
            let storage = inner.alloc(None, Payload::Slots, 2);
            let storage = inner.get(storage);
            inner.set_side_ivars(lost, storage);
        }
        assert_eq!(scope.heap.side_ivars.len(), 2);
        scope.collect();
        assert_eq!(scope.heap.side_ivars.len(), 1);
        assert_eq!(scope.stats().live_objects, 2);
        assert!(scope.has_side_ivars(kept));
        let storage = scope.side_ivars(kept);
        let storage = scope.root(storage);
        assert_eq!(scope.slot(storage, 0), Value::TRUE);
    }

    /// A weak map lets go of an entry once either half is unreachable, keeps
    /// one whose halves are both held or are immediates, and goes with its
    /// map (#36).
    #[test]
    fn weak_entries_clear_when_a_half_dies() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let map = scope.alloc(None, Payload::Slots, 1);
        let key = scope.alloc(None, Payload::Slots, 1);
        let value = scope.alloc(None, Payload::Slots, 1);
        let (key_value, value_value) = (scope.get(key), scope.get(value));
        {
            let mut inner = scope.nested();
            let lost_key = inner.alloc(None, Payload::Slots, 1);
            let lost_value = inner.alloc(None, Payload::Slots, 1);
            let (lost_key, lost_value) = (inner.get(lost_key), inner.get(lost_value));
            let table = inner.weak_table(map, false);
            table.insert(key_value, value_value, 0);
            table.insert(lost_key, value_value, 0);
            table.insert(key_value, value_value, 0);
            table.insert(Value::TRUE, lost_value, 0);
            table.insert(Value::NIL, Value::FALSE, 0);
            assert_eq!(table.len(), 4);
            // A second map that nothing keeps.
            let lost_map = inner.alloc(None, Payload::Slots, 1);
            inner
                .weak_table(lost_map, false)
                .insert(key_value, value_value, 0);
        }
        assert_eq!(scope.heap.weak.len(), 2);
        scope.collect();
        assert_eq!(scope.heap.weak.len(), 1);
        let table = scope.weak_table(map, false);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(key_value), Some(value_value));
        assert_eq!(table.get(Value::NIL), Some(Value::FALSE));
        assert_eq!(table.get(Value::TRUE), None);
        assert_eq!(scope.stats().live_objects, 3);
    }

    /// A `WeakKeyMap` value lives as long as its key, even when nothing else
    /// holds it — and through a chain, where one map's value is another's
    /// key, which is what the repeated pass in `trace_weak` is for.
    #[test]
    fn ephemeron_values_live_as_long_as_their_keys() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let first = scope.alloc(None, Payload::Slots, 1);
        let second = scope.alloc(None, Payload::Slots, 1);
        let key = scope.alloc(None, Payload::Slots, 1);
        let key_value = scope.get(key);
        let (middle, last) = {
            let mut inner = scope.nested();
            let middle = inner.alloc(None, Payload::Slots, 1);
            let last = inner.alloc(None, Payload::Slots, 1);
            let (middle, last) = (inner.get(middle), inner.get(last));
            // Added to the second map first, so one pass over the maps in
            // either order still has to come back for it.
            inner.weak_table(second, true).insert(middle, last, 7);
            inner.weak_table(first, true).insert(key_value, middle, 3);
            let dead_key = inner.alloc(None, Payload::Slots, 1);
            let dead_value = inner.alloc(None, Payload::Slots, 1);
            let (dead_key, dead_value) = (inner.get(dead_key), inner.get(dead_value));
            inner
                .weak_table(first, true)
                .insert(dead_key, dead_value, 3);
            (middle, last)
        };
        scope.collect();
        // The two maps, the key, and the chain it holds up.
        assert_eq!(scope.stats().live_objects, 5);
        assert_eq!(scope.weak_table(first, true).get(key_value), Some(middle));
        assert_eq!(scope.weak_table(first, true).len(), 1);
        assert_eq!(scope.weak_table(second, true).get(middle), Some(last));
        let hashed: Vec<Value> = scope.weak_table(first, true).keys_with_hash(3).collect();
        assert_eq!(hashed, [key_value]);
    }

    /// A finalizer is queued once its object is collected, with the id the
    /// object answered to, and the callable outlives the object: it is a
    /// root, the object is not. An object a finalizer holds is kept alive.
    #[test]
    fn finalizers_are_queued_once_their_object_is_collected() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let callable = scope.alloc(None, Payload::Slots, 1);
        let callable_value = scope.get(callable);
        let held = scope.alloc(None, Payload::Slots, 1);
        let id = {
            let mut inner = scope.nested();
            let doomed = inner.alloc(None, Payload::Slots, 1);
            inner.add_finalizer(doomed, callable_value);
            // Held only by its own finalizer.
            let selfish = inner.alloc(None, Payload::Slots, 1);
            let selfish_value = inner.get(selfish);
            inner.add_finalizer(selfish, selfish_value);
            inner.add_finalizer(held, callable_value);
            object_id(inner.address(doomed))
        };
        assert!(!scope.has_pending_finalizers());
        scope.collect();
        // The callable, the object kept by a handle, and the one its own
        // finalizer holds.
        assert_eq!(scope.stats().live_objects, 3);
        assert!(scope.has_pending_finalizers());
        let taken = scope.take_finalizers(false);
        assert_eq!(taken, [(id, vec![callable_value])]);
        // At exit, the rest: every one still registered.
        let mut rest = scope.take_finalizers(true);
        rest.sort_by_key(|(id, _)| *id);
        assert_eq!(rest.len(), 2);
        assert!(scope.take_finalizers(true).is_empty());
    }

    // T2
    #[test]
    fn size_classes_cover_their_range_exactly() {
        // Every byte count up to the largest cell lands in the smallest class that
        // holds it; one byte past it goes to the large-object space.
        for bytes in 1..=MAX_CELL {
            let index = size_class(bytes).expect("under the cap");
            let cell = SIZE_CLASSES[index];
            assert!(cell >= bytes, "{bytes} does not fit in {cell}");
            if index > 0 {
                assert!(
                    SIZE_CLASSES[index - 1] < bytes,
                    "{bytes} should have fitted in {}",
                    SIZE_CLASSES[index - 1]
                );
            }
        }
        assert_eq!(size_class(MAX_CELL + 1), None);
        assert_eq!(size_class(usize::MAX), None);

        // The slot counts the PRD promises, and the boundary each one implies.
        assert_eq!(
            SIZE_CLASSES.map(|cell| (cell - size_of::<Header>()) / size_of::<Value>()),
            [2, 6, 14, 30, 62]
        );
        assert_eq!(size_class(Heap::object_size(Payload::Slots, 2)), Some(0));
        assert_eq!(size_class(Heap::object_size(Payload::Slots, 3)), Some(1));
        assert_eq!(size_class(Heap::object_size(Payload::Slots, 62)), Some(4));
        assert_eq!(size_class(Heap::object_size(Payload::Slots, 63)), None);

        // Bytes round up to a whole word so the next cell stays aligned.
        assert_eq!(Heap::object_size(Payload::Bytes, 1), 24);
        assert_eq!(Heap::object_size(Payload::Bytes, 8), 24);
        assert_eq!(Heap::object_size(Payload::Bytes, 9), 32);
    }

    #[test]
    fn a_fresh_object_reads_as_nil_slots_or_zero_bytes() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();

        let slots = scope.alloc(None, Payload::Slots, 5);
        assert_eq!(scope.len(slots), 5);
        assert_eq!(scope.payload(slots), Payload::Slots);
        assert_eq!(scope.class(slots), None);
        for i in 0..5 {
            assert_eq!(scope.slot(slots, i), Value::NIL, "slot {i}");
        }

        let bytes = scope.alloc(None, Payload::Bytes, 5);
        assert_eq!(scope.payload(bytes), Payload::Bytes);
        assert_eq!(scope.bytes(bytes), &[0; 5]);
        scope.bytes_mut(bytes).copy_from_slice(b"hello");
        assert_eq!(scope.bytes(bytes), b"hello");
    }

    #[test]
    fn a_reused_cell_does_not_leak_the_dead_object_it_held() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        {
            let mut inner = scope.nested();
            let bytes = inner.alloc(None, Payload::Bytes, 8);
            inner.bytes_mut(bytes).copy_from_slice(b"secrets!");
        }
        scope.collect();
        let reused = scope.alloc(None, Payload::Bytes, 8);
        assert_eq!(scope.bytes(reused), &[0; 8]);
    }

    // T3
    #[test]
    fn large_objects_allocate_and_are_reclaimed() {
        // Comfortably past the largest size class either way.
        #[cfg(not(miri))]
        const LARGE_SLOTS: u32 = 10_000;
        #[cfg(miri)]
        const LARGE_SLOTS: u32 = 200;

        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let big = scope.alloc(None, Payload::Slots, LARGE_SLOTS);
        assert_eq!(scope.stats().large_objects, 1);
        assert_eq!(scope.stats().blocks, 0, "a large object needs no block");

        let marker = Value::fixnum(7).unwrap();
        scope.set_slot(big, LARGE_SLOTS as usize - 1, marker);
        scope.collect();
        assert_eq!(
            scope.slot(big, LARGE_SLOTS as usize - 1),
            marker,
            "a live large object survives"
        );
        assert_eq!(scope.stats().large_objects, 1);

        {
            let mut inner = scope.nested();
            inner.alloc(None, Payload::Bytes, 10 * MAX_CELL as u32);
            assert_eq!(inner.stats().large_objects, 2);
        }
        scope.collect();
        assert_eq!(scope.stats().large_objects, 1, "the unrooted one was freed");
    }

    // T4
    #[test]
    fn a_nested_scope_pops_its_own_handles_only() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let kept = scope.alloc(None, Payload::Slots, 1);
        assert_eq!(scope.rooted(), 1);
        {
            let mut inner = scope.nested();
            inner.alloc(None, Payload::Slots, 1);
            inner.alloc(None, Payload::Slots, 1);
            assert_eq!(inner.rooted(), 3);
            {
                let mut innermost = inner.nested();
                innermost.alloc(None, Payload::Slots, 1);
                assert_eq!(innermost.rooted(), 4);
            }
            assert_eq!(inner.rooted(), 3);
        }
        assert_eq!(scope.rooted(), 1);

        scope.collect();
        assert_eq!(
            scope.stats().live_objects,
            1,
            "only the outer handle rooted one"
        );
        assert_eq!(scope.slot(kept, 0), Value::NIL, "and it is still readable");
    }

    #[test]
    fn unreachable_objects_go_and_reachable_ones_stay() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let root = scope.alloc(None, Payload::Slots, 2);
        {
            let mut inner = scope.nested();
            // One is linked from the rooted object, the other is only in `inner`.
            let reachable = inner.alloc(None, Payload::Slots, 1);
            let value = inner.get(reachable);
            inner.set_slot(root, 0, value);
            inner.alloc(None, Payload::Slots, 1);
            inner.collect();
            assert_eq!(inner.stats().live_objects, 3, "nothing dropped yet");
        }
        scope.collect();
        assert_eq!(scope.stats().live_objects, 2, "the unlinked one went");

        // Dropping the link collects the second one too.
        scope.set_slot(root, 0, Value::NIL);
        scope.collect();
        assert_eq!(scope.stats().live_objects, 1);
    }

    #[test]
    fn a_class_pointer_keeps_its_class_alive() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let holder = scope.alloc(None, Payload::Slots, 1);
        {
            // A class is an ordinary heap object; #8 gives it a method table. After
            // this scope drops, the class is reachable *only* through the header of
            // its instance, which is the edge the collector has to follow.
            let mut inner = scope.nested();
            let class = inner.alloc(None, Payload::Slots, 1);
            let marker = Value::fixnum(99).unwrap();
            inner.set_slot(class, 0, marker);
            let instance = inner.alloc(Some(class), Payload::Slots, 1);
            let instance_value = inner.get(instance);
            inner.set_slot(holder, 0, instance_value);
        }
        scope.collect();
        assert_eq!(
            scope.stats().live_objects,
            3,
            "holder, instance, and its class"
        );

        let instance_value = scope.slot(holder, 0);
        let instance = scope.root(instance_value);
        let class_value = scope.class(instance).expect("the class pointer survived");
        let class = scope.root(class_value);
        assert_eq!(scope.slot(class, 0), Value::fixnum(99).unwrap());
    }

    /// The one way to reach a dangling pointer from safe code, and the assertion that
    /// stops it. Debug-only: a release build cannot afford a check on every store, and
    /// what prevents the mistake is `HandleScope`, not this.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "was collected")]
    fn storing_a_value_that_was_collected_is_caught() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let holder = scope.alloc(None, Payload::Slots, 1);
        let dangling = {
            let mut inner = scope.nested();
            let doomed = inner.alloc(None, Payload::Slots, 1);
            inner.get(doomed)
        };
        scope.collect();
        scope.set_slot(holder, 0, dangling);
    }

    // T5
    #[test]
    fn a_deep_chain_does_not_overflow_the_mark_stack() {
        // Recursion here would be a stack overflow inside the collector, with no Ruby
        // frame to blame it on. A million links is past any default stack.
        #[cfg(not(miri))]
        const LINKS: usize = 1_000_000;
        // Miri is checking the pointer arithmetic, not the depth, and it is four
        // orders of magnitude slower.
        #[cfg(miri)]
        const LINKS: usize = 256;
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let (head, cursor) = chain(&mut scope, LINKS);
        scope.collect();
        assert_eq!(chain_len(&mut scope, head, cursor), LINKS + 1);
        // The head and every link. `cursor` started on its own placeholder object and
        // moved off it, so that one is garbage — which is the collector agreeing that
        // a handle roots a slot, not an object.
        assert_eq!(
            scope.stats().live_objects,
            LINKS + 1,
            "the whole chain survived"
        );
    }

    #[test]
    fn a_cycle_is_traced_once_and_collected_together() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        {
            let mut inner = scope.nested();
            let a = inner.alloc(None, Payload::Slots, 1);
            let b = inner.alloc(None, Payload::Slots, 1);
            let (a_value, b_value) = (inner.get(a), inner.get(b));
            inner.set_slot(a, 0, b_value);
            inner.set_slot(b, 0, a_value);
            // Tracing terminates: the mark bit is set before the object is queued.
            inner.collect();
            assert_eq!(inner.stats().live_objects, 2);
        }
        // Unreachable, but pointing at each other. Reference counting would keep both
        // for ever; a tracing collector frees them together.
        scope.collect();
        assert_eq!(scope.stats().live_objects, 0);
    }

    // T6
    #[test]
    fn swept_cells_are_reused() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        let first = {
            let mut inner = scope.nested();
            let dead = inner.alloc(None, Payload::Slots, 1);
            inner.get(dead).as_heap()
        };
        scope.collect();
        let fresh = scope.alloc(None, Payload::Slots, 1);
        assert_eq!(
            scope.get(fresh).as_heap(),
            first,
            "the swept cell came back"
        );
        assert_eq!(scope.stats().blocks, 1, "and no second block was taken");
    }

    // T7
    #[test]
    // Crossing a 1 MiB threshold takes tens of thousands of allocations, and every
    // unsafe path it touches is covered by a test above that Miri does run.
    #[cfg_attr(miri, ignore)]
    fn allocation_alone_triggers_collection() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        assert_eq!(scope.stats().collections, 0);

        // Twice the initial threshold of garbage, with no `collect` call anywhere.
        let objects = 2 * MIN_GC_BYTES / SIZE_CLASSES[0];
        for _ in 0..objects {
            let mut inner = scope.nested();
            inner.alloc(None, Payload::Slots, 2);
        }
        let stats = scope.stats();
        assert!(stats.collections > 0, "{stats:?}");
        assert!(
            stats.live_bytes < MIN_GC_BYTES,
            "garbage was reclaimed: {stats:?}"
        );
    }

    #[test]
    // Crossing a 1 MiB threshold takes tens of thousands of allocations, and every
    // unsafe path it touches is covered by a test above that Miri does run.
    #[cfg_attr(miri, ignore)]
    fn the_threshold_grows_with_the_live_set_rather_than_thrashing() {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        // A live set well past the floor: every object stays rooted.
        let objects = 4 * MIN_GC_BYTES / SIZE_CLASSES[0];
        for _ in 0..objects {
            scope.alloc(None, Payload::Slots, 2);
        }
        let stats = scope.stats();
        // With a fixed threshold this would collect once per `MIN_GC_BYTES` for ever:
        // the live set never shrinks, so every collection frees nothing and runs again.
        assert!(
            stats.collections < 8,
            "{} collections for {objects} live objects: {stats:?}",
            stats.collections
        );
    }

    // T9 — the issue's definition of done.
    #[test]
    fn ten_million_objects_with_a_forced_gc_every_thousand() {
        #[cfg(not(miri))]
        const BATCHES: usize = 10_000;
        #[cfg(not(miri))]
        const PER_BATCH: usize = 1_000;
        // Under Miri the shape of the loop is what is being checked — allocate a
        // batch, drop its scope, collect, reuse the cells — not the count.
        #[cfg(miri)]
        const BATCHES: usize = 4;
        #[cfg(miri)]
        const PER_BATCH: usize = 8;

        let mut heap = Heap::new();
        let mut scope = heap.scope();

        // A small tree that must be intact at the end: four children reachable only
        // through the keeper's slots, so all 10,000 collections have to trace them
        // rather than merely leave them alone.
        let keeper = scope.alloc(None, Payload::Slots, 4);
        for i in 0..4 {
            let mut inner = scope.nested();
            let child = inner.alloc(None, Payload::Slots, 1);
            let marker = Value::fixnum(i as i64).unwrap();
            inner.set_slot(child, 0, marker);
            let child_value = inner.get(child);
            inner.set_slot(keeper, i, child_value);
        }

        for _ in 0..BATCHES {
            {
                let mut inner = scope.nested();
                for _ in 0..PER_BATCH {
                    inner.alloc(None, Payload::Slots, 2);
                }
            }
            scope.collect();
        }

        let stats = scope.stats();
        assert_eq!(stats.collections, BATCHES as u64);
        // Without this the test's name is the only evidence that ten million objects
        // were allocated, and an optimiser that elided the loop would still pass.
        assert_eq!(
            stats.total_allocated,
            (BATCHES * PER_BATCH + 5) as u64,
            "the batches, the keeper, and its four children"
        );
        assert_eq!(
            stats.live_objects, 5,
            "the keeper and its four children: {stats:?}"
        );
        // Ten million objects through a handful of blocks is the claim that the free
        // lists are reused rather than the heap growing.
        assert!(stats.blocks <= 2, "{stats:?}");

        for i in 0..4 {
            let child = scope.slot(keeper, i);
            let mut inner = scope.nested();
            let handle = inner.root(child);
            assert_eq!(inner.slot(handle, 0), Value::fixnum(i as i64).unwrap());
        }
    }
}
