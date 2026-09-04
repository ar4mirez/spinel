# PRD 0006 — `Value`: tagged immediates for fixnum, flonum, symbol, and special constants

Tracks [#6](https://github.com/ar4mirez/spinel/issues/6). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:L`, `area:engine`.

## Objective

The word every Ruby object travels as. Fixnums, most floats, symbols, and
`nil`/`true`/`false`/`undef` have to be immediate — the word *is* the object — because
a Ruby program allocating for `1 + 1` cannot be made fast afterwards.

This is the slice with the worst change cost in the project. Every later slice reads
`Value`: the interpreter branches on its tag, the GC decides what to trace by it, the
method cache keys on it, and the JIT emits the tag tests inline. So the encoding is
settled here, on paper, with the bit patterns written into `docs/engine.md` — not left
to be discovered by the first thing that breaks.

## Non-goals

- **The heap.** There is no `Heap` yet ([#7](https://github.com/ar4mirez/spinel/issues/7)).
  This slice defines the pointer *tag* and hands back the pointer it was given; the
  object header, the allocator, and `HandleScope` are #7's. `Value::heap` therefore
  takes a `NonNull<()>` — deliberately opaque, so #7 can pick the header type without
  changing this file.
- **Bignum promotion.** The issue's definition of done says "overflow into bignum", and
  what that means without a heap is the *boundary*: `Value::fixnum` returns `None`
  exactly when an `Integer` must become a heap bignum. Allocating the bignum needs #7;
  the arithmetic that triggers it belongs to `Integer`
  ([#17](https://github.com/ar4mirez/spinel/issues/17)). Encoding a promotion path that
  nothing can execute would be a guess with a test written to agree with it.
- **The symbol table.** `SymbolId` round-trips through a `Value` here. Interning is
  shared, append-only state under `crates/spinel-vm/src/shared/`, which arrives with the
  bootstrap classes ([#8](https://github.com/ar4mirez/spinel/issues/8)).
- **A ruby/spec delta.** Every engine slice from phase 1 on states one, and this one
  honestly cannot: nothing executes a line of Ruby until the interpreter lands
  ([#10](https://github.com/ar4mirez/spinel/issues/10)), so every example stays
  `blocked`. Claiming otherwise would be the exact failure
  [PRD 0005](0005-ruby-spec-harness.md) built the `blocked` column to prevent. The
  issue asks for Rust unit tests instead, and that is what this slice is judged on.

## Users

| User | Needs from this slice |
|---|---|
| [#7](https://github.com/ar4mirez/spinel/issues/7) `Heap`, GC | A tag that says "this word is a pointer", and a niche so an empty slot costs no extra word |
| [#10](https://github.com/ar4mirez/spinel/issues/10) bytecode, interpreter | An exhaustive `match` on the tag, and a truthiness test with no branch in it |
| [#15](https://github.com/ar4mirez/spinel/issues/15) `core/*.rb` | A stated fixnum range, so `Integer` knows when it must promote |
| [#120](https://github.com/ar4mirez/spinel/issues/120) JIT | Tag tests small enough to inline at every call site |
| Anyone debugging a later slice | A failing assertion that prints `nil`, not `Value(4)` |

## Requirements

### R1 — `Value` is one word, and so is `Option<Value>`

`#[repr(transparent)]` over a `NonZeroU64`. The zero word is not a `Value`, which buys
two things: `Option<Value>` is still eight bytes, and a slot that was never written
reads as an invalid `Value` rather than as a plausible object. Ivar slot arrays, hash
entries, and free lists all want the first; the GC wants the second.

Asserted at compile time, not by a reviewer:

```rust
const _: () = {
    assert!(size_of::<Value>() == size_of::<*const ()>());
    assert!(size_of::<Option<Value>>() == size_of::<Value>());
};
```

A 32-bit target fails the build with a message that says why, rather than silently
producing a `Value` that cannot hold a pointer.

### R2 — Every immediate round-trips without allocating

| low bits | the rest of the word | kind |
|---|---|---|
| `1` | 63-bit signed integer | fixnum |
| `10` | double, rotated left by three | flonum |
| `0100` | ordinal | `nil`, `false`, `true`, `undef` |
| `1100` | symbol id | static symbol |
| `000` | 8-byte-aligned pointer | heap object |

Constructors are `const fn`, so a literal in compiled bytecode is a constant, not a
call.

### R3 — Fixnum is 63-bit and says when it overflows

`Value::FIXNUM_MIN ..= Value::FIXNUM_MAX` is ±2^62. `Value::fixnum` returns `Option`,
and `None` is the single, checkable statement that `Integer` must promote to a bignum.
Returning a wrapped value instead would put the boundary in the caller, where each
caller could get it wrong differently.

### R4 — Flonum covers the doubles programs actually hold, and refuses the rest

The encodable band is the doubles whose top three exponent bits are `011` or `100`:
magnitudes between roughly 1.7e-77 and 1.8e77. Encoding rotates left by three, which
lifts the sign and the top two exponent bits into the low bits where the tag overwrites
two of them; the decoder reconstructs those two from the third, because within the band
they are `011` or `100` and never anything else.

Two cases are not arithmetic:

- `+0.0` has no exponent in the band, and is common enough to be worth the one spare
  bit pattern.
- `2**-255` is in the band and its rotation *is* that spare pattern. It allocates, so
  that `0.0` and `2**-255` can never become the same object.

### R5 — Bitwise equality is `equal?`

Derived `PartialEq`/`Eq`/`Hash` on the word. This is only sound because the band
excludes NaN, the infinities, and `-0.0` — the three cases where bit equality and `==`
disagree. That is not a happy accident to be discovered later by a wrong answer: it is
the property that lets the method cache and the interpreter compare `Value`s directly,
so `flonum_refuses_the_values_that_would_break_bit_equality` names it and pins it.

### R6 — One tag per value, and no way to forge one

Exactly one of fixnum, flonum, symbol, constant, pointer is true for any `Value`. The
one way to break that is an unaligned heap pointer: the low three bits are the tag, so
a pointer at `0x4` would read back as `nil`. `Value::heap` therefore uses `assert!`,
not `debug_assert!` — an unaligned pointer is not a wrong answer to be caught in a debug
run, it is an object silently becoming a different kind of object in a release one.

### R7 — Truthiness is one operation

`nil` and `false` differ in exactly one bit, so `is_truthy` is an `and` and a compare.
Ruby branches on truthiness more often than on anything else.

## Definition of done

The issue's three boxes, and where each is discharged:

| From the issue | Where |
|---|---|
| `Value` is pointer-sized | `const _` assertion, plus `value_is_pointer_sized_and_leaves_a_niche` |
| Fixnum, flonum, symbol, nil/true/false round-trip without allocating | Seven round-trip tests; no allocation is possible, the crate has no allocator |
| Rust unit tests cover tag boundaries and overflow into bignum | `fixnum_boundaries_hold_and_overflow_becomes_a_bignum`, and a 16,384-double sweep for the flonum band |

`cargo test -p spinel-vm`: 15 passing, 13 of them new.

## Tasks

| | Task | Check |
|---|---|---|
| T1 | `crates/spinel-vm/src/value.rs`, the tag scheme of R2 | `cargo build` |
| T2 | One word, with a niche | `const _` assertion; the build fails on a 32-bit target |
| T3 | Fixnum with an `Option` at ±2^62 | `fixnum_boundaries_hold_and_overflow_becomes_a_bignum` |
| T4 | Flonum rotation, `+0.0`, and the `2**-255` collision | `flonums_round_trip_inside_the_band_and_are_refused_outside_it` over every exponent |
| T5 | Symbol and the four constants | `symbols_round_trip`, `the_special_constants_are_four_distinct_objects` |
| T6 | `unpack` for an exhaustive `match` | `every_value_has_exactly_one_tag` |
| T7 | Branch-free `is_truthy` | `only_nil_and_false_are_falsy`, and the arm64 listing below |
| T8 | `assert!` on heap-pointer alignment | `an_unaligned_heap_pointer_is_caught_rather_than_read_back_as_a_symbol` |
| T9 | `Debug` that prints the Ruby object | `debug_reads_like_the_ruby_object` |
| T10 | `docs/engine.md` carries the real bit patterns | The table above matches the code |

## What the audit caught

The issue has no user interface, so the surface audited for clarity was the API every
later slice writes against, and the code the compiler emits for it.

- **The module diagram was wrong.** The header opened with a box-drawing diagram whose
  columns did not line up, in the most-read part of the most-read file in the crate.
  Replaced with a table. A diagram that has to be decoded is worse than no diagram.
- **Pointers round-tripped through `as` casts.** Correct today, and invisible to Miri
  under strict provenance — in the one crate whose next slice is a garbage collector.
  Now `expose_provenance` and `with_exposed_provenance_mut`, which say that an integer
  round trip is intended. Cost: `as_heap` and `unpack` are no longer `const fn`, which
  nothing needs them to be.
- **Codegen checked rather than assumed.** R7 claims "one operation"; claims about
  generated code are worth what the listing says. Release arm64:

  | | instructions | branches |
  |---|---|---|
  | `is_truthy` | 3 | 0 |
  | `is_immediate` | 2 | 0 |
  | `as_fixnum` | 3 | 0 |
  | `fixnum` with the range check | 5 | 0 |

  LLVM folded the two-sided range check in `fixnum` into a single comparison. Verified
  with temporary `#[no_mangle]` probes and `--emit asm`, then removed: an assertion on
  an instruction listing is a test that fails when a compiler improves.
- **`debug_assert!` on pointer alignment was upgraded to `assert!`.** See R6. The
  failure mode is not a bad value, it is a heap object that reads back as `nil` in
  release.

## Open decisions for owner

1. **`-0.0` allocates.** It is outside the band, as it is in CRuby. Giving it a second
   spare pattern would cost a second excluded in-band double. Left as CRuby has it.
2. **Symbol ids are `u32`, shifted by eight.** 4.3 billion symbols and a four-bit hole
   that keeps the tag byte-aligned in a hex dump. The word has room for 56 bits if #8
   ever wants them.

## Follow-ups

- `impl From<bool> for Value`, the inverse of `is_truthy`. Deliberately not added here:
  it has no caller in the tree today, and the first one to need it — the compiler in
  [#10](https://github.com/ar4mirez/spinel/issues/10) — adds it in three lines.
- Miri in CI under `-Zmiri-strict-provenance`. The pointer API is now the one Miri can
  follow, but installing a nightly toolchain is a change to the build, not to this
  slice. It earns its keep with the GC in
  [#7](https://github.com/ar4mirez/spinel/issues/7).
- `NonNull<()>` becomes the object header type in #7.
