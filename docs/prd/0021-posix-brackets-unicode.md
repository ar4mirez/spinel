# PRD 0021 — `[[:blank:]]`, and an exhaustive audit of the POSIX brackets

Issue: [#178](https://github.com/ar4mirez/spinel/issues/178) · Phase 1 · `area:engine`

## Objective

Make `[[:blank:]]` match the Unicode blank set, and — the larger half of the
issue — audit the other thirteen POSIX brackets against CRuby *by measurement
rather than by reading a table*, so that whatever is still wrong is known, named
and bounded instead of merely unnoticed.

`spec/tags/language/regexp/character_classes_tags.txt` skipped one example:

```ruby
"\u{1680}".match(/[[:blank:]]/).to_a.should == ["\u{1680}"]
```

`spinel-regex` answered `nil`: its `blank` was `' '` and `'\t'`, ASCII only.

## Baseline

Measured on `main` at 7e7c24a, before any change.

| | |
|---|---|
| Rust tests | 254 passing · 0 failing |
| ruby/spec | 3835 files · 25624 examples · 1291 passed · 0 failed · 22472 blocked · 1861 skipped |
| `language/regexp/` | 257 examples · 151 passed · 0 failed · 102 blocked · 4 skipped |
| `character_classes_spec.rb` | 128 examples · 105 passed · 3 skipped |
| POSIX brackets checked against CRuby | none, beyond the probe corpus in `oracle.txt` |

## Decisions

### The audit is exhaustive, not sampled

The issue asked for the brackets to be measured "exhaustively over the codepoints
that matter rather than sampled". Every scalar value is what matters: the bracket
sets are dense in places nobody probes by hand, and `[[:punct:]]` is the proof —
it agrees with CRuby on every character in `oracle.txt`'s thirty probe subjects
and disagrees on 962,009 codepoints.

`scripts/regexp-oracle.rb` grew a second table, `crates/spinel-regex/tests/posix.txt`:
one line per bracket, its membership over `0..0x10FFFF` as hex ranges. 25 lines,
48 KB, 5,475 ranges. Surrogates are excluded — Ruby cannot build a `String` from
one and Rust's `char` cannot hold one, so neither engine has an answer to compare.

Generation and checking were folded into the existing `--generate` / `--check`
rather than given their own flags, so CI's existing `regexp oracle` job covers
the new table with no workflow change.

### The replay is a unit test, not an integration test

`posix_matches` is the thing under audit, and it is private. Going through the
public API would mean compiling a pattern and running the backtracking machine
1.1 million times per bracket for the same answer. The replay therefore lives in
`crates/spinel-regex/src/exec.rs` as `mod posix_oracle`, calling the predicate
directly: 15.6 million calls, 0.4s in debug.

### `blank` is written out, not derived from a `char` predicate

Onigmo's `blank` is tab plus every `Zs`. The near miss it replaces is
`char::is_whitespace`, which is `Zs` *plus* the line separators — so it answers
true for `\n`, and `character_classes_spec.rb` has four examples that assert
`blank` rejects exactly those. Eight ranges, written out, measured against
`posix.txt`'s own `blank` line.

### Six brackets are named divergences, not silent ones

The audit found seven brackets wrong, not one. Six survive this slice:

| bracket | codepoints wrong | engine uses | Onigmo means |
|---|---|---|---|
| `[[:digit:]]` | 1,154 | `is_numeric` — `Nd`+`Nl`+`No` | `Nd` only |
| `[[:alnum:]]` | 915 | `is_alphanumeric` | `Alphabetic` + `Nd` |
| `[[:print:]]` | 814,732 | `!is_control` | assigned, minus `C*` |
| `[[:graph:]]` | 814,730 | `!is_control && !is_whitespace` | `print` minus space |
| `[[:word:]]` | 2,089 | `is_alphanumeric \|\| '_'` | `Alphabetic` + `M*` + `Nd` + `Pc` |
| `[[:punct:]]` | 962,009 | ASCII punct + "not anything else" | `P*` |

All six are one failure: `std` exposes no general categories, so `Nd`, `M*`,
`Pc`, `P*` and "assigned" are not reachable from `char`'s predicates. Closing
them needs a table, and where that table comes from — embed the measured ranges,
take a Unicode-tables dependency, or generate Rust from the oracle at build time
— is an owner decision about dependencies and about whether the oracle may also
be the implementation. That is [#180](https://github.com/ar4mirez/spinel/issues/180),
filed rather than guessed at, exactly as the issue's definition of done permits
("named in `KNOWN_DIVERGENCES` with its own issue").

The list is a `KNOWN_DIVERGENCES` const carrying each bracket's exact wrong-count,
and one `assert_eq!` on the whole list. That single assertion catches all three
ways such a list rots: a bracket that newly disagrees, a bracket that was fixed
and not deleted, and a count that drifted because Unicode moved underneath it.

## Plan

1. Add the POSIX table to `scripts/regexp-oracle.rb`; fold it into `--generate`
   and `--check`. — **done**
2. Generate `crates/spinel-regex/tests/posix.txt`. — **done**
3. Add the exhaustive replay as a unit test in `exec.rs`. — **done**
4. Fix `Posix::Blank` to tab + `Zs`. — **done**
5. Delete `spec/tags/language/regexp/character_classes_tags.txt`. — **done**
6. File the follow-up issue; bound the remaining six against it. — **done**, #180

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| `character_classes_spec.rb` | 105 passed · 3 skipped | **106 passed** · 2 skipped |
| `language/regexp/` | 151 passed · 0 failed · 4 skipped | **152 passed** · 0 failed · 3 skipped |
| whole corpus | 1291 passed · 1861 skipped | **1292 passed** · 1860 skipped |

No example moved to `failed`, and no directory regressed.

`spec/tags/language/regexp/character_classes_tags.txt` is gone: the tag it held
is paid off rather than reworded.

### The definition of done

- [x] `[[:blank:]]` matches the Unicode blank set, and the entry leaves
      `spec/tags/language/regexp/character_classes_tags.txt` — the file is deleted
- [x] Every other POSIX bracket audited against CRuby by the oracle rather than
      by reading a table — all fourteen, every scalar value. Seven agree exactly
      (`alpha`, `upper`, `lower`, `space`, `cntrl`, `xdigit`, `ascii`); `blank`
      is fixed; six are named in `KNOWN_DIVERGENCES` with #180
- [x] `language/regexp/` does not regress — 151 → 152 passed, 0 failed

### Verified by mutation, not just by green

Changing `("digit", 1154)` to `("digit", 9999)` fails the test. Worth recording
that the first restore *appeared* to fail too: `mv` preserves the backup's older
mtime, so Cargo did not rebuild and re-reported the mutant's failure. The check
is only honest after a `touch`.

### Left for later

#180 — the six brackets, and the dependency question behind them.
