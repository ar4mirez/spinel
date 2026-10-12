# PRD 0072 — the rest of `Regexp` and `MatchData`, short of encodings

Issue: [#33](https://github.com/ar4mirez/spinel/issues/33) (most of it; the
issue stays open for what is named under *Not done*) · Phase 2 ·
`area:core-lib`, `area:engine`

## Objective

`core/regexp`, `core/matchdata` and `language/regexp` stood at 352 of 699.
Past the encoding modifiers, almost everything blocked was a method that did
not exist.

## Baseline

Corpus 18774 passed, 0 failed, after `Integer#round(half:)`.

## Design

**Two readings from the engine, and the rest in Ruby.** A MatchData already
held the byte offsets of every group, and a compiled pattern its table of
group names. `MatchData#__offsets__` and `Regexp#__name_table__` hand those
over; nothing else here is a primitive.

**MatchData.** `byteoffset`, `bytebegin`, `byteend`; `offset`, `begin` and
`end` in characters, which is how many lie before the byte; `match` and
`match_length`; `named_captures`, `deconstruct` and `deconstruct_keys`, so a
match can be the subject of `in {year:, month:}`; `values_at` with names and
Ranges; `[]` with a Range. A name that labels several groups means the last
of them that took part. A negative index counts back from the last group and
never reaches the whole match: `m[-m.size]` is nil. Measured.

**Regexp.** `names`, `named_captures`, `casefold?`, `encoding`,
`fixed_encoding?`, `~`; and on the class `compile`, `union`, `try_convert`,
`last_match`, `linear_time?`, `timeout` and `timeout=`. `Regexp.new` takes a
String of flags (`"imx"`), what converts with `to_str`, and reports a bad
pattern as CRuby does, naming it: `premature end of char-class: /[/`. A
Symbol is matched as its name.

**A source's encoding.** US-ASCII when nothing in it is past ASCII — not a
character, and not a `\u` escape naming one — and UTF-8 otherwise.
`fixed_encoding?` is that same question.

## Found on the way: the core library was overwriting `$~`

`$~` is one slot per heap here, where Ruby gives each method frame its own.
So a core method written in Ruby that matches a regexp for its own purposes
left its match in the caller's `$~`:

```ruby
"abc" =~ /b(c)/
"1.5".to_f
$1   # ruby => "c"    spinel => nil
```

Everything that reads a number or a date out of a String did this — `to_f`,
`Float()`, `Rational()`, `Complex()`, `Time.new(string)`, and more — and most
of it arrived in the last few slices. `Regexp#__match__` is `match` that
leaves `$~` as it was, and the core library's own parsing uses it, or
`match?`, throughout. The methods whose job is to set `$~` (`String#sub`,
`=~`, `scan`) are untouched.

## Not done

- **The `n`, `e`, `s`, `u` modifiers** (77 examples) and a pattern carrying
  an encoding of its own. One `Regexp.union` example is tagged to #33 for it.
- The Onigmo dialect past what the engine has: `\g<>`, `\p{}`, conditional
  groups, a multi-codepoint `\u{}`.
- `Regexp.timeout` is kept and answered, not enforced.
- `linear_time?` is decided by whether the source has a backreference.
- `$~` is still one slot per heap. A program's own method that matches still
  changes its caller's; only the core library stopped.

## Check

Rows in `crates/spinel-vm/tests/eval.txt`, measured on ruby 4.0.6, one of
them pinning `$~` across the core methods above. The ruby/spec delta is in
the pull request.
