# PRD 0050 — `encode`, `Encoding::Converter`, `pack` and `unpack`

Issue: [#19](https://github.com/ar4mirez/spinel/issues/19) · Phase 2 ·
`area:core-lib`

## Objective

The last of #19: transcoding — `String#encode`, `#encode!`, `#scrub` and
`Encoding::Converter` — and `Array#pack`/`String#unpack`/`#unpack1`. After
PRDs 0048 and 0049 these were the largest rows left in the corpus: `pack`
265 examples, `Encoding::Converter` 162, `encode` 148, `unpack` 82.

## Baseline

`core/string` 988 / 1905, `core/encoding` 64 / 319, `core/array/pack`
0 / 172. Corpus 6108, 0 failed.

## Transcoding

**The encodings whose mapping is arithmetic.** UTF-8, US-ASCII, BINARY,
ISO-8859-1, and UTF-16 and UTF-32 in both byte orders, plus the BOM-carrying
`UTF-16`/`UTF-32`. Converting from or to any other encoding is refused — the
example is blocked — because its table is not here; `Encoding::Converter.new`
and `search_convpath` refuse such a pair up front rather than guess a path.
The same module, `transcode.rs`, gives the VM character boundaries for UTF-16,
UTF-32 and ISO-8859-1, so `length`, `[]` and `inspect` work on transcoded
strings.

**One primitive, one step.** `__transcode__` converts from a byte offset until
the input ends or a character cannot be converted, and reports CRuby's three
kinds of invalid input exactly: a bad byte; a sequence *followed by* a byte
that cannot continue it, which is given back to be read again; and a
sequence the input ends inside (`incomplete`). UTF-8's validity is Unicode's
Table 3-7, so overlongs and surrogates are invalid where CRuby says they are.

**Ruby decides what an error means.** `core/transcode.rb` loops over steps:
`invalid:`/`undef:` with `replace:` (U+FFFD for a Unicode target, `?`
otherwise), `fallback:` (Hash, Proc or anything with `[]`), `xml: :text` and
`:attr` with hex character references for undefined characters, and the
newline options. Errors are CRuby's, with their attributes: a conversion that
goes through UTF-8 says so in the message and reports the UTF-8 step's
character and encodings, `readagain_bytes` is nil when there are none, and a
replacement that cannot be converted is — measured — `ConverterNotFoundError`.

**`Encoding::Converter`** is the same loop as a stream: `primitive_convert`
with partial input, destination offsets and size limits (output past the limit
waits for the next call), bytes read past an error kept for the next call
(`putback` returns them), `insert_output`, `convert`/`finish`,
`primitive_errinfo`, `last_error`, `convpath`, `search_convpath`,
`asciicompat_encoding`, and the flag constants.

## `pack` and `unpack`

`core/pack.rb` is `pack.c` in Ruby: every integer directive with `_`/`!` and
`<`/`>`, `U`, `w` (BER), the floats through `Float#__bits__` and
`Integer#__float_from_bits__`, `a`/`A`/`Z`, `B`/`b`/`H`/`h` (including CRuby's padding
formula for a count past the input, which differs between bits and nibbles),
`m` (base64, strict with `m0`), `M` (CRuby's `qpencode`, ported), `u`, and
`x`/`X`/`@` (whose missing count is 1 in `pack` and 0 in `unpack`), with
`buffer:` and `offset:`. The result is UTF-8 when `U` comes first, US-ASCII for
`m`/`M`/`u` or an empty format, and BINARY otherwise. `p` and `P` move raw
pointers and are refused; a float that needs the heap (NaN, Infinity, -0.0)
is refused until #18 boxes floats.

## Found on the way

- **A regexp against a UTF-16 or UTF-32 string** is CRuby's
  `Encoding::CompatibilityError`; a UTF-16 space is valid UTF-8 bytes, so it
  was matching.
- **`chomp`, `chop` and `inspect`** work by characters in an encoding whose
  newline is not the byte 0x0A, and `inspect` shows UTF-16/32 as `\uXXXX`.
- **The spec harness runs a shared group without `it_behaves_like`'s
  argument**, so three `unpack` examples build an empty format; tagged #145.
- **`Regexp.union`** checks patterns' encodings against each other, which
  needs regexps that carry one; two examples tagged #33.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/string` | 988 / 1905 | **1209 / 1905** |
| `core/encoding` | 64 / 319 | **174 / 319** |
| `core/array` | 796 / 1229 | **899 / 1229** |
| **corpus** | 6108 | **6546** |

0 failed; all passes re-run on ruby 4.0.7 agree. Five tags: three #145, two
#33.

## What this closes, and what it does not

This closes #19: `String` and `Encoding` with UTF-8, US-ASCII and binary, byte
and character indexing, negotiation, and broken sequences as ruby/spec
requires. What remains in `core/string/` and `core/encoding/` is mspec's
helpers (#145), table-driven encodings (EUC-JP, Shift_JIS, ISO-2022-JP and the
rest: a slice of their own if a program needs them), `Float` (#18), and
`Regexp` encodings (#33).
