# PRD 0066 — `Rational`

Issues: [#34](https://github.com/ar4mirez/spinel/issues/34) (the Rational
half) and [#227](https://github.com/ar4mirez/spinel/issues/227) (the `3r`
half) · Phase 2 · `area:core-lib`, `area:engine`

## Objective

`Rational` did not exist. 158 of `core/rational`'s 160 examples were blocked,
and so were about eighty in `core/time`, where Ruby keeps the fraction of a
second as one. `Complex` is the other half of #34 and is not here.

## Baseline

Corpus 17286 passed, 0 failed, after PRD 0065. `core/rational` 2 of 160.

## Design

**Two Integers, reduced.** A numerator and a positive denominator with no
common factor, so each value has one spelling and equality compares the two.
Integers are unbounded here, so the schoolbook arithmetic is exact. It is
`core/rational.rb`, with no new primitive.

**`Kernel#Rational` is the way in.** `Rational.new` is undefined, as Ruby's
is. A Float argument is the fraction it exactly is; a String is parsed
strictly; `exception: false` answers nil where it would raise.

**A class under `Numeric` can be instantiated.** `allocate` refused one, on
the ground that `Numeric` had no representation. It has none of its own, and
a class written under it keeps its parts in instance variables like any
other. This is the one change in the VM, and a program's own `Numeric`
subclass gets it too.

**Mixing with Integer and Float is the coercion protocol that was already
there.** `1 + Rational(1, 2)` falls from the Integer primitive to
`Numeric#+`, which asks `Rational#coerce`. `Numeric`'s guard against a pair
it cannot represent used to refuse any `Numeric`; it refuses only an Integer
or a Float now, the two the fast path owns.

**`3r` and `1.5r` compile** to `Rational.__literal__` on the two Integers
Prism already made. `3i` still refuses, for `Complex`.

**The conversions:** `Integer#to_r`, `#rationalize`, `#numerator`,
`#denominator`, `#quo`; `Float#to_r`, exact, from the mantissa and exponent;
`Float#rationalize`, the simplest fraction that still rounds to the Float,
by the continued-fraction walk CRuby uses; `String#to_r`; `nil.to_r`.

**`Integer#**`** answers a Rational for a negative exponent, takes a base
wider than a word by squaring in Ruby, and for an exponent wider than
thirty-two bits has an answer only for 0, 1 and -1 — ArgumentError otherwise,
as CRuby's is.

**`Time` uses it:** `subsec` and `to_r` answer Rationals, and `Time.at`, `+`
and `-` take them.

**`Numeric` has what every number has.** Once a class under it could exist,
`core/numeric`'s examples could run, and three files of them never finished:
`Numeric#<` asked `coerce`, which for two of one class hands the pair back,
and asked again. In Ruby the relational operators are Comparable's and the
arithmetic ones are Integer's and Float's; `Numeric` itself has `<=>` (itself
or nothing), `eql?`, `dup` and `clone` (the number), `zero?`, `abs`, `-@`,
`div`, `%`, `divmod`, `remainder`, `fdiv` and the rounding four, each written
over the operators a subclass defines. They are that now, and the operator
fallbacks that Integer and Float fall into say NoMethodError for any other
class, as Ruby does, instead of recursing.

## Came with it

- `Module#===` answers for a BasicObject, which has no `is_a?` to ask.
  `Rational(basic_object)` is specified, and needed it.
- `String`'s `<`, `<=`, `>` and `>=` are Comparable's. Its own asked nil
  whether it was less than zero, so `"a" < 1` was a NoMethodError where Ruby
  says the comparison failed. Since #159; found measuring.
- `Integer#magnitude` is an alias of `abs`.

## Not done

- `Complex`, and so `Rational#**` with a negative base and a fractional
  exponent, and `3i`.
- `Numeric#step` does not exist for any class. Found measuring.
- `Time` still keeps nanoseconds, not a Rational. Eleven `core/time`
  examples that were blocked on `Rational` now run and fail on that; they are
  tagged to #288, filed with this slice.
- `Marshal`.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6. The
ruby/spec delta is in the pull request.
