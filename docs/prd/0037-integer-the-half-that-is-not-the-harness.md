# PRD 0037 — `Integer`: the half of it that is not the harness

Issues: [#17](https://github.com/ar4mirez/spinel/issues/17) · Phase 2 ·
`area:core-lib`

## Objective

Second slice on the order [PRD 0036](0036-phase-2-p0-enumerable-enumerator.md)
measured. `core/integer` is 526 examples at 14%, which looks like a large slice
and is not: over half of what blocks it belongs to #145.

## Baseline

`core/integer`: 70 files · 526 examples · 76 passed · 14%. Blocked reasons from
`scripts/spec.sh --platform=linux --blocked=0 core/integer`, split by who owns
them:

| examples | reason | owner |
|---:|---|---|
| 168 | `bignum_value` | #145 — a four-line mspec helper |
| 45 | `mock` | #145 |
| 22 | `Encoding` | #19 |
| 13 | a `should` shape | #145 |
| 18 | `Integer#[]` | this slice |
| 9 | `fdiv` | this slice |
| 7 | `chr` | #19 — needs a String primitive |
| 6+6+5 | `gcd`, `lcm`, `gcdlcm` | this slice |
| 6 | no block given | this slice — `times`/`upto`/`downto` |
| 5 | `digits` | this slice |
| 4+4+4+3+3 | `bit_length`, `div`, `round`, `pow`, `remainder` | this slice |
| 4 | `size` | this slice |
| 3 | `Integer.sqrt` | this slice |
| 4+3 | `rationalize`, `to_r` | #34 — needs `Rational` |

**226 of the 450 blocked examples are the harness, not `Integer`.** The
genuinely-Integer list is short, and this slice is all of it.

## What this slice writes

All Ruby, in `core/integer.rb`. No Rust.

`[]`, `fdiv`, `gcd`, `lcm`, `gcdlcm`, `digits`, `bit_length`, `div`, `divmod`,
`modulo`, `remainder`, `size`, `pow`, `round`, `ceil`, `floor`, `truncate`,
`Integer.sqrt`, and the no-block Enumerator forms of `times`, `upto` and
`downto` — with the sizes those enumerators report.

Deliberately not written, each because it belongs elsewhere:

- **`chr`** needs a String built from a byte value, which is a primitive this VM
  does not have. #19.
- **`to_r`, `rationalize`** need `Rational`. #34.
- **`div` with a Float operand** needs `Float#floor`, and there is no
  float-to-integer primitive. #18. The Integer case — every case ruby/spec
  reaches without a Float — is right, and the Float case reports the missing
  `Float#floor` by name rather than guessing.
- **`1.fdiv(0)`** is `Infinity`, which this VM cannot represent: only flonums are
  Floats, and infinities need a heap box. #18. Every finite case is right.

## Check

`scripts/spec.sh --platform=linux core/integer` moves off 76/526 with 0 failed,
and `bench/spec-status.md` is regenerated. `scripts/verify-passes.rb` re-runs
every claimed pass on a real Ruby. `cargo test` green in debug.

## Results

`core/integer` **76 → 154 of 526 (14% → 29%)**, 0 failed. `core/numeric` +1.
Corpus total **2727 → 2806**. `scripts/verify-passes.rb` re-ran all 154 on
ruby 4.0.6 — all agree. Differentials against CRuby — 74 probes on the method
set, 25 on `Integer#[]`, 15 on the rounding family — are identical.

### What had to be measured rather than reasoned about

- **`round` goes half away from zero.** `25.round(-1)` is 30, not the 20 a
  banker's rule gives, and `-15.round(-1)` is -20.
- **And it has to be computed on the magnitude.** Shifting the signed value by
  half a step and floor-dividing gives the same answer for every small case and
  is still wrong: floor division of a negative already rounds away from zero, so
  the two biases compound and `-42.round(-100)` comes out as `-10**100` instead
  of 0. Found by probing a digit count large enough to separate them — every
  spec in `round_spec.rb` passed with the broken version.
- **`bit_length` counts against the sign bit**, so `-1` and `0` are both 0 and
  `-256` is 8. `~self` folds the negative case onto the positive one exactly.
- **`size` has the machine word as a floor.** `1` and `2**63` are both 8, `2**64`
  is 9 — bytes for the magnitude, but never fewer than 8.
- **`pow(e, nil)` is a TypeError, not "no modulus".** A default parameter value
  cannot express that, so the modulus is a splat: absent and nil are different
  arguments, and CRuby's message says so —
  "2nd argument not allowed unless all arguments are integers".
- **An enumerator's size is computed on demand**, because asking for it is what
  raises: `1.upto(:a).size` is an ArgumentError about comparison, and it does not
  fire until `size` is called.
- **`remainder` takes the sign of the receiver** where `%` takes the sign of the
  divisor: `-13.remainder(4)` is -1 while `-13 % 4` is 3.
- **`Integer#[]` has three shapes and the field form is not the bit form.**
  `n[i]` clamps a negative index to 0, but `n[i, len]` and `n[range]` move a
  negative `from` *up* into the more significant bits — `0b000001[-3, 4]` is
  `0b1000`. And an upper bound below the lower one is *ignored*, not empty:
  `0b101001101[4..1]` is every bit from 4 up. Detecting that by comparing the
  bounds rather than by a negative computed width matters, because `-4..-5`
  computes a width of exactly 0 and still means unbounded.

### Next

`core/integer`'s remaining blocked list is now led by `bignum_value` **168** and
`mock` **45** — both #145 — then `Encoding` **22** (#19). There is no
Integer-sized slice left here: what remains is owned by other issues, which is
the same conclusion PRD 0036 reached about #19, #21 and #22.
