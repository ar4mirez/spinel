# PRD 0074 — every match in one pass

Issue: [#299](https://github.com/ar4mirez/spinel/issues/299) · Phase 2 ·
`area:engine`, `area:core-lib`

## Objective

PRD 0073 left `gsub`, `split` and `scan` linear *per match*: each asked the
pattern for one match from a position, and each such call copied the subject,
checked that it was UTF-8, counted its characters, and made the MatchData a
frozen copy of it. Three thousand matches read an eight-thousand-character
String three thousand times.

## Baseline

`bench/string_loops.rb` after PRD 0073. The pull request has the table.

## Design

**`Regexp#__match_all__(string)`** answers every match, left to right, as an
Array of MatchData. The subject is read once and one frozen copy of it is
shared by all the matches. After a match the next search starts where it
ended; after an empty one, a character further on. It does not touch `$~`.
It answers nil for a subject the engine cannot read as it stands — invalid
bytes, an encoding that is not ASCII-compatible — and the caller then runs
the one-at-a-time loop it always had, which is where those are refused.

**`scan` and a global `gsub` walk that Array.** What lies between two matches
is copied as it is, which is also what carries the character after an empty
match; the old loop copied that character by hand.

**`split` walks it too, with one rule in place of a retry.** The old loop
re-searched after an empty match at the point the last piece ended. The same
pieces come from the plain walk by passing over an empty match that begins
where the last piece ended: it divides nothing.

**`split` no longer changes `$~`.** Ruby's does not, and the old loop did, by
calling `match`. Measured, and pinned in the oracle table.

## Not done

- `sub`, and `gsub` on a subject the primitive declines, still match one at a
  time.
- A MatchData accessor still copies the subject's bytes to read a few of
  them. With the loops no longer quadratic that is the largest cost left in
  `gsub`, and is why it is still some fifteen times CRuby.
- `sort` with a block, and `s[i]` on a String with multi-byte characters,
  are as PRD 0073 left them.

## Check

`bench/string_loops.rb`, before and after and against `ruby --yjit`, in the
pull request. Three rows in `crates/spinel-vm/tests/eval.txt` for the walks —
empty matches, lookarounds, limits, captures, and what `$~` is afterwards —
measured on ruby 4.0.6.
