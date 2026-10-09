# PRD 0068 — a Time keeps what it was given

Issue: [#288](https://github.com/ar4mirez/spinel/issues/288) · Phase 2 ·
`area:core-lib`

## Objective

PRD 0065 kept an instant as whole seconds and an Integer of nanoseconds, and
a UTC offset as whole seconds, because `Rational` did not exist. It does now
(PRD 0066), and the difference became observable: eleven `core/time` examples
that had been blocked ran and failed, and were tagged to #288.

## Baseline

Corpus 18040 passed, 0 failed, after PRD 0067. Eleven tags to #288.

## Design

**The nanoseconds are an Integer, or a Rational when the instant is finer.**
Nothing else about the representation changes, and a Time made from Integers
never meets a Rational. `Time.__split__` is the one place a number becomes
seconds and a fraction, and it no longer floors: a Float is the fraction it
exactly is, so `Time.at(0.1)` remembers all of it, as CRuby's does.

`nsec` and `usec` floor. `subsec` and `to_r` answer exactly. `%N` with more
than nine digits has more than nine digits to give. `inspect` prints the
decimal when there is one within nine places and the fraction itself when
there is not: `1970-01-01 00:00:00 1/3 UTC`.

**The offset may be a Rational**, from a Rational, a Float, or what answers
`to_r`. `utc_offset` answers it as given. The instant is the civil time less
the offset, exactly. The reading is the other way and is floored, while the
fraction of a second shown stays the instant's own — which is CRuby's
behaviour and is odd: `Time.new(2007, 1, 9, 13, 0, 0, Rational(7201, 2))`
reads `13:00:00.5`. `%z` rounds the offset to the nearest second, and
`inspect` shows its seconds when it has any.

**`Time.new(string, precision:)`** keeps that many digits of the fraction,
nine unless it says; nil or a negative number keeps them all.

## Came with it

- `Time#to_s` and `#inspect` are US-ASCII. Two older tags, to #32, go.

## Not done

- Arithmetic on a Time that has a Rational fraction is Rational arithmetic in
  Ruby, and slower than the Integer path. A Time made from the clock or from
  Integers does not take it.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6. The
ruby/spec delta is in the pull request.
