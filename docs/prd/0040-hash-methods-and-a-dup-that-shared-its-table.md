# PRD 0040 — `Hash`: the methods, and a `dup` that shared its table

Issue: [#22](https://github.com/ar4mirez/spinel/issues/22) · Phase 2 ·
`area:core-lib`

## Objective

Third slice on the order [PRD 0038](0038-phase-2-p0-order-and-the-lazy-chain.md)
measured. `core/hash` is 65 / 460 at 14%, and unlike #19 or #21 nothing in its
blocked list needs a new primitive: it is a long tail of methods that were never
written.

## Baseline

`core/hash`: 460 examples · 65 passed · 14%. Corpus 2914, 0 failed. The blocked
list is `compare_by_identity` 35, then `to_proc` 11, `merge` 10, `Hash.[]` 9,
`flatten` 8, `merge!` 7, `rassoc` 7, `assoc` 6, `dig` 6, `reject!` 6,
`replace` 6 — no single large row, ~120 examples in the tail.

## What this slice writes

All Ruby, in `core/hash.rb`. No Rust.

`merge`, `merge!`/`update`, `replace`, `clear`, `assoc`, `rassoc`, `dig`,
`values_at`, `fetch_values`, `slice`, `except`, `reject!`, `delete_if`,
`select!`/`filter!`, `keep_if`, `compact`, `compact!`, `invert`, `flatten`,
`transform_values`, `transform_values!`, `transform_keys`, `to_proc`,
`each_entry`, `default=`, `default_proc=`, `Hash.[]`, `==`, `eql?`, and
`initialize_copy` with the `dup`/`clone` that call it.

Deliberately not written:

- **`compare_by_identity`** (35 examples, the largest row) changes which of
  `hash`/`eql?` the table consults, which is the table's business rather than a
  method's. It is the one item here with engine surface and is left for a slice
  that can do it properly.
- **`Hash#hash`** — see below.
- **`ruby2_keywords_hash`** is #200.

## The bug this slice found

**`Hash#dup` shared its backing store.** `h.dup[:b] = 2` mutated `h`.

`Kernel#dup` is a shallow copy of the object's slots. For `Array` that is the
elements, so `Array#dup` is correct. For `Hash` it is one ivar *pointing at* an
Array of pairs — so the copy and the original shared one table, and every read
still looked right.

It surfaced through `merge_spec.rb`'s "processes entries with same order as
merge()", which calls `h.merge(h)` and then `h.merge!(h)`: the first call's
`dup` had already written into `h`, so the second saw values the first
invented. The spec's own subject is ordering; the failure was aliasing.

`Hash#initialize_copy` fixes it, and `dup`/`clone` are overridden to call it
because `Kernel#dup` does not yet — that is
[#201](https://github.com/ar4mirez/spinel/issues/201). When #201 lands both
overrides delete and `initialize_copy` stays as written.

**Any core class whose representation is an ivar pointing at a mutable object
has this bug today.** `Hash` is the one this slice looked at.

## `Hash#hash`, written and then reverted

Two hashes that are `eql?` have to answer the same `hash`, so `eql?` is not
usable for a Hash-as-a-key without it. A content digest was written, and it
hangs: `hash_spec.rb` builds `h[:a] = h` and asks for `h.hash`, and a recursive
walk over a self-referential table does not terminate. One spec file went from
0.2s to over 100s.

CRuby guards this with `rb_exec_recursive`. Doing it in Ruby needs a threaded
seen-list **and** an `Array#hash` that joins the same list, because the spec
also recurses through `h[:x] = [h]` — and `Array#hash` is #21's. A
wrong-but-terminating digest would make `{a: 1}` findable as a key and break
the first time two hashes collided, so identity stays until both halves exist.

`==` and `eql?` survive the same structures, because both short-circuit on
`equal?` before recursing.

## Check

`scripts/spec.sh --platform=linux` 0 failed, `bench/spec-status.md` regenerated,
`scripts/verify-passes.rb` re-runs every claimed pass, `cargo test` green in
debug, `cargo clippy --all-targets` clean.

## Results

`core/hash` **65 → 153 of 460 (14% → 33%)**, corpus **2914 → 3002**, 0 failed.
All 153 claimed passes re-run on ruby 4.0.6 agree. A 47-probe differential
against CRuby agrees on 45: the two that differ are `Hash#inspect` printing
`{:a => 1}` where Ruby 3.4+ prints `{a: 1}`
([#216](https://github.com/ar4mirez/spinel/issues/216)) and the deliberately
absent `Hash#hash`.

### What had to be measured rather than reasoned about

Eleven specs failed on the first build, and every one was a rule that reads
backwards from the obvious:

- **`merge` with no arguments still answers a new Hash.** `h.merge.equal?(h)`
  is false.
- **`assoc` compares with `==`, not the table's `eql?`.** `{1.0 => :v}
  .assoc(1)` finds the entry that `key?(1)` does not, so it has to scan rather
  than index.
- **`reject!`, `select!` and `compact!` answer nil when they changed nothing**,
  while `delete_if` and `keep_if` always answer self. That is the *only*
  difference between the two pairs.
- **The default rides along on some copies and not others.** `compact` and
  `replace` keep it — `replace` takes the *argument's* — while `Hash[]`,
  `except` and `slice` must not, which is why those three build a fresh hash
  instead of `dup`ing one.
- **`slice` reads through the ordinary `Hash#[]`** even on a subclass that
  overrides `[]`.
- **`default_proc=` type-checks twice**, and only one of the checks is about
  arity: a non-Proc is "wrong default_proc type X (expected Proc)", and a
  *lambda* — not a proc — of arity other than 2 is "default_proc takes two
  arguments (2 for N)".
- **`transform_keys(nil)` is a TypeError**, where `transform_keys` with no
  argument is fine.
- **`flatten` had to be written out rather than handed to `Array#flatten`**,
  which does not exist (#21). Calling it would have raised NoMethodError — and a
  raising method reports *blocked*, not failed, so every `flatten` spec would
  have stayed green-looking while the method did nothing. Found by the
  differential, not by the specs.
