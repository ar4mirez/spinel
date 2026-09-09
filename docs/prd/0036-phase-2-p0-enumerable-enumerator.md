# PRD 0036 — Phase 2 P0: the order, and the first slice (`Enumerable`, `Enumerator`)

Issues: [#25](https://github.com/ar4mirez/spinel/issues/25) ·
[#26](https://github.com/ar4mirez/spinel/issues/26) ·
[#16](https://github.com/ar4mirez/spinel/issues/16) ·
[#17](https://github.com/ar4mirez/spinel/issues/17) ·
[#19](https://github.com/ar4mirez/spinel/issues/19) ·
[#21](https://github.com/ar4mirez/spinel/issues/21) ·
[#22](https://github.com/ar4mirez/spinel/issues/22) ·
[#28](https://github.com/ar4mirez/spinel/issues/28) ·
[#29](https://github.com/ar4mirez/spinel/issues/29) ·
Phase 2 · `area:core-lib`

## Objective

Order the nine P0 issues of the Phase 2 milestone by what actually blocks them,
then execute the first slice. The order is not a preference: it is read off a
blocked-reason ranking, because a P0 whose blockers are owned by another P0
cannot be started first no matter how large it is.

## Baseline

Committed `bench/spec-status.md`, `--platform=linux` as `scripts/spec-status.sh`
pins it — spec counts are platform-dependent, so every number here is that
platform.

| directory | issue | files | examples | passed | % |
|---|---|---:|---:|---:|---:|
| `core/array` | #21 | 132 | 1229 | 169 | 14% |
| `core/class` | #28 | 8 | 54 | 12 | 22% |
| `core/encoding` | #19 | 45 | 319 | 0 | 0% |
| `core/enumerable` | #25 | 61 | 446 | 5 | 1% |
| `core/enumerator` | #26 | 73 | 390 | 0 | 0% |
| `core/exception` | #29 | 39 | 233 | 22 | 9% |
| `core/fiber` | #16 | 15 | 124 | 0 | 0% |
| `core/hash` | #22 | 69 | 460 | 44 | 10% |
| `core/integer` | #17 | 70 | 526 | 76 | 14% |
| `core/module` | #28 | 86 | 1021 | 106 | 10% |
| `core/string` | #19 | 151 | 1905 | 51 | 3% |
| **P0 total** | | 749 | 6707 | 485 | **7%** |

Corpus total: 3835 files · 25624 examples · 2228 passed · 0 failed · 21477
blocked · 1919 skipped.

## What blocks the P0 set, measured

`scripts/spec.sh --platform=linux --blocked=0 <dir>`, top reasons per directory.
Three kinds of blocker turn up, and they do not get the same treatment.

**(a) The harness, not the engine.** `mock`, `should_receive`, `be_computed_by`,
`fixture`, `complain`, `ruby_exe`, `tmp`, and the `should` shapes the Rust
matcher in `spec/harness/src/run.rs` does not recognise. Corpus-wide these are
the largest rows — `tmp` 673, `mock` 570, `fixture` 224 — and they are spread
evenly across *every* P0 directory (`core/array` 139 `mock`, `core/string` 102,
`core/module` 49+75 `fixture`). No amount of core-library work moves them. They
are #145, which is P1 in the same milestone and is the milestone's own exit
condition. **This is the single largest multiplier on the P0 set, and it is not
a P0.** Recorded here so the ranking is not mistaken for a core-library ranking.

**(b) Another P0's work.** `uninitialized constant Encoding` (579 corpus-wide,
129 in `core/string`, 22 in `core/integer`) is #19. `uninitialized constant
Enumerator` (119) and `to_enum` (169 across its shapes) are #26. `Fiber` (93) is
#16. `define_method` on an anonymous class (50 in `core/module`) is #28.

**(c) A method nobody has written yet.** The rest, and the only kind a
core-library slice can actually close.

## Order, and why

1. **#25 `Enumerable` + #26 `Enumerator`, together, first.** `core/enumerable`
   is 5/446. Its blocked list is almost entirely `undefined method '<x>' for an
   instance of EnumerableSpecs::Numerous` — category (c), pure Ruby, no engine
   work. Verified against the built binary: `include`, a module method calling
   `each`, `yield` inside it, `block_given?`, `ancestors`, `is_a?`, and
   `send(meth, *args, &block)` all already work, so nothing gates writing the
   module. It is first because it is also the *dependency* of three other P0s:
   `Array`, `Hash` and `Range` today re-implement `map`/`select`/`find`/`any?`
   inline and do not include `Enumerable` at all, and `Array#sort`,
   `Array#sort_by`, `Array#to_h`, `Array#each_slice`, `Array#inject`,
   `Range#map` and `Hash#map` do not exist.

   `Enumerator` rides along because it cannot be separated: every Enumerable
   method called without a block returns one. It is scoped to **internal
   iteration only** — `to_enum`, `each`, `with_index`, `with_object`, `size`,
   and the whole of `Enumerable` on top. `next`/`peek`/`rewind` need a suspended
   call stack, which is #16, and are refused by name until then rather than
   guessed at.

2. **#17 `Integer`** — 76/526, and 168 of its blocks are one missing spec helper
   (`bignum_value`), not a missing method. Small, and it is the arithmetic every
   other class's specs lean on.

3. **#28 `Module`/`Class` reflection** — `define_method`, `instance_eval`,
   `method_missing`, and the definition hooks. Engine work, size L, and the
   fixtures of *every other* directory use it: the `included` hook is already a
   named refusal, hit while verifying slice 1.

4. **#19 `String` + `Encoding`** — the largest single row in the corpus (1905
   examples at 3%) and 579 corpus-wide blocks on the `Encoding` constant alone.
   After #28, because its specs lean on `define_method` fixtures.

5. **#21 `Array`** and **#22 `Hash`** — both collapse in size once #25 lands,
   because the Enumerable half of each stops being their problem. Measure again
   before sizing them.

6. **#16 Fibers**, then the rest of **#26** (`next`/`peek`/`rewind`), then
   **#29 `Exception`**. #29 is last of the P0 set because 32 of its blocks are
   `Errno` (Phase 3, needs `IO`) and 27 are #15, already named.

Not in this PRD's scope but stated because the ranking demands it: **#145 should
be scheduled ahead of #19, #21 and #22.** Those three are the directories where
the harness rows are largest, so their measured ceiling today is not their real
one.

## Slice 1 — `Enumerable` and `Enumerator`

`core/enumerable.rb`, a new `core/enumerator.rb`, `to_enum`/`enum_for` on
`Kernel`, and `include Enumerable` in `Array`, `Hash` and `Range`. Everything in
Ruby; no Rust changes, per rule 5 of "how to vibecode this".

Every method is written against `each` alone, which is what makes the module
correct for any class that defines `each` — including the spec fixtures, which
is what `core/enumerable` actually tests.

### Check

`scripts/spec.sh --platform=linux core/enumerable` moves off 5/446, and
`core/array`, `core/hash`, `core/range`, `core/enumerator` move up in the
regenerated `bench/spec-status.md`. `scripts/verify-passes.rb` re-runs every
newly claimed pass on a real Ruby, so a pass means what it says. `cargo test`
stays green, run unoptimised as well as release.

## Out of scope

- `Enumerator#next`, `#peek`, `#rewind`, `Enumerator::Yielder` from an arbitrary
  block, and `Enumerator::Lazy` — all need a suspended stack (#16).
- Anything that moves a category (a) row. That is #145.

## Results

Slice 1 landed. `bench/spec-status.md` regenerated by `scripts/spec-status.sh`;
these are that file's rows, not typed numbers.

| directory | before | after | delta |
|---|---:|---:|---:|
| `core/enumerable` | 5 (1%) | 288 (65%) | **+283** |
| `core/array` | 169 (14%) | 252 (20%) | **+83** |
| `core/range` | 35 (7%) | 110 (24%) | **+75** |
| `core/enumerator` | 0 (0%) | 29 (7%) | **+29** |
| `core/hash` | 44 (10%) | 65 (14%) | **+21** |
| `core/module` | 106 | 109 | +3 |
| `language` | 1202 | 1205 | +3 |
| `core/kernel` | 117 | 119 | +2 |
| **total** | **2228 (9%)** | **2727 (11%)** | **+499** |

`0 failed` corpus-wide, which is the condition `scripts/spec-status.sh` refuses
to write the table without. `scripts/verify-passes.rb` re-ran all 744 passes across the
five directories on ruby 4.0.6 and all agree.

### What the measurement changed about the plan

- **`Enumerator` did not need fibers.** The issue and the roadmap both said it
  did. Only external iteration does; `to_enum`, the generator block, `with_index`
  and `with_object` are ordinary Ruby, and they are 29 of `core/enumerator`'s
  examples plus the no-block half of all of `Enumerable`. `docs/roadmap.md` is
  corrected in this PR. #26 stays open for `next`/`peek`/`rewind` and
  `Enumerator::Lazy`.
- **`Range` was the second-largest winner and was never in the slice.** #23 is
  P1 and untouched, but `Range` includes `Enumerable`, so 75 examples came free
  — and four of its methods had to be written anyway, because Enumerable cannot
  answer them. `min` on a beginless range is a `RangeError`, not a scan; `max`
  on an endless one likewise; `reverse_each` needs an end to walk back from; and
  `include?` is `cover?` for numbers and a walk for everything else. A generic
  scan would have looped forever on `(1..).min(2)`.
- **The `Array` half was a deletion.** `min`, `max`, `any?`, `all?`, `none?` and
  `reverse_each` came out of `core/array.rb`: Enumerable's take an `n`, a
  pattern, or answer an Enumerator, and Array's did none of that. The file got
  shorter and more correct at once. What stayed is what is genuinely faster over
  indexed storage — the `map`/`select` family, `first`, `last`, `count`,
  `include?`, `sum` — and those only needed the no-block Enumerator guard.

### Two yield conventions, measured not read

The one thing in this slice that could not be reasoned about. A block passed to
`map` sees what `each` yielded with its original arity; a block passed to
`select` sees it packed into one value, so `yield 1, 2` reaches a `map` block as
`1` and a `select` block as `[1, 2]`. There is no rule that predicts the split.
`scripts/enumerable-oracle.rb` prints what the block actually received for each
method on a real Ruby, and the comment at the top of `core/enumerable.rb` is
that output. Guessing it wrong is invisible on a fixture that yields one value
at a time and wrong on `EnumerableSpecs::YieldsMulti`, which is most of the
directory.

### Type-preserving overrides the generic module cannot supply

A module written against `each` alone answers in Arrays, and three Hash methods
must answer in Hashes. Found by differential against CRuby, not by the spec run,
which had already gone green:

- `Hash#select`, `#filter` and `#reject` answer a Hash. Exactly those three —
  `find_all`, `filter_map`, `map` and `partition` all still answer Arrays, which
  is not a rule anyone would guess, so it was measured.
- `Hash#to_h` hands its block the key and value as *two* arguments where
  Enumerable's hands over the one pair, and with no block answers the receiver
  itself rather than a copy.
- `Array#<=>` did not exist, so `Hash#min`, `#max` and `#sort` — which compare
  `[key, value]` pairs — raised NoMethodError. Added, with `NilClass#<=>` under
  it, because a nil inside a pair is `0 <=> 0`, not an error.

### Disagreements this unblocked in code the slice never touched

Budgeted for, and all fixed rather than tagged:

- `Array#sum` did not use Kahan-Babuska compensated summation, so a float sum
  was off by one ulp. Invisible until `Array#sum`'s spec file compiled far
  enough to reach the assertion.
- A **bare `**` in a hash pattern** passed `nil` to `deconstruct_keys` where
  Ruby passes the named key list; only `**rest` passes nil. A one-line compiler
  fix — the AST already told the two apart, `Splat(Some)` versus `Splat(None)`,
  and the match arm ignored it. This surfaced only because `Array#sort` started
  existing, which is what the spec calls on its ScratchPad.

### Tagged, with reasons

Three, all naming another subsystem:

- `Enumerable#grep` and `#grep_v` "does not set `$~`" — asserts a side effect of
  the match rather than the result, and reading `$~` in the caller needs the
  frame-local match global Regexp owns (#33).
- `Enumerable#map` "reports the same arity as the given block" — measures the
  arity of the block Enumerable hands to `each`, which CRuby forwards from the
  caller's. A core library written in Ruby cannot forward arity: the
  `each { |*values| }` block reports -1 whatever the caller wrote.

### Known, attributed, not fixed here

`inject(:+)` and `reduce(:sym)` raise NoMethodError, because `2.send(:+, 1)`
does — while `2.+(1)` works and `2.respond_to?(:+)` is true. That is #239,
reproduced in a plain file rather than assumed; the block forms of both are
correct.

## Next, on the measured order

#17 `Integer` is next per the ranking above, then #28. Two revisions the slice
makes to that order.

**#145 has grown, not shrunk.** Every directory this PR moved now has harness
rows at the top of its blocked list — `core/enumerable`, the directory that went
from 1% to 64%, is led by `should` on an Enumerator (28) and `mock` (17), and
`mock` is the single largest row in `core/array` (139) and the second in
`core/hash` (30) and `core/range` (34). The ceiling for #19, #21 and #22 is the
harness before it is the core library.

**#26's remainder is bigger than its first half.** `core/enumerator` is led by
`lazy` (133), `Enumerator::Product` (37), `Float::INFINITY` (26) and
`Enumerator::Lazy` (19). The lazy chain, not external iteration, is where the
examples are — and `Float::INFINITY` is #18, a one-constant dependency worth
landing first.
