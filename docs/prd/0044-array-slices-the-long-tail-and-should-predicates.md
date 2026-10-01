# PRD 0044 — `Array`: slices, the long tail, and `should.predicate?`

Issue: [#21](https://github.com/ar4mirez/spinel/issues/21) · Phase 2 ·
`area:core-lib`

## Objective

`Array` in Ruby on the primitive backing store. PRD 0038 put #21 after #22 and
asked that it be re-measured first.

## Baseline

`core/array` 258 / 1229 (21%), corpus 3330, 0 failed. Blocked list: `mock` 139
(#145), `pack` 95, `Array[]` on a class 52 + 14, `[]=` on a splice 47, `fill`
35, `should` on an Array 32, `[]` on a slice 31, then a long tail of methods
that had never been written.

## The engine: slices and splices

`Array#[]` and `#[]=` refused anything but one Integer index. They now take
`(start, length)` and a Range with Integer or nil ends, CRuby's `rb_ary_subseq`,
`rb_range_component_beg_len` and `rb_ary_splice` rule for rule — including the
error wordings (`index -5 too small for array; minimum: -3`,
`negative length (-1)`, `-5..-4 out of range`). A Float index is truncated as
`Float#to_int` would; an index needing a Ruby `to_int` still refuses, because a
native cannot send. `slice` is the same native, `at` its one-index form, and
`pop(n)` answers its Array.

**A bug found on the way:** `a[5] = x` past the end raised IndexError. The store
used the *read* rule, which answers nil there; the gap now fills with nil.

## The Ruby

`Array.[]`, `Array.try_convert`, `-`/`difference`, `&`/`intersection`,
`intersect?`, `|`/`union`, `*`, `replace`, `map!`/`collect!`, `select!`/
`filter!`, `keep_if`, `reject!`, `delete_if`, `delete`, `delete_at`, `compact`,
`compact!`, `uniq!`, `reverse!`, `sort!`, `sort_by!`, `rotate`, `rotate!`,
`insert`, `fill`, `prepend`, `values_at`, `fetch`, `fetch_values`, `dig`,
`assoc`, `rassoc`, `rindex`, `rfind`, `reverse_each`, `transpose`, `flatten`,
`flatten!`, `slice!`, `bsearch`, `bsearch_index`, `permutation`, `combination`,
`repeated_combination`, `repeated_permutation`, `product`.

Set membership is `hash`/`eql?` — a Hash's rule, from #22. The mutators that
filter in place do so as they go, so a block that raises part-way leaves what it
had already removed removed (`ary_reject_bang`); `reverse_each` and `rfind`
re-read the size after every yield. A 70-line differential against ruby 4.0.7 —
every edge case written down before any of it was — came out identical.

## The harness, again

**`x.should.predicate?(args)`** — mspec's form for anything that is not `==`,
`=~` or `raise`: it holds when `x.predicate?(args)` is truthy, and
`should_not` when it is not. It was the largest Array row after `mock`, and it
is in every directory: the corpus went from 3330 to 4134 on this alone.
`verify-passes.rb`'s `ShouldProxy` became a `BasicObject` so that `equal?`,
`frozen?` and `is_a?` reach its `method_missing` instead of `Object`'s own.

**`before :all` outlives its group.** mspec runs a file against one
environment object, so an ivar a `before :all` set is still there in the
`describe` blocks after it — `fill_spec.rb` assigns `@never_passed` once and
passes `&@never_passed` in two later groups. The harness now replays an ended
group's `before :all` hooks ahead of later examples.

## What the predicate form surfaced

Forty-eight examples that had been blocked on `should` ran and disagreed.
Forty are fixed here; eight are tagged (six #39 globals, one #28, one #27).

- **`==` on an object skipped its own `==` when both sides were the same
  object.** The fast path's identity shortcut now applies only to immediates
  and the core types it understands. And `BasicObject#==` called `equal?`,
  which a class may override; it is the C identity now (`__identical__`), with
  `equal?` and `__id__` on `BasicObject` where CRuby puts them.
- **`Kernel#===`** checks identity first.
- **Frozen, and the same object every time:** `true.to_s`, `false.to_s`,
  `nil.to_s`, `Symbol#name`, `Module#name`; `MatchData#string` is a frozen
  copy; `Range` instances are frozen.
- **Literal identity.** `"abc".freeze` is one interned String (a per-heap
  fstring table beside the regexp literal cache), and so is every literal under
  `# frozen_string_literal: true` — which Spinel was not honouring at all.
  `(1..3)` with literal ends is one object per site (`Insn::OnceGet`/`OnceSet`,
  numbered with the `/o` regexp sites).
- **`+str` was compiled to `str`.** It is a send now, and `String#+@` answers an
  unfrozen copy of a frozen String; `Numeric#+@` is `self`.
- **Subclass answers:** `Array#to_a`, `String#to_s` and `Hash#to_h` on a
  subclass answer the base class; `Hash[]` and `merge` keep it.
- **`Hash#dig`** hands the rest of the path to an inner object in one call;
  `transform_keys`/`transform_values` without a block are sized.
- **`Exception#exception(msg)`** is a clone carrying the new message, not a
  re-initialised instance.
- **`Enumerator#initialize`, `Chain#initialize`, `Product#initialize`** answer
  self; `Lazy#chunk` and the `slice_*` family stay lazy.
- **`Integer#dup`/`clone`** answer the integer, a bignum included.
- **Hidden state.** The core library's own ivars are spelled `@__name__` and
  `instance_variables` leaves them out: `{}.instance_variables` is `[]`, as in
  CRuby. `Hash` and `Range` moved to that spelling.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/array` | 258 / 1229 (21%) | **785 / 1229 (64%)** |
| `core/hash` | 265 | **335** |
| `core/enumerable` | 304 | **348** |
| `core/enumerator` | 126 | **170** |
| `core/kernel` | 134 | **166** |
| **corpus** | 3330 | **4178** |

0 failed. All 4178 passes re-run on ruby 4.0.7 and agree.

## What blocks the rest of `core/array`

`mock` and mspec's other helpers (#145), `pack` (shares its format machinery
with `String#unpack`, so it goes with #19), `sample`/`shuffle` (`Random`),
`instance_method` (#28), and `Float::INFINITY` (#18).
