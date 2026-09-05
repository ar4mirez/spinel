# PRD 0009 — Per-class serials, and the method cache they invalidate

Issue: [#9](https://github.com/ar4mirez/spinel/issues/9) · Phase 1 · `area:engine`

## Objective

Make method-cache invalidation *precise*: a definition on one class must not evict the
cached lookups of classes that cannot see it. [#8](https://github.com/ar4mirez/spinel/issues/8)
landed the cache and one serial for the whole table, which is correct and coarse — it is
free at load time and wrong in a program that defines methods while running. This slice
replaces that with one serial per class, adds the two invalidation triggers the issue's
definition of done names but that nothing could reach yet (`extend`, singleton-class
mutation), and puts a number on both sides of the trade in `bench/`.

## Baseline

Measured on `main` at b33468b, before any change here.

| | |
|---|---|
| Rust tests | 221 passing |
| ruby/spec | 1243 passed · 0 failed · 22520 blocked · 3.8s |
| Boot (`bootstrap` + `core/*.rb`) | 56.4µs per heap |
| `Classes::serial()` | one `u64` for the whole table |
| `Classes::cache` | one `HashMap<(ClassId, SymbolId), _>`, emptied whole on any change |
| `Object#extend` | did not exist |

`Classes::serial()` had no caller outside its own tests, which is why its shape could
change without a migration.

## What Ruby actually does

Measured against `ruby 4.0.6 (2026-07-14) +PRISM`, not read from documentation.

- `obj.extend(M)` answers `obj`, and puts `M` in the singleton's chain.
- `obj.extend(A, B)` orders them `[singleton, A, B]` — `A` nearer, the same right-to-left
  splice `include A, B` performs.
- `obj.extend(String)` raises `TypeError: wrong argument type Class (expected Module)`. A
  Class *is* a Module in the hierarchy, and is still refused here.
- `obj.extend()` raises `ArgumentError: wrong number of arguments (given 0, expected 1+)`.
- `obj.respond_to?(:m)` is true after `obj.extend(M)` and after `def obj.m`. This is where
  the audit found a bug; see below.

## Decisions

### The serial and the cache both move into the class entry

The alternative was to keep one `HashMap<(ClassId, SymbolId), _>` and evict selectively
from it, which cannot be done without scanning: a `HashMap` cannot answer "every key whose
first component is this class". Moving the cache into `Entry` makes invalidating a class
*be* clearing its own map. The serial follows it for the same reason — they are always
read and written together.

### Invalidation is an eager walk down, not a lazy check up

A cached answer at `D` names a method found by walking `D`'s chain, so only classes with
`C` in their chain can hold an answer that a change at `C` invalidates. That set is
reachable from `C` by two edges: `subclasses` (new here) and the flat `includers` list
that already existed for [Feature #9573](https://bugs.ruby-lang.org/issues/9573).

The lazy alternative — leave caches alone, and check on read whether any ancestor has
changed since the entry was made — costs a chain walk per *lookup*, which is the thing the
cache exists to avoid. Definitions are rarer than lookups once a program is loaded, so the
work belongs on the definition. The benchmark below is what says "rarer": a lookup is 8ns
and a worst-case definition is 499ns, and lookups outnumber definitions by orders of
magnitude after load.

### The walk carries a stamp, not a `HashSet`

The first version allocated a `Vec` and a `HashSet` per invalidation. That was not a
detail: it cost 2.0µs per definition on `Object` and made `core/*.rb` load 2.6x slower,
which the full spec run showed as 3.8s → 6.1s. A `u64` stamp on each entry plus one
frontier `Vec` reused across walks brought the same walk to 499ns. Dedup is still needed —
a diamond through two modules reaches the same entry twice — the stamp is just a cheaper
way to spell it.

### `Object#extend` is a primitive, not `singleton_class.include`

`Module#include` is private in Ruby, and the singleton class of an ordinary object is
allocated by the class table rather than by anything `core/*.rb` can reach. Writing it in
Ruby would mean exposing both, which is a larger surface than the primitive.

### `respond_to?` moves from `core/kernel.rb` to a primitive

`core/kernel.rb` defined it as `self.class.method_defined?(name)`. `Object#class` skips
the singleton — correctly, that is its job — so that spelling could never see a
`def obj.foo` or an `extend`ed module. It answered `false` for a method the very next call
would dispatch to.

This is a pre-existing bug, reproduced on `main` before this slice touched anything, and
it is fixed here rather than filed because shipping `extend` beside a `respond_to?` that
cannot see it is shipping a known lie. The fix asks the question dispatch asks, from the
class dispatch starts at.

### `extend` reuses `singleton_of`, which the audit then had to fix twice

The first version rooted the receiver and called `singleton_class_of` directly, which
**panicked** on `1.extend(M)` — `a handle from alloc is a heap object`. A VM must not panic
on Ruby input. `def obj.foo` and `class << obj` already went through `singleton_of`, which
refuses an immediate with Ruby's own `TypeError: can't define singleton`, so `extend` now
goes through it too and the second implementation of that rule does not exist.

`singleton_of` was then wrong in the other direction. Its comment said `nil`, `true` and
`false` "answer `NilClass` and friends, which this VM will have once `core/*.rb` does
(#15)" — and #15 landed in cf21fb6. Measured against CRuby, those three are immediates
that *do* have singleton classes: the singleton **is** `NilClass`, so a definition goes
straight into it. `1`, `:s` and `1.5` remain refused. Three `language/metaclass_spec.rb`
examples were waiting on exactly this.

### The cache stays unbounded

PRD 0008's second open decision, revisited and unchanged. It is now bounded per class
rather than per heap — each class's map holds at most the distinct names called on it
since its last invalidation. Still a program-shaped bound rather than a number, and a cap
still costs an eviction policy that nothing yet measures as necessary.

### `define_class` no longer invalidates

It used to bump the table-wide serial, because there was only one. A brand-new class is in
nobody's chain, so no cached answer anywhere can be wrong because it exists. It now only
registers itself with its superclass.

## Non-goals

- **Inline caches at call sites.** That is [#169](https://github.com/ar4mirez/spinel/issues/169),
  filed by this slice — `docs/engine.md` described the side table and the code comments
  pointed at #10, which is the closed *bytecode* slice. #10 did create `Iseq::call_sites`;
  it did not create the per-heap table that memoises a target against a serial, and nothing
  tracked that. `Classes::serial(id)` now exists to be guarded against. Nothing reads it yet.
- **Method visibility.** `respond_to?` inherits `method_defined?`'s missing `private`
  handling ([#161](https://github.com/ar4mirez/spinel/issues/161)) and ignores its second
  argument, because there is nothing yet for it to select.
- **`Module#remove_method` in Ruby.** Removal invalidation is covered by a Rust test; the
  Ruby-visible method is not in this slice's definition of done.

## What ships

### `crates/spinel-vm/src/class.rs`

`Entry` gains `serial`, `cache`, `subclasses`, and a `visited` stamp; `Classes` loses its
table-wide `serial` and `cache` and gains a reusable walk frontier. `bump()` becomes
`invalidate(id)`. `serial()` becomes `serial(id)`.

### `crates/spinel-vm/src/interp.rs`, `crates/spinel-vm/src/method.rs`

`Native::Extend` and `Native::RespondTo`, registered on `Kernel`. `singleton_of` learns
that `nil`, `true` and `false` have singleton classes.

### `core/kernel.rb`

`respond_to?` deleted, replaced by the primitive.

### `bench/method_cache.rs` (new)

The first benchmark in the repo, wired as a `[[bench]]` on `spinel-vm` with `harness =
false` — `std::time::Instant` and a min-of-five, no new dependency.

### `docs/engine.md`, `docs/prd/0008-*.md`

The cache section described the design this slice replaces; 0008's first two open
decisions are settled.

## Checks, and what they measured

### ruby/spec

1243 → **1248 passed, 0 failed**, nothing lost. The delta, by name:

```
core/kernel/extend_spec.rb    Kernel#extend raises an ArgumentError when no arguments given
core/kernel/extend_spec.rb    Kernel#extend updated class methods of a module when it extends
                              self and includes another module
language/metaclass_spec.rb    self in a metaclass body (class << obj) is NilClass for nil
language/metaclass_spec.rb    self in a metaclass body (class << obj) is TrueClass for true
language/metaclass_spec.rb    self in a metaclass body (class << obj) is FalseClass for false
```

The second is itself an invalidation test, which is a better witness than a count. The
three `metaclass_spec` rows come from the `nil`/`true`/`false` fix below.

### Rust tests

221 → **224 passing**, 0 failing. Three new tests in `class.rs` — the precision claim, the
descendant walk over both edges, and singleton-class mutation — plus the existing trigger
test rewritten per class and widened to cover redefinition. Eight new oracle rows in
`tests/eval.txt`, each asking the same question twice with a definition in between, so a
cache that outlived the definition would repeat the first answer.

### Differential against CRuby

`extend` and `respond_to?` were diffed expression-by-expression against `ruby 4.0.6` over
15 cases — ordering, `extend self`, immediates, string names, both error types — and agree
byte for byte. Pass counts hide a method that raises; a diff does not.

### The benchmark

```
dispatch                                   cached   uncached   speedup
  hit on the class itself        4 deep     9.0ns     10.0ns     1.1x
  hit on the class itself       27 deep     8.0ns     10.0ns     1.2x
  hit on Object, 1 away          4 deep     8.0ns     17.0ns     2.1x
  hit on Object, 24 away        27 deep     8.0ns     69.0ns     8.6x
  hit on Object, 8 modules      12 deep     8.0ns     21.0ns     2.6x
  miss (method_missing)         27 deep     8.0ns     90.0ns    11.2x

dispatch while an unrelated class is being defined into
  undisturbed lookup              8.0ns
  definition alone               14.0ns
  definition + lookup            21.0ns   (lookup share 7.0ns)
  ... one serial per table       80.0ns   (lookup share 66.0ns, 3.8x worse)

invalidation, 90 classes in the heap
  define on Object    499.0ns   (every class downstream)
  define on a leaf     14.0ns   (nothing downstream)

boot (bootstrap + core/*.rb)
  per heap             70.6µs
```

Three things worth reading off it.

**The cache is flat and the walk is not.** A cached lookup is 8ns whatever the chain looks
like; the walk it replaces runs 10ns to 90ns. So the cache is worth nothing on a method
defined on the receiver's own class and 11.2x on a miss — and a miss is the
`method_missing` and `respond_to?` path, which is the one that walks furthest.

**Precision is worth 3.8x on the workload it was built for.** With definitions landing on
an unrelated class, the lookup keeps its cached answer and costs 7ns. One serial per table
evicted it every time, making the same lookup 66ns. That line is the justification for the
slice, and it is generous to the old design: it does not charge it for the re-insert.

**It is paid for at load time.** Boot went 56.4µs → 70.6µs per heap, ~25%. The spec
harness builds one heap per example, so the full run went 3.8s → 4.3s. That is the trade
PRD 0008 predicted, now with a number on it, and it is the right side of the trade for a
VM that will run programs longer than it loads them.

## Open decisions for the owner

1. **Boot is 25% slower, and the spec harness pays it 25,624 times.** The walk is already
   6x cheaper than the first version. Getting it lower means a compact parallel array for
   the serial and cache so the walk stops touching 90 scattered `Entry` structs, which is
   real work for a load-time-only win. Worth doing when boot shows up in a profile that is
   not the spec harness.
2. **Nothing reads `serial(id)` yet.** It exists for [#169](https://github.com/ar4mirez/spinel/issues/169),
   and until that lands, the per-class serial is exercised only by tests — the *cache*
   precision is what the benchmark measures. If #169 changes shape, the serial should be
   re-litigated with it rather than treated as settled.
3. **An `Error::NoDispatch` for an undefined method is not rescuable.** `begin; obj.nope;
   rescue NoMethodError; end` does not catch it, where CRuby does. Pre-existing on `main`,
   unrelated to this slice, filed as [#170](https://github.com/ar4mirez/spinel/issues/170)
   — it is why the invalidation cases in `eval.txt` are written with `respond_to?` rather
   than a rescue. It is also the corpus's top blocker: the two largest blocking reasons are
   both a `NoMethodError` raised out of a spec helper, 1,606 examples between them.
