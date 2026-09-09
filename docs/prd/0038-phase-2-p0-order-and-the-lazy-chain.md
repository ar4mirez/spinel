# PRD 0038 — the Phase 2 P0 order, and the lazy chain

Issues: [#26](https://github.com/ar4mirez/spinel/issues/26) · Phase 2 ·
`area:core-lib`. Orders [#16](https://github.com/ar4mirez/spinel/issues/16),
[#17](https://github.com/ar4mirez/spinel/issues/17),
[#19](https://github.com/ar4mirez/spinel/issues/19),
[#21](https://github.com/ar4mirez/spinel/issues/21),
[#22](https://github.com/ar4mirez/spinel/issues/22),
[#28](https://github.com/ar4mirez/spinel/issues/28),
[#29](https://github.com/ar4mirez/spinel/issues/29),
[#145](https://github.com/ar4mirez/spinel/issues/145).

## Objective

Two things. Settle the order the nine Phase 2 `P0` issues are worked in, by
measuring who owns what rather than by reading the issue titles — and then run
the first slice that order picks out.

The milestone target is "`core/` passes above 80%, `mspec` itself runs on Spinel
and replaces `spec/harness/`". `core/` is at 11%, so the order matters more than
the individual slice does.

## Baseline

Measured at 453e9ec on this branch. Corpus counts pinned `--platform=linux`,
which is what `scripts/spec-status.sh` pins, because guards resolve against the
host and an unpinned number is not comparable.

| | |
|---|---|
| ruby/spec | 3835 files · 25624 examples · 2806 passed · 0 failed · 20896 blocked |
| `core/enumerator` | 390 examples · 29 passed · 0 failed · 360 blocked · 7% |
| Rust tests | green in debug |
| `bench/spec-status.md` | current, regenerates clean |

## The order, measured

`scripts/spec.sh --platform=linux --blocked=0 spec/ruby/core` ranks every
blocking reason under `core/`. Grouping the top of that ranking by which issue
owns the row is what settles the order — and it does not agree with the issue
list read top to bottom.

| examples | owner | reachable today? |
|---:|---|---|
| ~2,300 | **#145** — `tmp` 695, `mock` 577, `fixture` 227, `io_fixture` 205, `bignum_value` 204, `be_computed_by` 80, `ruby_exe` 65, `mock_numeric` 47, the `should` shapes | no — needs `File`, `Dir`, `eval`, fibers |
| ~1,300 | Phase 3 — `IO` 223, `File` 213, `ENV` 194, `Thread` 175, `Process` 90 | no — not this milestone |
| ~1,100 | **#19** — `Encoding` 580, `unpack` 78, `to_i` 65, `encode` 59, `force_encoding` 58, `[]=` 45 | yes, with new primitives |
| ~500 | **#28** — `refine` 82, `public_methods` 56, `define_method` 51, `method` 49, `private_instance_methods` 47 | **partly — the hooks are blocked on #16** |
| ~350 | **#21** — `pack` 146, `MyArray.[]` 52, `Array#[]=` splice 47 | yes |
| ~60 | **#29** — `Errno` 32, exception `initialize` 27, `full_message` 12 | yes, but see the note below |
| ~250 | **#22** — `compare_by_identity` 35, `merge`, `flatten`, `dig` | yes |
| **213** | **#26** — `lazy` 133, `Product` 37, `Lazy` 19, `product` 10, `Chain` 8, `produce` 6 | **yes, and with no new machinery at all** |
| ~93 | **#16** — `Fiber` | yes, large |

Three conclusions the ranking forces, none of them visible from the issue list:

**#145 is the largest owner in `core/` and cannot be worked yet.** It owns more
blocked examples than #19, #21, #22 and #28 combined. PRD 0036 and PRD 0037 each
reached this conclusion for one directory; the whole-`core/` ranking is what
shows it is the general case. mspec is not vendored in this repo and
`spec/ruby/spec_helper.rb` reaches for it through `$LOAD_PATH` and `require` —
so #145 is gated on `File`, `Dir`, `require` (#39) and fibers (#16), three of
which are Phase 3. It stays last, and the milestone's second clause with it.

**#28 is gated on #16, and the code says so out loud.** `hook_refusal` in
`interp.rs` refuses `inherited`, `method_added`, `included`, `prepended` and
`const_added` with the reason "firing one needs a primitive that pushes the
hook's frame and then carries on with the definition, which a native cannot do".
That primitive is what #16's re-entrant `corosensei` half is for. `define_method`
has the same shape — the binder calling Ruby. So #28 splits: the reflection that
only reads tables (`instance_methods`, `const_get`, `remove_const`,
`private_instance_methods`) is reachable now; the hooks and `define_method` are
#16's dependents and should not be attempted before it.

**#26's remainder needs nothing that does not already exist.** The issue says
"on top of fibers", and its `next`/`peek` half genuinely is — that is the 26
examples already refusing by name. The lazy chain is not: it is deferred blocks
composed over `each`, which is Ruby over machinery `core/enumerator.rb` already
has. At 213 examples for no new primitive it is the best value in the table, and
it is the slice this PRD runs.

**#29 is smaller than its `Errno` row suggests, and the row is why.** `Errno`
blocks 210 examples under `core/`, but only **32 of them are in
`core/exception`** — 167 are in `core/dir`, where defining the `Errno` classes
is necessary and nowhere near sufficient, because the `Dir` operation that
would raise one does not exist either. Counting a reason corpus-wide and
crediting it to the issue whose class it names is the mistake PRD 0033 was
written about; this table is grouped by *directory* for that reason, and #29's
row was corrected after the fact when a per-directory check caught it.

The resulting order: **#26 (this PR) → #29 → #22 → #21 → #16 → #28 → #19 → #145**,
with #19 movable earlier if `Encoding` is split out of it — 580 of its 1,100 are
that one constant. #29 keeps its position on cheapness rather than size: at ~60
reachable examples it is small, but it needs no engine work and no harness.

## What this slice writes

All Ruby, in `core/enumerator.rb` and `core/enumerable.rb`. No Rust.

`Enumerator::Lazy` with `map`/`collect`, `select`/`filter`, `reject`,
`take`, `take_while`, `drop`, `drop_while`, `flat_map`/`collect_concat`,
`filter_map`, `grep`, `grep_v`, `uniq`, `compact`, `zip`, `with_index`,
`each_with_index`, `force`, `eager`, `lazy`, `first` and `to_enum`;
`Enumerator::Chain` with `Enumerator#+` and `Enumerable#chain`;
`Enumerator::Product` with `Enumerator.product`; and `Enumerator.produce`.

Deliberately not written:

- **`Enumerator.produce(x) { }.size`** is `Float::INFINITY`, and this VM has only
  flonums — an infinity needs a heap `Float` box. #18. It reports the missing
  constant by name rather than answering `nil`, which would be a wrong answer.
- **`next`, `peek`, `rewind`, `feed`** stay refused by name. #16.
- **`Lazy#slice_before`/`slice_after`/`slice_when`/`chunk_while`** return an
  eager `Enumerator` in CRuby, so `Enumerable`'s already-correct versions are
  inherited rather than overridden.

## What had to be measured rather than reasoned about

`scripts/lazy-oracle.rb` is the measurement, and it prints three tables.

- **`Lazy` does not inherit `Enumerable`'s yield split — two methods disagree
  with their eager twin.** `drop_while` packs eagerly and passes through lazily;
  `uniq` passes through eagerly and packs lazily. Both directions, no rule.
  Every spec whose fixture yields one value per iteration passes either way, so
  only `EnumerableSpecs::YieldsMulti` separates them.
- **`size` survives some links in the chain and not others.** `map`,
  `with_index` and `zip` keep it, `take`/`drop` arithmetic on it —
  `(1..10).lazy.take(20).size` is 10, not 20 — and `select`, `reject`,
  `flat_map`, `uniq`, `compact`, `take_while`, `filter_map` and `grep` all
  answer `nil`, because none of them knows its own length without running.
- **`take(0)` must not touch the source at all.** Not an optimisation: an
  infinite generator makes it observable, and `force` on it answers `[]` with
  the producing block never entered.
- **`Enumerator.product()` with no arguments is `[[]]`, size 1** — one empty
  tuple, not zero tuples.

## Check

`scripts/spec.sh --platform=linux spec/ruby/core/enumerator` moves off 29/390
with 0 failed, `bench/spec-status.md` is regenerated, and
`scripts/verify-passes.rb` re-runs every claimed pass on a real Ruby.
`scripts/lazy-oracle.rb` reproduces the tables above. `cargo test` green in
debug.

## Results

`core/enumerator` **29 → 120 of 390 (7% → 31%)**, 0 failed. `core/enumerable`
288 → 289, `core/range` 110 → 113. Corpus total **2806 → 2901**, 0 failed.
`scripts/verify-passes.rb` re-ran all 774 claimed passes under
`core/{enumerator,range,enumerable,array}` on ruby 4.0.6 — all agree, so none of
the new passes is a false one. A 72-probe differential against CRuby agrees on
71; the 72nd is `(1..2.5).size`, which reports `Float#floor` missing by name.

### What had to be measured rather than reasoned about

The oracle tables are in `scripts/lazy-oracle.rb`, and the two divergences from
`Enumerable` it found are in the PRD above. Three more came out of running a
differential against CRuby rather than out of the specs, which is the point of
running one — every one of them was green under `scripts/spec.sh`.

- **`Lazy#with_index` with a block discards the block's value.** The block runs
  for its side effect and the *original* element passes through, so
  `(10..13).lazy.with_index(1) { "X" }.first(3)` is `[10, 11, 12]` and not
  `["X", "X", "X"]`. Mapping the value — the obvious reading, and what the
  eager `Enumerator#with_index` does — is wrong here. No spec in
  `core/enumerator/lazy/` separates the two.
- **`[1, 2].each.size` is 2, not nil.** `Array#each` hands its enumerator a size
  block; without one `Enumerator::Chain#size` summed `nil` and answered `nil`.
  Legal-but-incomplete rather than wrong, and one line to make right.
- **`Range#size` is decided by the *begin*, three different ways.** An Integer
  begin computes; a `Numeric`-but-not-Integer begin or a `nil` one raises
  TypeError "can't iterate from <Class>"; anything else answers `nil`. So
  `(1.0..2.0)` and `(..1)` raise while `("a".."c")` is nil — and the first
  version of this method, which checked the *end* first, passed every
  `core/range` spec but one and got `(..1)` wrong.

### Known, attributed, not fixed here

- **`Float::INFINITY` is now the top blocker in `core/enumerator` at 73**, up
  from 26, because `Range#size` on an endless range and
  `Enumerator.produce(...).size` both mention it rather than answering `nil`.
  That is the intended trade: the constant cannot be faked, since infinity is
  not an encodable flonum — `(bits >> 60) & 0b111` is `0b111` for it, outside
  both flonum exponents — so it needs the heap `Float` box of #18. At 73 here
  and 59 elsewhere under `core/` it is the cheapest large unblock left.
- **`Float#floor` does not exist**, so a Float-ended range reports it by name.
  Also #18.
- **`next`, `peek`, `rewind`** remain 34 examples refusing by name. #16.

### Found by reviewing the diff rather than by any check

`Enumerator::Lazy#to_enum` first ignored its `method` argument and always
enumerated `each`. Nothing caught it: no spec reaches it, the corpus stayed at
0 failed, and `verify-passes` only re-runs what is claimed. Ruby honours the
name — `(1..6).lazy.to_enum(:each_slice, 2).first(2)` is `[[1, 2], [3, 4]]` —
so it now drives the named method, and the differential covers it.
