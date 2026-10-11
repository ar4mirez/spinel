# PRD 0070 — a Float, in and out

Follows PRD 0069 · Phase 2 · `area:core-lib`, `area:engine`

## Objective

Two loose ends from `Complex`: `Kernel#Float` did not exist, and a Float did
not always print the way CRuby prints it.

## Baseline

Corpus 18426 passed, 0 failed, after PRD 0069. `core/kernel/Float_spec.rb`
15 of 99.

## Design

**`Kernel#Float` is `to_f` that refuses.** A Float, an Integer or a Rational
is converted; nil is a TypeError; anything else is asked for `to_f` and must
answer a Float. A String must be a number from end to end: decimal with
single underscores between digits, or hexadecimal with an optional fraction
and a power of two (`0x1.8p3`). It is read by the same exact rounding
`String#to_f` uses, and not through a String subclass's own `to_f`.
`exception: false` answers nil where it would raise.

**Printing follows `flo_to_s` in two places it did not.**

- *Where plain digits stop.* It was at 1e15. CRuby prints plain up to
  sixteen digits before the point, but the sixteenth only when a fraction
  follows: `1151450203773890.8` is plain and `1.0e+15` is not.
- *A tie between two shortest spellings.* 900719925474099.25 is exactly as
  near `…99.2` as `…99.3`. Rust's shortest-digits takes the upper; CRuby
  takes the even. When the Float's exact expansion shows a tie and the
  shortest spelling ended on an odd digit, the lower one is used if it reads
  back as the same Float.

Both were found by printing eight thousand Floats — random bit patterns,
dyadic fractions, quarters — on CRuby and here, and comparing; after the
change the two lists are identical.

## Not done

- A hexadecimal String with more than fifty-three significant bits and a
  positive power is converted through an Integer, whose `to_f` rounds as the
  bignum library does.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6, and cases
in `float_to_s`'s unit test. The ruby/spec delta is in the pull request.
