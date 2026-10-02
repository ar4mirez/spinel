# PRD 0048 — Strings with an encoding, and strings that can change

Issue: [#19](https://github.com/ar4mirez/spinel/issues/19) · Phase 2 ·
`area:core-lib`

## Objective

The first half of #19. A String had no encoding and could not change: its
bytes lived inline in a heap cell whose size was fixed at allocation, so there
was no `<<`, `replace`, `[]=` or `force_encoding`, and `"é".b` did not exist.
This slice gives a String an encoding and mutable bytes, adds `Encoding`, and
writes the methods that sit directly on those. The long tail — `split`,
`sub`/`gsub`, `%`, case mapping, `encode`, `pack`/`unpack` — is the second
half.

## Baseline

`core/string` 70 / 1905, `core/encoding` 0 / 319 (129 examples blocked on the
constant `Encoding` not existing). Corpus 5011, 0 failed.

## Design

**A String is three slots: `[buffer, bytesize, encoding]`** (`strings.rs`).
`buffer` is a classless byte object whose length is the capacity, so a String
grows by replacing its buffer while its own address never moves — `Array`'s
indirection, for `Array`'s reason. Every in-place change is one primitive,
`splice(start, len, bytes)`. `dup` copies the buffer; equality and `hash`
follow CRuby's comparable-encodings rule, so `"a" == "a".b` and
`"é" != "é".b`.

**The VM knows an encoding only by index.** The list is CRuby's: 103
encodings, their aliases, their two flags and the 211 constants naming them,
generated into `encoding_table.rs` by `scripts/encoding-table.rb` from a real
Ruby and checked by a new CI job, `encoding oracle`. Index 0, 1 and 2 are
BINARY, UTF-8 and US-ASCII, CRuby's own order. Building the `Encoding`
objects and constants is one primitive at boot: done as a Ruby loop it added
16ms to every heap's boot, which the spec harness pays per example.

**Characters are walked for three encodings and refused for the rest.**
UTF-8 (an invalid byte is one character, as CRuby counts), US-ASCII and
BINARY. A character operation on any other encoding is `Unknowable` — the
example is blocked — rather than counting bytes and calling them characters.
Every encoding has its names, flags and `Encoding.compatible?` answer.

**Literals take their file's encoding.** Prism's `encoding:` magic comment is
read into the `SourceMap`, so every scope compiled from the file sees it; a
literal is in the encoding an escape forced (`\u` → UTF-8, a high `\x` →
BINARY) or the source encoding. ruby/spec's files are mostly
`# -*- encoding: us-ascii -*-`, which is why `"a".encoding` there is
US-ASCII. The frozen-literal table is keyed by bytes and encoding.
`__ENCODING__` compiles.

**Ruby on primitives.** `core/encoding.rb` is `Encoding`: `find` (ASCII
case-insensitive, one table built on first use), `list`, `name_list`,
`aliases`, defaults, and `compatible?` ported line for line from CRuby's
`enc_compatible_latter`. `core/string.rb` adds `encoding`, `force_encoding`,
`b`, `valid_encoding?`, `ascii_only?`, the byte methods, `[]` in all its forms,
`[]=`, `slice!`, `insert`, `replace`, `clear`, `<<` (with codepoints),
`concat`, `prepend`, `chars`, `reverse`, `ord`, and CRuby's `inspect`
escaping. `String.new` now allocates an empty BINARY string and runs
`initialize`, which also makes a String subclass's own `initialize` run.

## Found on the way

- **Unbounded recursion in `inspect`.** `Array#inspect`, `Hash#inspect` and
  `Enumerator::Product#inspect` had no guard, so a structure holding itself
  looped forever — on a non-recursive interpreter that is a hang, not a
  `SystemStackError`. One spec running for the first time took the corpus
  from 170s to over 1000s. All three now use `Kernel.__inspect_guard__` and
  answer `[...]`/`{...}`; `Array#join` raises `recursive array join`.
- **Quadratic builders.** `Array#inspect`, `Array#join`, `Hash#inspect` and
  `Integer#to_s` built with `+`, copying the whole result per part; they append
  into one buffer now.
- **Encodings of derived strings.** `MatchData` pieces and `$&`, `` $` ``,
  `$'`, `$1` keep the subject's encoding; numbers' `to_s` and `Symbol#to_s` of
  an ASCII name are US-ASCII; `Kernel#to_s` and an anonymous class's are BINARY.
  Matching a regexp against bytes invalid in their encoding raises CRuby's
  `ArgumentError`.
- **`*_methods(false)`** lists the singleton class's methods *and* the
  object's own class's, so `Class.private_methods(false)` has `initialize`.
- **`scripts/verify-passes.rb` dropped encoding comments** on the theory that
  the corpus was UTF-8 throughout. Once literals honour them, a replay without
  `# -*- encoding: us-ascii -*-` was a different program, and 22 passes looked
  false. It now carries them over as it does `frozen_string_literal`, and
  scrubs a fixture in another encoding before scanning it for `require`s.

## Cost

The corpus runs in 160s, against 170s before this slice: the boot primitive
and the fixes above more than pay for a String's extra indirection.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/string` | 70 / 1905 | **271 / 1905** |
| `core/encoding` | 0 / 319 | **64 / 319** |
| `core/symbol` | 22 / 159 | **24 / 159** |
| **corpus** | 5011 | **5328** |

0 failed; all passes re-run on ruby 4.0.7 agree. One tag: `` Kernel#` `` is
#223's.

## What is left for the second half

The blocked list for `core/string` after this slice is led by mspec's `mock`
(#145), `unpack`/`pack`, `to_i`, `encode`, `split`, `%`, `gsub`/`sub`,
`chomp` — the long tail, which is Ruby over these primitives.
