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
//! | 14 | reserved for #8's shape id | — |
//! | 16.. | `len` slots, or `len` bytes | — |

use std::alloc;
use std::fmt;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use crate::class::Classes;
use crate::value::Value;

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
    /// #8's shape id. Two bytes, already paid for by the alignment of `class`.
    _reserved: [u8; 2],
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
    definitions: crate::method::Definitions,
    regexps: crate::regexp::Regexps,
    /// The `MatchData` the last successful match produced, which is what `$~`
    /// and `$1` read.
    ///
    /// ponytail: one per heap. Ruby scopes `$~` to the method frame and to the
    /// thread; `back-references_spec.rb` has an example for the thread half.
    /// Give it a frame slot when frames carry their own specials, and a
    /// Ractor-local when threads arrive.
    last_match: Value,
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
            definitions: crate::method::Definitions::new(),
            regexps: crate::regexp::Regexps::new(),
            last_match: Value::NIL,
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

    /// The last successful match, or nil.
    pub fn last_match(&self) -> Value {
        self.last_match
    }

    pub fn set_last_match(&mut self, value: Value) {
        self.last_match = value;
    }

    pub fn definitions_mut(&mut self) -> &mut crate::method::Definitions {
        &mut self.definitions
    }

    pub fn classes_mut(&mut self) -> &mut Classes {
        &mut self.classes
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
        if self.bytes_since_gc >= self.next_gc {
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
                    _reserved: [0; 2],
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
        self.mark();
        self.sweep();
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
        Heap::shade(&mut self.mark_stack, self.last_match);
        // R3: a worklist, not recursion. A Ruby program can build a chain a million
        // objects deep, and a recursive tracer turns that into a stack overflow inside
        // the collector, with no Ruby frame to blame it on.
        while let Some(obj) = self.mark_stack.pop() {
            // SAFETY: only live objects of this heap are ever shaded, and `shade` marked
            // this one before pushing it.
            let header = unsafe { &*obj.header() };
            let (class, len, payload) = (header.class, header.len, header.payload);
            if let Some(class) = class {
                Heap::shade(&mut self.mark_stack, class);
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
                    // ponytail: a dead object is reclaimed by being relinked, and
                    // nothing runs. `ObjectSpace.define_finalizer` is phase 2 in
                    // engine.md and queues the finalizer here.
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

    /// The method bodies this heap's class table points at.
    pub fn definitions(&self) -> &crate::method::Definitions {
        self.heap.definitions()
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

    pub fn last_match(&self) -> Value {
        self.heap.last_match()
    }

    pub fn set_last_match(&mut self, value: Value) {
        self.heap.set_last_match(value);
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
    pub fn freeze(&mut self, handle: Handle<'h>) {
        let object = self.object(handle);
        // SAFETY: `handle` is rooted, so the cell it names is live.
        unsafe { (*object.header()).flags |= FROZEN };
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
