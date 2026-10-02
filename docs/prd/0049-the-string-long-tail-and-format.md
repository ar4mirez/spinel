# PRD 0049 — `String`'s long tail, and `format`

Issue: [#19](https://github.com/ar4mirez/spinel/issues/19) · Phase 2 ·
`area:core-lib`

## Objective

The second half of #19, on PRD 0048's mutable, encoded String: the methods
`core/string/` was blocked on, written in Ruby over the byte primitives, and
`format`/`sprintf`/`String#%`.

## Baseline

`core/string` 271 / 1905 after PRD 0048, its blocked list led by `encode`,
`unpack`/`pack`, `to_i`, `split`, `gsub`, `%`, `sub`, `chomp`. Corpus 5328,
0 failed.

## What this slice writes

Ruby, in `core/string.rb`, checked case by case against CRuby with
differential scripts before each spec run:

- **Search and slicing:** `index`/`rindex` with offsets and Regexps (and
  `$~` reset), `byteindex`/`byterindex` with the character-boundary check,
  `start_with?`/`end_with?` with Regexps, `partition`/`rpartition` (the
  middle is the pattern itself, in its own encoding).
- **Trimming and padding:** the `strip` family (whitespace and NUL; an invalid
  character where stripping stops is CRuby's error, a different class on each
  side), `chomp`/`chop` and their bang forms, `center`/`ljust`/`rjust`.
- **Splitting:** `split`, ported from CRuby's `rb_str_split_m` so its limit,
  awk-mode, empty-match and capture rules come with it; `lines`/`each_line`
  with paragraph mode and `chomp:`.
- **Substitution:** `scan`, `sub`/`gsub` and their bang forms — String, Hash
  and block replacements; `\0`-`\9`, `\&`, `` \` ``, `\'`, `\\`, `\+` and
  `\k<name>`; `$~` for the block; a snapshot so a block that changes the
  receiver does not change the answer, and `string modified` in a bang form.
  `Regexp.escape`.
- **Character sets:** `tr`, `tr_s`, `delete`, `squeeze`, `count`, with ranges,
  `^` negation and intersecting sets.
- **Case:** `upcase`/`downcase`/`swapcase`/`capitalize` and bang forms, full
  Unicode mapping (`"ß".upcase` is `"SS"`, `"ß".capitalize` is `"Ss"`, `ǆ`
  titlecases to `ǅ`), `:ascii`, `:turkic`, `:fold` and CRuby's option rules;
  `casecmp`/`casecmp?`.
- **Numbers and characters:** `to_i` (bases 2-36, prefixes, underscores),
  `hex`, `oct`, `chr`, `Integer#chr` (BINARY, US-ASCII, UTF-8; another
  encoding's codepoints are refused), `succ`/`next` and `upto`.
- **`format`:** CRuby's `sprintf.c` in Ruby — flags, width, precision, `*`,
  `%1$`, `%<name>` and `%{name}`, the two's-complement `..f01` form for
  negative numbers in base 2, 8 and 16, and every error message.

## Primitives

Four, each where Ruby cannot reasonably go: `__byte_index__`/`__byte_rindex__`
(substring search on bytes), `__case_map__` (Unicode case mapping, which is
Rust's — Unicode's — tables), `Float#__format__` (C's digit generation for
`%f`/`%e`/`%g`), and `succ`. `succ` is a port of CRuby's `str_succ`: it steps
within runs of Unicode letters or decimal digits, so its character classes —
761 alphabetic and 72 digit ranges, Unicode 17 — are generated from CRuby by
`scripts/encoding-table.rb` and checked by the `encoding oracle` job. In Ruby
it walked ~37,000 steps for one spec example in 16 seconds.

## Found on the way

- **`Integer#==` with an object** asks the object, as CRuby's `num_equal`
  does, so a class's own `==` decides `5 == obj`. The fast path answered false.
- **`Enumerable#include?`** called `argument == element`; CRuby calls
  `element == argument`.
- **`max_by`/`minmax_by`** kept the last of a tie; the first wins.
- **`Range#include?` on Strings** walks `upto` after a `to_str` conversion.
- **`Proc#to_s`** shows where the block was written and `(lambda)`.
- **`Kernel#inspect`** skips names `instance_variables_to_inspect` lists that
  the object does not have.
- `Float` has no `to_i`, `floor`, `*` or `/` yet (#18), so `format` truncates
  through its own primitive and has no NaN/Infinity branch: only immediate
  floats exist, and none is either.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/string` | 271 / 1905 | **988 / 1905** |
| `core/kernel` | 286 / 1277 | **289 / 1277** |
| `core/enumerable` | 351 / 446 | **363 / 446** |
| `core/range` | 137 / 468 | **155 / 468** |
| **corpus** | 5328 | **6108** |

0 failed; all passes re-run on ruby 4.0.7 agree.

## What is left in #19

`encode` (transcoding), and `pack`/`unpack` — together about 250 examples —
and the mspec helpers (`mock`, `be_computed_by`; #145) behind about 170 more.
`encode` among UTF-8, US-ASCII and BINARY, with `invalid:`/`undef:`/
`replace:`, and the byte-format directives of `pack`/`unpack` are the next
slice.
