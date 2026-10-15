# PRD 0084 — `strscan`

Issue: [#57](https://github.com/ar4mirez/spinel/issues/57) · Phase 3 ·
`area:gems`

## Objective

`require "strscan"` loads a `StringScanner` that behaves as CRuby's does, at
the version CRuby 4.0.6 ships (3.1.6). `library/stringscanner/` held 162
blocked examples, and parsers written in Ruby — `rdoc`, `erb`'s lexer, most
template and markup gems — scan with it.

## Design

**One primitive.** `Regexp#__match_at__(string, byte, anchored)` makes a
single attempt at that byte, or with `anchored` false searches from it, and
answers every group's byte offsets as a flat Array, leaving `$~` alone as the
C extension does. `spinel-regex` gained `match_at`, the single attempt, with
`\G` anchored at the byte.

**`lib/strscan.rb`, in Ruby,** following `strscan.c` function by function:
`strscan_do_scan`'s four switches give `scan`, `match?`, `skip`, `check`,
`scan_until`, `exist?`, `skip_until`, `check_until` and the two `_full`
forms; a String pattern is matched as bytes; positions are bytes; the last
match's groups, names, `pre_match` and `post_match` are kept as offsets.
Without `fixed_anchor:` a pattern sees the rest of the String as the whole of
it, so it is matched against that slice; with it, in the whole String.
`inspect` shows five bytes either side of the pointer, as CRuby's does.
`ponytail:` the slice is a copy per match, quadratic over a long String.

**`String#dump`,** which `inspect` needed and the core library lacked:
CRuby's `rb_str_dump`, with `\u` for UTF-8 characters, `\x` for any other
byte, and a `force_encoding` suffix for an encoding that is not
ASCII-compatible.

## Results

`library/stringscanner` 0 to **239** of 240, the last blocked on #19's
encodings; `String#dump` with it. Corpus 22,838 to **23,093** passing, 0
failed; compared example by example with `main`, 177 blocked examples pass
and none that passed fails.
