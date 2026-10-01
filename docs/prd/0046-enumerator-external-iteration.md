# PRD 0046 — `Enumerator`: external iteration on fibers

Issue: [#26](https://github.com/ar4mirez/spinel/issues/26) · Phase 2 ·
`area:core-lib`

## Objective

The half of #26 PRD 0038 left: `next`, `peek`, `rewind`, `next_values`,
`peek_values` and `feed`, which raised `NotImplementedError` naming #16 rather
than being faked with a `to_a` buffer — a buffer answers `next` correctly and
`Enumerator.new { loop { y << rand } }.next` never.

## Baseline

`core/enumerator` 170 / 390 after #21's harness work. Corpus 4252 with #16.

## What this slice writes

All Ruby, in `core/enumerator.rb`, on #16's `Fiber`:

- The enumerator keeps a fiber that runs `each`, suspending at every element
  with `[:value, args]` and ending with `[:done, result]`. `next_values` resumes
  it; `next` unwraps one value and leaves several as their Array.
- At the end both raise StopIteration carrying `each`'s return value as
  `result`, and keep raising until `rewind`.
- `peek` buffers one element; `rewind` drops the fiber and the buffer and calls
  the source's own `rewind` when it has one.
- `feed` sets what the producer's `yield` answers next — once; a second `feed`
  before that is a TypeError.
- A producer that raises ends that iteration, and the next `next` starts again.
- `Chain#rewind` rewinds the sources iterated so far, last first;
  `Product#rewind` rewinds every source in order. Both undefine `next` and the
  rest, as CRuby does.

## Found on the way

- **`Yielder#yield` answered nil.** CRuby's answers what the consuming block
  did — that is how `feed` reaches the producer at all — and it had been
  written as nil with a comment saying so. Fixed, and `Yielder#call`, which
  CRuby does not have, is gone.
- **`Enumerable#to_a`'s block answered the Array it was building**, so a
  producer reading `y.yield` saw it. CRuby's C collector answers nil.
- **`Kernel#loop` answered nil when a StopIteration ended it**; it answers the
  StopIteration's `result`, so `loop { e.next }` is `e`'s return value.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/enumerator` | 170 / 390 | **216 / 390** |
| **corpus** | 4252 | **4300** |

0 failed. `core/enumerator`'s largest remaining row is `Float::INFINITY` (73),
which is #18's — an endless enumerator's size is Infinity, and this VM has no
boxed Float to hold one.
