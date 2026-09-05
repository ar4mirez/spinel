# PRD 0018 — Monomorphic inline caches at call sites

Issue: [#169](https://github.com/ar4mirez/spinel/issues/169) · Phase 1 · `area:engine`

## Objective

Put a per-call-site memo in front of `Classes::lookup`, so a send that keeps hitting the
same receiver class stops paying a hash probe per call.

`docs/engine.md` has described this since the engine document was written: "Call sites
carry a monomorphic inline cache `(class serial, target)`. Because bytecode is shared
across Ractors, inline caches do not live in the bytecode; each heap owns a side table
indexed by call-site id." Both halves that depend on already exist and nothing consumes
them — `Iseq::call_sites` landed in [#10](https://github.com/ar4mirez/spinel/issues/10)
and `Classes::serial(id)` landed in [#9](https://github.com/ar4mirez/spinel/issues/9),
where its doc comment already points forward to this issue. This slice writes the table
and the guard, and is what [#123](https://github.com/ar4mirez/spinel/issues/123) (the JIT
feeding on interpreter caches, Phase 6) needs to exist first.

## Baseline

Measured on `feat/inline-caches-169` at cd2d321, before any change here.

| | |
|---|---|
| Rust tests | 224 passing |
| ruby/spec | 1248 passed · 0 failed · 22515 blocked · 4.4s |
| Cached `Classes::lookup` | 8.0ns, flat across chain depth — one hash probe |
| Uncached walk | 10.0ns (own class) to 93.0ns (miss, 27 deep) |
| Boot (`bootstrap` + `core/*.rb`) | 73.5µs per heap |
| Inline caches | did not exist; `Iseq::call_sites` had no consumer but the interpreter's operand decode |

The 8.0ns is the number to beat. It is flat because the per-class cache turns every depth
into the same probe, which is exactly why an inline cache has to be cheaper than a probe
rather than cheaper than a walk to be worth its guard.

## Decisions

### A call-site id is a per-`Iseq` base plus the instruction's index

`Insn::Send(index)` indexes `Iseq::call_sites`, which is per `Iseq`. The side table is per
heap. Something has to turn the pair into one number.

Giving each `Iseq` a contiguous run of the table and adding the operand to the run's base
does it, and keeps the hot path a `Vec` index. The alternative — keying the table by
`(iseq pointer, index)` — is a hash probe, which is the cost being removed.

### The base is resolved once per frame, not once per send

`Call` already interns the whole symbol pool once per frame push rather than per
instruction (`symbols: iseq.link()`). The cache base is the same shape of fact about the
`Iseq` being entered, so it rides along in the same place: `push_frame` resolves it, and
`Insn::Send` does `frame.cache_base + index`.

That puts the one hash probe this design needs on frame entry, where `link()` already pays
a probe per symbol, and takes it off the send.

### An `Arc` address is the memo key, and the table keeps the `Arc` alive

`Definitions::intern_iseq` already established both halves of this: an address is only a
safe key because the entry it points at holds a clone of the same `Arc`, so the `Iseq`
cannot be dropped while the table remembers it and its address cannot be reused by a
different one. A memo that stored a base without keeping the `Arc` would eventually hand a
new `Iseq` another one's cache entries — which is a wrong-method bug, not a stale one.

### The guard is receiver class plus that class's serial

An entry is `(ClassId, u64, Method)` and is used only when both the class and the serial
match. Two comparisons against a `Copy` struct already in cache.

Class equality covers the receiver changing shape, including the case that looks like it
needs its own trigger: `def obj.foo` allocates a singleton and *replaces* the object's
class header with it, so the next send through that site sees a different `ClassId`.

Serial equality covers everything #9 defined a serial to cover — a definition on the class
or any ancestor, a mixin that moved the chain, `remove_method`, `extend`. This slice adds
no invalidation triggers, because the whole point of the per-class serial was that the set
it already covers is exactly the set that can change what a name resolves to for a class.

### Entries are never invalidated eagerly, unlike the per-class cache

`docs/engine.md` says the per-class method cache is emptied rather than stamped and left,
because a stale entry can name a body `remove_method` just dropped and the cache is the
only thing keeping it from the collector — a stamped cache is a GC bug, not just a stale
answer.

That reason does not reach here. A `Method`'s three fields are `owner: ClassId`,
`cref: CrefId`, and `body: Value` — two indices into per-heap arenas that only ever grow,
and a fixnum definition id, which is the whole reason #8 made a method body a fixnum: the
collector never traces one. A stale inline-cache entry roots nothing. So the guard is
allowed to be lazy, and a class that is never sent to again keeps a dead entry until the
heap goes, at no cost but the eight bytes.

### Misses are not cached inline

`Classes::lookup` caches a `None` so a name nothing defines costs one chain walk rather
than one per call, and that stays. The inline cache stores only hits: a miss falls through
to `lookup`, which answers from *its* memo, and then goes on to raise or to
`method_missing`. Caching the miss inline would widen the entry to distinguish "no entry"
from "entry saying nothing" to save a probe on a path that is about to build an exception.

## Plan

1. `crates/spinel-vm/src/callcache.rs` — `CallCaches` with `base`, `get`, `fill`, and a
   `filled()` count for tests, mirroring `Classes::cached_lookups()`.
2. `Heap` owns one; `HandleScope` forwards to it, like `definitions`.
3. `Call` carries `cache_base`; `push_frame` and `eval_in` fill it.
4. `Pending` carries `cache: Option<u32>`; only `Insn::Send` sets it. `Yield` targets a
   block and resolves no name; `Native::Send` re-dispatches under a name the call site
   never mentioned, so neither may borrow the site's entry.
5. `dispatch`'s `Target::Method` arm consults, falls back, and re-fills.
6. Rust unit tests: hit, class change, serial change, and one end-to-end that a `Send`
   fills the table.
7. `bench/method_cache.rs` grows a section putting the guard beside the probe, and an
   end-to-end Ruby send loop.

## Results

| | before | after |
|---|---|---|
| Rust tests | 224 | 236 |
| ruby/spec | 1248 passed · 0 failed | 1248 passed · 0 failed |
| `verify-passes.rb` | 670 agree | 670 agree |
| Cached `Classes::lookup` (hash probe) | 8.0ns | 8.0ns |
| Inline cache hit, serial read included | — | **1.0ns** (8x) |
| 300k sends through one site, `spinel run` | 150ms | **137ms** (1.10x) |
| Boot, 100 heaps | 418ms | 416ms |

No spec moved, which is the point: an inline cache that changes an answer is a bug, not a
slice. The 1248 are the same 1248, and `verify-passes.rb` re-ran all 670 `language/`
passes against `ruby 4.0.6` and found no disagreement.

The end-to-end 10% is what an 8x on the lookup is worth once frame push, environment
allocation and argument binding are in the way — those are the next thing in the profile,
not dispatch. `docs/engine.md` already names the environment-per-frame allocation as a
`ponytail:` with a delta benchmark owed.

Boot did not move, which was the risk worth checking: the spec harness boots one heap per
example 25,624 times, and this slice adds a hash probe per frame push. It lands where
`Iseq::link` already pays one probe per symbol, and does not register.

### Verified by mutation, not just by green

Each half of the guard was deliberately broken and the suite re-run, because a test that
passes against a cache nothing consults is worth nothing:

| mutation | caught by |
|---|---|
| drop `serial` from the guard | `a_redefinition_between_two_calls_through_one_site_is_seen`, `a_definition_on_a_superclass_between_two_calls_is_seen` |
| drop `class` from the guard | `one_site_with_two_receiver_classes_answers_both` |
| drop the per-`Iseq` base memo | `a_repeated_call_reuses_one_entry` |

### Audit: what can still change a resolved method, and does it bump a serial

Every mutator in `class.rs` that can move a method was walked. `define_method_in` and
`remove_method` invalidate; the mixin path invalidates from `target`, and `invalidate`'s
two edges reach the `includers` it also spliced. Nothing else writes a method table.
`ClassId` and `CrefId` index arenas that only grow, so neither can be recycled under a
stale entry. `def obj.m` needs no trigger of its own — a singleton *replaces* the object's
class, so the class half of the guard sees it.

### Left for later

- `Insn::BinOp`'s slow path has no call-site index, so an operator that misses the fixnum
  fast path still pays the full probe. Marked `ponytail:` at the site. Worth a call site
  when a benchmark shows operator dispatch on user-defined classes mattering.
- Sites stay monomorphic. A polymorphic site pays the guard *and* the probe. Ruby code has
  plenty of these; the shape of the fix is a second entry, and the evidence for it is a
  benchmark that does not exist yet.
- Misses are not memoised inline, so a `method_missing`-heavy program is unchanged.
- An `Iseq` this heap has entered is kept alive for the heap's lifetime, to keep the
  address key sound. `Definitions` already holds every method and block body for the same
  span; the reference this adds is the top-level script's.
