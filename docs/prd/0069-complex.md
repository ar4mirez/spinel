# PRD 0069 — `Complex`

Issues: [#34](https://github.com/ar4mirez/spinel/issues/34) (the Complex
half, which finishes it) and
[#227](https://github.com/ar4mirez/spinel/issues/227) (`3i`, which finishes
it) · Phase 2 · `area:core-lib`, `area:engine`

## Objective

`Complex` did not exist: all 169 of `core/complex`'s examples were blocked,
and with them the part of `Numeric` that only means something beside it —
`i`, `to_c`, `rect`, `polar`, `arg`, `conj`.

## Baseline

Corpus 18059 passed, 0 failed, after PRD 0068.

## Design

**Two real numbers, kept as given.** `Complex(1, 0.0)` holds an Integer and a
Float and prints `(1+0.0i)`. It is `core/complex.rb`, with no new primitive,
and the parts are hidden instance variables, so `instance_variables` is empty
as Ruby's is.

**The arithmetic is over whatever the parts are.** Two Complexes of Integers
multiply to one of Integers and divide to one of Rationals — reduced to
Integers where they come out whole, which is why `c / c` is `(1+0i)`. With a
Float anywhere, division is the scaled form that does not overflow on the
way. A whole power is repeated multiplication and exact; any other goes round
through the length and the angle.

**There is an order only on the real line.** `<=>` answers for two numbers
whose imaginary parts are zero and nil otherwise, and `<`, `floor`, `%`,
`step` and the rest of what a point on a plane has no meaning for are
undefined on the class, as Ruby undefines them.

**`Kernel#Complex`, `Complex.rect`, `Complex.polar`** are the ways in;
`Complex.new` is undefined. Strings are read strictly by `Kernel#Complex` and
leniently by `String#to_c`: `real`, `imag i`, `real±imag i`, `length@angle`.

**`3i`, `2.5i` and `3ri` compile**: the number inside is compiled as itself
and handed over as the imaginary part. Nothing in Ruby's numeric literals is
now refused.

**The real numbers learn about it.** `Numeric#i`, `#to_c`, `#imaginary`,
`#rect`, `#polar`, `#arg`, `#conjugate`; a negative Integer, Float or Rational
to a power that is not whole answers a Complex; `1 <=> Complex(2, 0)` is -1.

## Came with it

- **`String#to_f` did not exist.** `Complex("1.5")` needs it. It reads what
  leads the string and rounds the decimal to the nearest Float exactly, by
  dividing Integers to fifty-five bits and a note of the remainder.
- `Numeric#quo`, `#numerator` and `#denominator`, through `to_r`.

## Not done

- `Complex#*` and `#/` do not have CRuby's special cases for an infinity or a
  NaN in one part.
- A Float prints with a different last digit from CRuby when two shortest
  spellings are equally close to it: `900719925474099.25` is `…99.2` there
  and `…99.3` here. It is the same Float. Found measuring; it is the Float
  printer's, not this slice's.
- `Marshal`.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6. The
ruby/spec delta is in the pull request.
