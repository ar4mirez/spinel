# PRD 0007 — `Heap`: mark-sweep GC with size classes, and `HandleScope`

Tracks [#7](https://github.com/ar4mirez/spinel/issues/7). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:L`, `area:engine`, `area:gc`.

## Objective

[#6](https://github.com/ar4mirez/spinel/issues/6) settled what a `Value` is. Four of its
five tags are immediates and allocate nothing; the fifth is a pointer to memory that does
not exist yet. This slice is that memory: one `Heap` per Ractor, a precise non-moving
mark-sweep collector over size-classed free lists, and the `HandleScope` discipline that
keeps a Rust-held object visible to the collector.

The discipline is the load-bearing part. A precise collector is only as good as its root
set, and the root set is only as good as the weakest primitive that forgets to register
one. That kind of bug does not fail where it is written: it fails later, in unrelated
code, as an object whose class pointer has become a free-list link. So the rule here is
not documentation. `Heap` has no method that allocates. Allocation is a method on
`HandleScope`, which returns a `Handle` and never a bare pointer, so an unrooted object
is not a mistake a primitive can make — it is a program that does not compile.

## Non-goals

- **Classes and shapes.** [#8](https://github.com/ar4mirez/spinel/issues/8) brings
  bootstrap classes, method tables, and the shape tree. The header carries a class
  pointer and the collector traces it, because a collector that cannot follow a class
  pointer would have to be rewritten to gain one; nothing in this slice can *supply* a
  class, so the tests pass an ordinary heap object in that position, which is exactly
  what a class will be. The shape id is not a field yet — it goes in the two reserved
  bytes named in R1.
- **Generational, incremental, or moving collection.** engine.md picks non-moving
  mark-sweep as the default and names the upgrade path. Stop-the-world and non-moving
  means there is no write barrier and no read barrier to get wrong, and `HandleScope`
  is the abstraction that makes a moving collector a contained change later.
- **Weak references and finalizers.** `ObjectSpace::WeakMap`, `WeakRef`, and
  `define_finalizer` are phase 2 in engine.md. Sweeping today reclaims a cell by pushing
  it onto a free list and runs nothing; the one comment that matters is where a finalizer
  queue would hook in, and it is in `sweep`.
- **Multiple Ractors.** One `Heap` is one Ractor's heap. `Heap` is deliberately neither
  `Send` nor `Sync` — it holds raw pointers — so the type system already refuses the
  thing [#118](https://github.com/ar4mirez/spinel/issues/118) will have to design
  properly.
- **Tuning.** The size classes, the block size, and the growth factor are the boring
  first guesses. Phase 3 has the benchmarks that would justify changing them; changing
  them before there is a program to measure is guessing with extra steps.

## Users

| User | Needs from this slice |
|---|---|
| [#8](https://github.com/ar4mirez/spinel/issues/8) classes, method tables | Somewhere to put a class object, and a collector that traces class pointers |
| [#10](https://github.com/ar4mirez/spinel/issues/10) bytecode, interpreter | Allocation that cannot lose an object mid-primitive; a VM stack that plugs into the root set |
| [#15](https://github.com/ar4mirez/spinel/issues/15) `core/*.rb` primitives | `Payload::Bytes` for `String`, `Payload::Slots` for `Array`, and one rule for holding a `Value` in Rust |
| [#120](https://github.com/ar4mirez/spinel/issues/120) JIT | A root set that a stack map can be appended to, not a scan that assumes the interpreter |
| `spinel-ext` authors (phase 5) | Handles from day one, so a moving collector later is not an ABI break |

## Requirements

### R1 — A 16-byte header that the collector can read on its own

engine.md fixes the header at 16 bytes. The collector needs three things from it and
nothing else: what this object points at, how long it is, and whether it is marked.

| offset | field | why |
|---|---|---|
| 0 | `class: Option<Value>` | the one reference every object has; `None` until #8 |
| 8 | `len: u32` | `Value` slots, or bytes — 4 billion of either |
| 12 | `flags: u8` | `MARKED`, `FROZEN`, `SHAREABLE` |
| 13 | `payload: Payload` | `Slots` or `Bytes` — whether the tracer descends |
| 14 | reserved, 2 bytes | #8's shape id |

`Payload` is the field that lets a `String` exist. Without it every object is a slot
array, and the first object holding raw bytes forces a header change and a tracer
change — which is to say, forces this slice to be done twice.

Offset 0 is also the free-list link when the cell is free. That overlap is safe in one
direction only: a free cell's link is overwritten by a full `Header` write before the
cell is ever read as an object, and a live object's `class` is never read as a link.

### R2 — Size classes, and a large-object space for what does not fit

Five classes, powers of two, chosen so the class index is one `leading_zeros`:

| cell | header + payload | slots |
|---|---|---|
| 32 B | 16 + 16 | 2 |
| 64 B | 16 + 48 | 6 |
| 128 B | 16 + 112 | 14 |
| 256 B | 16 + 240 | 30 |
| 512 B | 16 + 496 | 62 |

Cells come from 64 KiB blocks, one cell size per block, `alloc_zeroed` so that a cell
that has never held an object still reads as unmarked rather than as uninitialised
memory. Anything over 512 bytes gets its own allocation and lives in the large-object
list, which is swept by the same mark bit and freed individually.

### R3 — Marking is precise, and iterative

Every root is known: there is no stack scanning and no heuristic that asks whether a
word looks like a pointer. Today the root set is exactly the handle stack. The VM stack,
frames, the current exception, and the per-heap tables are named in engine.md and each
plugs into the same `mark_roots` when its slice lands.

The mark phase uses an explicit worklist, not recursion. A Ruby program can build a
linked list a million objects deep, and a recursive tracer turns that into a stack
overflow inside the collector — a crash with no Ruby frame to blame it on.

### R4 — `HandleScope` makes an unrooted object unrepresentable

`Heap` exposes no allocation. `HandleScope::alloc` returns `Handle<'h>`, an index into
the heap's root stack, and the scope pops back to its base on drop. `Handle` is invariant
in nothing and covariant in `'h`, which is what makes the two directions differ:

- a parent scope's handle is usable inside a nested scope, because the parent outlives it;
- a nested scope's handle is not usable in the parent, because it does not.

So the rule "a `Value` a primitive holds across an allocation must be in a scope" is
checked by the borrow checker, at the call site, in every crate that ever links `spinel-vm`.

### R5 — Collection is triggered by allocation, and never by anything else

There is no collector thread, no timer, and no interior mutability. A collection happens
inside `alloc`, when bytes allocated since the last one cross a threshold, or when a
caller says `collect()`. Every collection point is therefore a `&mut` borrow of the heap,
which is what makes "a GC in the middle of a primitive" a thing the compiler can see.

Threshold: 1 MiB to start, then twice the live bytes after each collection, floored at
1 MiB. Boring, and the only shape that does not degenerate — a fixed threshold collects
constantly once the live set passes it.

### R6 — No process-global mutable VM state

CLAUDE.md's rule, already a CI job. `Heap` owns every byte it allocates and frees it on
drop; the root stack is a field, not a thread-local; nothing is `static mut`. The
`HandleScope` linked list that engine.md describes is a single per-heap stack here, which
is the same structure with the pointers implied by the index. When suspended coroutine
stacks arrive with fibers, each gets its own root region in the same vector.

### R7 — The heap frees everything it allocated

`Heap: Drop` releases every block and every large object. A leak here is invisible in a
test that only checks liveness, so it is checked by Miri rather than asserted by a human.

## Definition of done

The issue's three boxes, and where each is discharged:

| From the issue | Where |
|---|---|
| Allocation stress test survives 10M objects with a forced GC every 1k allocations | `ten_million_objects_with_a_forced_gc_every_thousand`: 10,000,005 objects and 10,000 collections, ending with five live objects in at most two blocks |
| `HandleScope` discipline documented and used by all allocating primitives | R4: `Heap` has no allocating method, so "all primitives" is enforced by the type, not the reviewer. Module docs state the rule. |
| No `static mut`, `lazy_static` with interior mutability, or thread-local holding VM objects | The `layering` CI job's grep, already in the tree |

## Tasks

| | Task | Check |
|---|---|---|
| T1 | `Header`, `Payload`, flags | `the_header_is_two_words` |
| T2 | Size classes and 64 KiB blocks | `size_classes_cover_their_range_exactly` |
| T3 | Large-object space | `large_objects_allocate_and_are_reclaimed` |
| T4 | `HandleScope`, `Handle`, nesting | `a_nested_scope_pops_its_own_handles_only` |
| T5 | Mark with an explicit worklist | `a_deep_chain_does_not_overflow_the_mark_stack` |
| T6 | Sweep to intrusive free lists | `swept_cells_are_reused` |
| T7 | Automatic trigger and growth | `allocation_alone_triggers_collection` |
| T8 | `Drop` for blocks and large objects | Miri |
| T9 | The stress test | `ten_million_objects_with_a_forced_gc_every_thousand` |
| T10 | Miri in CI, the follow-up PRD 0006 left | `.github/workflows/ci.yml` |
| T11 | engine.md carries the real numbers | The tables above match the code |

`cargo test -p spinel-vm`: 34 passing, 19 of them new — 15 unit tests on the heap and 4
doctests, three of which are `compile_fail` and are the only way to check a claim about
what *cannot* be written. `cargo miri test -p spinel-vm` is green.

## Numbers

10,000,005 objects and 10,000 collections, on an M-series laptop:

| build | wall | objects/s |
|---|---|---|
| release | 88 ms | 114,000,000 |
| debug | 983 ms | 10,200,000 |

Measured with a temporary probe, then removed: a timing assertion is a test that fails
when the machine is busy. What is left behind is `Stats::total_allocated`, which is why
the count in the table is evidence and not a restatement of the loop bounds.

## What the audit caught

- **Safe code could store a dangling pointer.** R4 claims an unrooted object is
  unrepresentable. It was not: `set_slot`, `set`, and `root` take a bare `Value`, so a
  `Value` read out of a nested scope, kept across a collection, and stored afterwards
  reaches freed memory from safe Rust. Fixed with an `ALLOCATED` flag bit — set at
  allocation, cleared when the cell is relinked — and a `debug_assert` on the storing
  paths. One masked load in debug and under Miri, nothing in release, and
  `storing_a_value_that_was_collected_is_caught` is the test that it fires. The bit is
  never read by the collector; the discipline that *prevents* the mistake is still the
  borrow checker.
- **PRD 0006 asked for the wrong Miri flag.** Its follow-up named
  `-Zmiri-strict-provenance`. That mode bans integer-to-pointer round trips, and a
  tagged `Value` *is* an integer-to-pointer round trip — every heap test aborts on the
  first `as_heap`. The flag and the type are incompatible by construction, so CI runs
  Miri's default model, which tracks exposed provenance and is the one `Value` is
  written against. Under it the whole crate is clean.
- **The leak check started red.** `cargo miri test` reported two leaks before this slice
  added a line: `value.rs`'s `one_of_each` helper used `Box::leak` to get a stable
  aligned pointer. A leak check nobody can keep green is a leak check nobody reads, so
  the helper now borrows the caller's `u64`. That is the whole reason R7 is checkable.
- **The stress test proved less than its name.** It allocated in a loop whose results
  were unused and asserted only on the live set, so an elided loop would have passed
  just as well. `Stats::total_allocated` now makes the ten million an assertion.
- **A deep chain was the missing test, not a hypothetical.** The mark phase uses a
  worklist because a recursive one overflows, but nothing checked it. A million-link
  chain does, and it is 250 links under Miri, where the arithmetic is the point and the
  depth is not.
- **`Heap: !Send` was a claim in a doc comment.** Now two `compile_fail` doctests. The
  same tool checks R4's escape rule, which is the only honest way to test that something
  does not compile.

## Open decisions for the owner

1. **Five size classes, powers of two.** CRuby uses multiples of 40; powers of two make
   the class index one `leading_zeros` and waste up to half a cell on the object that
   just crossed a boundary. Phase 3 has the allocation profiles that would settle it.
2. **Empty blocks are never returned to the allocator.** After a spike the heap stays
   at its high-water mark. Giving a block back needs a per-block live count, which is
   cheap to add and pointless to tune before there is a program to measure.
3. **A `Handle` is an index, not a branded one.** Two heaps in one Rust function could
   pass a handle from one to the other; the index is bounds-checked, so the failure is a
   panic or a wrong object, never memory unsafety. Making it impossible costs a
   `GhostCell`-style brand on every signature, for a mistake that needs two Ractors in
   one function.

## Follow-ups

- `HandleScope::escape`, to return a freshly allocated object from a nested scope to its
  parent. Deliberately not added: nothing in the tree returns one yet, and the shape it
  should take depends on whether the first caller is a primitive or the interpreter.
- Miri under `-Zmiri-tree-borrows` as well as the default Stacked Borrows. One aliasing
  model is what CI can afford per push; the second is worth a nightly job when the
  interpreter is holding raw pointers into frames.
- `frozen` and `ractor-shareable` flag bits, and the shape id in the reserved two bytes.
  All three belong to [#8](https://github.com/ar4mirez/spinel/issues/8), which is the
  slice that can set them.
