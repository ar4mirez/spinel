# PRD 0060 — every double is a Float, and `Math`

Issues: [#18](https://github.com/ar4mirez/spinel/issues/18) (the boxed half)
and [#35](https://github.com/ar4mirez/spinel/issues/35) · Phase 2 ·
`area:engine`, `area:core-lib`

## Objective

`Math` was next on the list, and it could not be written: a Float here was a
flonum and nothing else. `1 / 0.0` was a refusal, `Float::INFINITY` did not
exist, and neither did NaN, `-0.0`, or any double outside roughly 1e-77 to
1e77. ruby/spec's `Math` examples ask about exactly those values, and 171
examples elsewhere stopped at an Integer divided by a Float zero. So this
slice is the prerequisite first, and `Math` on top of it.

## Baseline

Corpus 15640 passed, 0 failed. `core/math` 0 of 235, `core/float` 71 of 261.

## Design

**A Float that does not fit a word is boxed.** `crates/spinel-vm/src/float.rs`
is two functions. `value(f)` answers a flonum when `f` fits one and otherwise
an eight-byte heap cell whose class is `Float`, frozen; `read(v)` reads
either. It is what `bignum.rs` is to a fixnum. Every place the interpreter
made or read a float goes through them, so the arithmetic no longer has a
refusal in the middle of the type: `float_op` cannot fail.

**The boundary is not observable.** A boxed Float is of class `Float`, takes
no singleton class, is frozen, and compares, hashes and prints as its
value. `Float#hash` is by bits with `-0.0` folded onto `0.0`, as `eql?`
requires. Two boxed floats holding one number are different objects, which is
CRuby's answer too.

**NaN is not equal to itself, except by identity inside an Array.** `nan ==
nan` is false through the numeric path. `[nan] == [nan]` for one NaN object
is true, because CRuby's `rb_equal` asks identity first; the array loop now
does.

**`Float` grew what the values need.** The constants; `nan?`, `infinite?`,
`finite?`; `to_i` and `truncate` through one truncation primitive that
answers an Integer of any size and raises `FloatDomainError` for a value
that has none; `floor`, `ceil` and `round` with digits and `half:`, following
CRuby's `float_round_overflow`, `float_round_underflow` and the
neighbour check in `round_half_up`; `fdiv`, `div`, `divmod`, `modulo`, `**`;
and `<=>` against an object that answers `infinite?`.

**`Math` is one primitive and Ruby's rules.** `__math__(:name, x, y)` makes
the libm call. Converting the argument as `Float()` does, and raising
`Math::DomainError` outside a function's domain, are Ruby. `erf`, `erfc`,
`tgamma`, `lgamma_r`, `frexp` and `ldexp` are declared `extern "C"`: `std`
links libm already, so this adds no dependency. `Math.gamma` is exact for
the whole numbers a Float holds a factorial for, and `Math.log` of an Integer
too wide for a Float takes it apart first.

## Found on the way

- `sprintf` printed `inf` and `nan` with an exponent; Ruby prints `Inf` and
  `NaN`, padded with spaces even under `0`.
- `Enumerator.produce`, `Enumerator::Chain#size` and `Product#size` answer
  `Float::INFINITY` where they had been waiting for the constant.
- `Array#sum` no longer turns an infinity into NaN by compensating for it.
- `Numeric#<=>` answered 0 for NaN; it is nil.
- `Integer#**` with a Float exponent is a Float.
- `Integer#fdiv` of two bignums no longer divides Infinity by Infinity.

## Not in this slice

The rest of #18: `Float#to_s` is unchanged, `next_float`, `prev_float`,
`rationalize`, `to_r`, and `Float#**` with a negative base and a fractional
exponent, which is a Complex. `Kernel#Float` and `String#to_f` are #181.
`Integer#fdiv` of two bignums is right to about the last bit, not exactly.

## Results

| | passed | failed | blocked |
|---|---:|---:|---:|
| before | 15640 | 0 | 14905 |
| this slice | **16161** | **0** | 14388 |

`core/math` 0 to 217 of 243, `core/float` 71 to 198 of 261. Five examples
that became reachable are tagged: two for `Integer#fdiv`'s last bit (#17),
and one each for #19, #23 and #28.

`tests/eval.txt` holds 19 new rows, measured on ruby 4.0.6.
