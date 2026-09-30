# PRD 0043 — `Hash` keyed by `hash`, and a `should ==` that dispatches

Issue: [#22](https://github.com/ar4mirez/spinel/issues/22) · Phase 2 ·
`area:core-lib`

## Objective

Close #22. PRD 0040 wrote `Hash`'s method surface and left the third box of
the definition of done unticked — "custom `hash`/`eql?` on keys is honored" —
because the table matched keys by `eql?` alone and `Hash#hash` hung on a
self-referential hash. `compare_by_identity` (35 examples) was the largest
Hash-owned row.

## Baseline

`core/hash` 153 / 460 (33%). Corpus 3094, 0 failed. The top row was not a
Hash method at all: 41 examples blocked on "`==` on an object whose class has
no methods yet".

## The largest row was the harness

`spec/harness` checked `x.should == y` with `interp::ruby_eq`, a Rust
comparison that knows Strings, Arrays and numbers and refuses the rest. A Hash
is Ruby, so every `h.should == {...}` was blocked — although the VM itself
answers `h == {...}` correctly by dispatching to `Hash#==` when that same fast
path refuses. mspec's `should ==` *is* that call.

The harness now dispatches: it parks the two computed values in two hidden
locals of the example's frame (a new `Frame::set_local`) and evaluates a
compiled `%actual == %wanted`. Evaluating the original expressions again would
have repeated their side effects. The two locals are declared before the
example runs, because growing the frame mid-example gives it a new environment
and a block made earlier would keep reading the old one — which
`enumerator/size_spec.rb` caught the first time.

That moved 101 examples across the corpus and surfaced eleven disagreements
the blocked comparison had been hiding. Ten are fixed here:

- **`Hash#slice` stored the pair's index, not its value** — `{a: 1}.slice(:a)`
  was `{a: 0}`. Every slice example had been blocked on the comparison.
- **`Array#==` did not exist**, so an Array holding a Hash compared by
  identity. Written, with the `to_ary` fallback and the recursion rule.
- **`\G` anchored to each match attempt** instead of the search's start, so
  `'hello'.match(/\Go/)` matched. `spinel-regex` now passes the start through.
- **`String#match`/`Regexp#match` ignored their block.** The native pushes the
  block's frame itself, so `$~` stays in the caller's frame.
- **`Enumerable#tally(hash)` counted from the hash's default.**
- **`Exception#exception(self)` built a new exception** instead of answering
  self.
- **`Enumerator::Lazy#size` answered a Proc size uncalled.**
- **A splatted Hash argument was passed as the Hash** — `f(*{a: 1})` passes
  `[:a, 1]`. The binder now splices a core Hash's pairs, and refuses (blocked)
  any other object whose `to_a` is Ruby rather than passing it through.
- **`Enumerator.produce` and `.product` accepted unknown keywords** once
  keywords could arrive as a Hash (below); both name them now, and `produce`
  takes 4.0's `size:`.

The eleventh — a `to_ary` only `method_missing` answers — is #28's and is
tagged.

## The table, keyed by `hash`

`__index__` now asks what CRuby asks: the same hash code *and* `key.eql?(stored)`
(the lookup key's `eql?`, as CRuby calls it), or the very same object. Each key's
code is taken when it is stored, in `@hashes` beside `@pairs` — so a key
mutated afterwards is not found until `rehash`, which is Ruby's behaviour and
what `rehash_spec.rb` checks. `@hashes` is dropped by anything that rebuilds
`@pairs` and recomputed on demand.

`compare_by_identity` asks identity only and never calls `hash`; it survives
`dup`, `clone`, `select`, `reject`, `slice`, `except`, `compact` and
`transform_values`, and is dropped by `transform_keys` and `invert` — each
measured. `replace` takes the argument's. A non-frozen String key is stored as
a frozen copy, except under `compare_by_identity`.

## `hash` for structures that contain themselves

`Array#hash` and `Hash#hash` are Ruby folds over their elements' own `hash`
(through `to_int`, as CRuby converts), mixed by one new primitive,
`__hash_combine__`. `Hash#hash` sums its pair digests, so it is independent of
insertion order.

Recursion follows CRuby's `rb_exec_recursive_outer`: a shared stack of objects
being hashed, and when one meets itself anywhere below, a `throw` reaches the
**outermost** call, which answers a value depending only on its kind and size.
That is why `rec = []; rec << rec` hashes like `[rec]` and `[[rec]]`, and
`h = {}; h[:x] = h` like `{x: h}` — `eql?` says they are equal, so their hashes
must be. `Array#==`, `Array#eql?` and `Hash#==` get the paired form of the same
guard: a pair already being compared is taken as equal.

Two more found on the way: a bignum hashed by identity (`(2**70).hash` differed
between two equal bignums), and `Array#eql?` compared elements with `==`, so
`[1].eql?([1.0])` was true.

## Keywords to a method that declares none

`def m(h) = h; m(a: 1)` refused, because Ruby packs such keywords into a
trailing positional Hash and the binder could not build one. It builds one now
(`hash_of_pairs`, the three ivars `Hash.allocate` writes), before the arity
check, so the Hash counts as an argument. 15 examples in `core/hash`, more
across the corpus.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/hash` | 153 / 460 (33%) | **265 / 460 (58%)** |
| `language` | 1205 / 2464 | **1282 / 2464** |
| `core/enumerable` | 289 / 446 | **304 / 446** |
| `core/array`, `core/enumerator`, `core/exception`, `core/range` | | **+6, +6, +6, +5** |
| **corpus** | 3094 | **3330** |

All 3330 passes re-run on ruby 4.0.7 and agree.

0 failed.

## What blocks the rest of `core/hash`

Nothing Hash-owned is large. The rows are the harness's other matcher shapes
and `mock` (#145), `&` on a non-Proc block argument (#239), `instance_method`
(#28), `ruby2_keywords_hash` (#200), and `Proc.allocate`. `Hash#inspect`'s 3.4
`{a: 1}` spelling is #216.
