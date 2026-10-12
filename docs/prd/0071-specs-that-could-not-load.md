# PRD 0071 — the examples that could not load

Phase 2 · `area:core-lib`, `area:engine`

## Objective

Sixty-three records in a full run read `wrong number of arguments (given 2,
expected 0..1)` — examples that stopped before they started, in
`language/method_spec.rb`, `language/lambda_spec.rb` and the three `arity`
specs. They had one cause, and behind it were bugs those examples exist to
find.

## Baseline

Corpus 18540 passed, 0 failed, after PRD 0070.

## The cause

mspec's `evaluate` helper formats its source with `ruby.each_line.to_a`.
`String#each_line` without a block answers `to_enum(:each_line, separator,
chomp: chomp)`, and an Enumerator kept the keywords of the call it
remembered as a trailing Hash among the positional arguments. Replayed, the
Hash was a second positional argument to a method that takes one.

**An Enumerator keeps keywords apart**, and they are keywords again when the
call is made. `to_enum(:m, {x: 2})` — a Hash written as one — is still
positional. `inspect` shows them as they were written, and `each_line`
remembers only what it was given, so `"a".each_line.inspect` no longer shows
a separator nobody passed.

## What the examples then found

**`Proc#arity` did not count keywords.** A lambda's arity is a method's: a
required keyword is one more required argument, and optional keywords leave
the count open. A plain proc does not mind missing arguments, so only a rest
parameter opens its count: `proc { |a = 1| }` is 0 where the lambda is -1.
Nineteen parameter lists, as lambdas and as procs, are pinned in the oracle
table. The count the core library used to tell `warn(message)` from
`warn(message, category:)` was this same number and wanted the old meaning,
so it has its own name now, `positional_arity`.

**`&b` was bound to the wrong block.** A lambda called with no block bound
`b` to the block of the method it was written in. That was the frame's block
standing in for the argument: since PRD 0062 the frame takes the defining
scope's block for `yield` and `block_given?`, and the argument is only what
was passed.

**The harness could not name those examples.** An `evaluate` example carries
its source in its description, which runs over several lines. A record of
one was written raw and read back as several records, so a tag for it could
not be matched and `scripts/verify-passes.rb` could not re-run it. A
description with a newline is now written on one line, quoted and escaped —
mspec's own spelling in a tags file — in the records and in the verifier.

## Not done

- *(Done since: #294.)* A destructuring parameter that binds no name — `proc { |(*), c| }` — still
  does not compile. Three `Proc#arity` examples reach it and are tagged to
  #294, filed with this slice.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6. The
ruby/spec delta is in the pull request.
