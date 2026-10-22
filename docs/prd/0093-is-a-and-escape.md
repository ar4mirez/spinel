# PRD 0093 — `is_a?` without `ancestors`, and `Regexp.escape` by bytes

Issue: [#299](https://github.com/ar4mirez/spinel/issues/299) · Phase 2 ·
`area:engine`, `area:core-lib`

## Objective

PRDs 0073 and 0074 made `gsub`, `split` and `sort` linear and took the
issue's four rows from seconds to tens of milliseconds. Measured again,
what was left in a `gsub` with a String pattern was not the loop over its
matches but two costs paid once a call, before the first match:

- `Kernel#is_a?` built the receiver's whole `ancestors` Array and searched
  it with a Ruby `Array#include?`, about 8µs when the answer was false.
  `gsub`, `split`, `String.__coerce__`, `Encoding.find` (so `force_encoding`
  and `String.new(encoding:)`) and `Module#===` each ask it.
- `Regexp.escape`, which a String pattern is escaped with on every call,
  walked characters through a `case` of sixteen String `when`s, about 32µs
  for a one-character pattern.

## Design

**`__reflect_kind_of__(object, mod)`**, a class-table read beside the other
`__reflect_*__` primitives: whether `mod` is among the ancestors of the
object's class, singleton included. `Classes::reaches` walks the chain in
place rather than collecting it. `is_a?` asks it and keeps its Ruby
`TypeError` for a `mod` that is not a module. `Module#<=` asks the same walk
through **`__reflect_reaches__(mod, other)`**, in place of two `ancestors`
Arrays, and so does `Module#include?`.

**`Regexp.escape` walks bytes where every byte below 0x80 is a character**:
BINARY, UTF-8, US-ASCII, or any ASCII-compatible String that is all ASCII.
A 128-entry table gives each metacharacter's escape, and the runs between
them are copied whole. Shift_JIS, Windows-31J, Big5 and GBK have trail bytes
like `\` and `[` below 0x80, so any other String is walked a character at a
time as before, against the same table.

## Not done

What is left per match is the interpreter's own call cost: a `yield` from
`Array#each` is about 0.7µs and a send to a Ruby method 0.3–0.5µs, and a
global `gsub` makes about six per match. That is Phase 5 and 6's (an
interpreter within 1.5× of CRuby's, then the JIT), not a String method's.

## Check

`bench/string_loops.rb` and the issue's four rows, before and after and
against `ruby --yjit`, in the pull request. A full corpus run before and
after: no example that passed fails, and `core/regexp/quote_spec.rb` passes
its EUC-JP example.
