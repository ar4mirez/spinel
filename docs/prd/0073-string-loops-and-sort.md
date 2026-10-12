# PRD 0073 — the loops that were quadratic, and the sort

Issue: [#299](https://github.com/ar4mirez/spinel/issues/299) · Phase 2 ·
`area:engine`, `area:core-lib`

## Objective

`gsub`, `split` and `scan` took about a millisecond a match; a `while` loop
over a String's characters was quadratic; sorting sixteen thousand Integers
took twenty-eight seconds. None of it was a wrong answer, and all of it was
in the way: `core/dir` was the slowest chunk of a corpus run by a wide
margin.

## Baseline

`bench/string_loops.rb`, added here, on `main`. The pull request has the
table.

## What was slow, and why

**Every `s[i]` built a table of the whole String.** `String#[]` with
Integers went through a Ruby method — a splat, a helper, an Array and its
`min` — to reach a primitive that then recorded the byte offset of every
character in the String to find one. `String#length` built the same table to
count its rows. So a loop over a String's characters was quadratic twice.

**So did every offset a match was asked for.** `MatchData#begin` and `#end`
counted the characters before a byte by tabulating them, and `gsub` asks
several times a match.

**`<<` was six Ruby calls.** The frozen check, the coercion, the encoding
negotiation, the splice and the encoding write were each a method.

**A sort compared through the whole coercion protocol.** `1 <=> 2` reached
`Numeric#<=>`, which asks `<`, then `>`, then `==`, each a dispatch — inside a
merge sort written in Ruby that allocates an Array a merge.

## Design

**One byte, one character, no table.** `strings::single_byte` says whether a
character index *is* a byte index: a single-byte encoding, or nothing but
ASCII in an ASCII-compatible one. `String#[]`, `String#length` and a match's
offsets take that path when it holds. Where it does not and only a count is
wanted, UTF-8 is counted without keeping where each character was.

**The plain case is the primitive's, and the rest is sent on.** `String#[]`
with Integers, `String#<<` with a String that can be appended as it stands,
and `Integer#<=>` between two unboxed numbers are answered in the primitive.
Anything else — a Range, a Regexp, an index that needs `to_int`, two
encodings to reconcile, a bignum, an object that coerces — is forwarded to
the Ruby method that was there before, under a private name. The semantics
stay where they were written; only the common case stops paying for them.
This is the shape #298 arrived at for `MatchData#[]`.

**A sort of plain things is a primitive.** `Array#__sort_plain__` sorts an
Array that is all fixnums, or all Strings of class String in one encoding,
and answers nil for any other, which is the Ruby merge sort's. It is stable,
as that sort is. It checks first that `Integer#<=>` and `String#<=>` are
still the primitives: a program that has written its own is asked. `sort_by`
uses the same primitive on its keys.

## Not done

- A regexp match from a position still copies the subject and checks that it
  is UTF-8 each time, so `gsub`, `split` and `scan` are still linear *per
  match*. They are far from the numbers above and not yet near CRuby's. A
  primitive that finds every match in one pass is the next step.
- `sort` with a block is the Ruby merge sort with a faster comparison: about
  a hundred times CRuby still.
- A String that is not all single-byte still tabulates for `s[i]`. A cached
  character index on the String would be the fix.

## Check

`bench/string_loops.rb`, before and after and against `ruby --yjit`, in the
pull request. Rows in `crates/spinel-vm/tests/eval.txt` for what the
primitives must still answer, measured on ruby 4.0.6.
