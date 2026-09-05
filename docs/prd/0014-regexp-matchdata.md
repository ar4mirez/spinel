# PRD 0014 — Regex engine decision, `Regexp` and `MatchData` basics

Tracks [#14](https://github.com/ar4mirez/spinel/issues/14). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:L`, `area:engine`.

## Objective

Two things at once, because the second cannot start until the first is settled.

`docs/engine.md` carried an open question — bind Onigmo, write a Rust engine, or start with
`fancy-regex` and measure — marked "decide at the end of phase 1, because `language/regexp/`
and most of `core/string/` depend on it". This slice is that end. It records the decision
with the options it rejected and why, and then integrates the result: regexp literals,
`Regexp` and `MatchData`, `=~`, `$~` and the numbered capture globals, and `case`/`when`
with a regex.

At the baseline below, `language/regexp/` was **0 passed, 280 blocked of 283**, with 248
of those blocked on the single reason "a regexp is not compiled yet" — the largest
single-reason group left in the corpus.

### The decision, and how it was made

The question was answered by measurement, not by preference. `scripts/regexp-oracle.rb`
extracts every regexp literal `language/regexp/` uses (342 unique, plus 23 hand-added for
behaviour the corpus exercises only indirectly), runs each against a thirty-subject probe
corpus on a real CRuby, and records the answers. Replaying those through `fancy-regex`
with the fairest available translation gave:

```
338 patterns CRuby accepts
  281  agree
   16  fancy-regex rejects
   41  DIFFERENT answer          (12%)
```

Part of the 41 is reachable by translation — Ruby's `\w`/`\d`/`\s` are ASCII where Rust's
are Unicode, and its POSIX brackets are Unicode where Rust's are ASCII, an inversion in
both directions. The rest are properties of the match engine: `/(a*)*/` on `"a"` leaves
group 1 empty in Ruby and full in `fancy-regex`; `/(a|\2b|())*/` on `"ab"` matches two
characters in Ruby and one in `fancy-regex`. Twelve percent silent wrong answers is the
plausible-but-wrong answer the project refuses, and the only way to ship it would be a list
of patterns known to be wrong, which the conformance rule forbids.

Onigmo was rejected for cost rather than correctness: a C toolchain in every build, against
the cross-compilation story and `spinel build --compile`. The `spinel-regex` boundary is
narrow enough — compile, match at an offset, read capture offsets — that Onigmo stays a
drop-in replacement if the remaining dialect proves more expensive than the toolchain.
The full argument is in `docs/engine.md`, section "Regex".

## Non-goals

- **The whole Onigmo dialect.** `(?~)`, `\g<>`, conditional groups, `\K`, `\R`, `\X`,
  `\p{}` and `\k<name+1>` level specifiers are refused, not approximated. A refusal is
  `Error::Unsupported`, which the VM turns into `Error::Unknowable` and the harness reports
  as blocked. 23 examples in `language/regexp/` sit behind these; each names its construct
  in the blocked report, so the next slice is chosen from data.
- **Encodings.** `/n`, `/e`, `/s`, `/u` are refused for the same reason. Matching is
  byte-oriented over UTF-8. A `\xNN` above ASCII makes a pattern binary-encoded and is
  refused rather than read as a codepoint. 28 examples wait on the Encoding slice.
- **Regexp interpolation.** `/#{x}/` needs string interpolation, which is not compiled yet;
  9 examples. Not built here, because building it here would build half of another slice.
- **`Regexp.new` and `Regexp.escape`.** Both need `new` on a built-in class, which is #15.
  `spinel_regex::escape` exists and is tested; nothing calls it from Ruby yet.
- **Ordinary global variables.** `$~`, `$&`, `` $` ``, `$'` and `$1`..`$n` get their own
  instruction; every other global stays `Unsupported`, and the global table is its own slice.
- **`$~` scoping.** One slot per heap. Ruby scopes it per frame and per thread; the thread
  half has an example in `back-references_spec.rb` that stays blocked.

## What ships

### `crates/spinel-regex` — a new crate, no dependencies

- `parse.rs` — Ruby's dialect to an AST. Flags are *baked into* the nodes rather than
  carried at match time, which is what makes `(?i:a)b` mean what it says.
- `exec.rs` — AST to a program, and a backtracking machine: iterative over an explicit
  stack for the main flow, recursive only for lookaround and atomic groups, whose nesting
  is a property of the pattern rather than of the subject. A step budget turns a
  pathological pattern into `Error::Budget` rather than a hung spec run.
- `lib.rs` — `Regex`, `Flags`, `Captures`, `escape`.

Dialect implemented: literals and the escape set, character classes with ranges, negation,
POSIX brackets, nesting and `&&` intersection, `.`, the anchors including `\G`, capturing,
non-capturing, named and atomic groups, inline and scoped flags, alternation, greedy, lazy
and possessive quantifiers, backreferences by number and by name, and all four lookarounds.

Three behaviours were measured and would not have been guessed:

- `^` is a line anchor always, and does *not* match after a trailing final newline.
- After the exact `{n}` form, a following `?` or `+` is a **new quantifier**, not a
  laziness marker — `a{2}?` is `(a{2})?`. Only the comma-bearing forms take the suffix.
- Onigmo's empty check: an iteration that consumes nothing may still go round again if it
  *changed a capture*, and when it changes nothing the loop stops **with a cut**, discarding
  the body's remaining alternatives. That single rule is what makes `/(?:|a)*/` match `""`
  against `"aaa"` while `/(a|\2b|())*/` crosses an empty iteration and matches all of
  `"aaabbb"`.

### VM integration

- `Builtin::Regexp` and `Builtin::MatchData`, appended so every existing class id holds.
- `Literal::Regexp` and `Insn::LastMatch(MatchRef)`.
- A per-heap table of compiled patterns, plus a literal cache that **is** a GC root:
  Ruby answers the same object every time a literal is evaluated, and `regexp_spec.rb`
  checks it with `equal?`.
- Natives: `Regexp#=~ #match #match? #=== #source #options #to_s #inspect`;
  `String#=~ #match #match?`; `MatchData#[] #to_a #captures #pre_match #post_match
  #begin #end #size #length`. `match` and `match?` take the optional position argument,
  in characters, negative counting back from the end.
- `case`/`when` with a regex, via `Regexp#===`.

### The oracle

`scripts/regexp-oracle.rb --generate|--check|--survey` writes
`crates/spinel-regex/tests/oracle.txt`; `oracle.rs` replays every line on every
`cargo test`. `--survey` re-runs the engine-choice measurement so the decision can be
re-derived rather than believed.

### `spec/tags/`

`CLAUDE.md` names `spec/tags/` as where a skipped example's reason goes; nothing read it
before. `spec/tags/skip.txt` now exists and the harness loads it. It holds one entry, with
its reason and the work that closes it. It is not an expected-failure list — an example
there is reported *skipped*, never passed and never failed.

## Definition of done

- [x] Decision recorded in `docs/engine.md` with the rejected options and why.
- [x] `language/regexp/` newly passes: **0 → 152 of 283**, 0 failed.
- [x] `=~`, `$~`, `case`/`when` with a regex all work.
- [x] `language/` overall: **391 → 545 passing**, 0 failed.
- [x] Every pass verified against CRuby by `scripts/verify-passes.rb`: 545 agree.
- [x] The engine agrees with CRuby on 319 of 320 oracle patterns; the one divergence is
      named, explained, and printed on every test run.
- [x] `cargo test` green, including the whole-corpus harness test.

## Measurements

| | before | after |
|---|---|---|
| `language/regexp/` passed | 0 | 152 |
| `language/regexp/` blocked | 280 | 127 |
| `language/` passed | 391 | 545 |
| `language/` failed | 0 | 0 |

Top remaining blockers in `language/regexp/`, each attributed:

| examples | reason | slice |
|---|---|---|
| 28 | a regexp encoding modifier | Encoding |
| 10 | `Kernel#eval` | phase 2, string eval |
| 9 | fixture constant `LanguageSpecs` | fixture loading |
| 9 | regexp interpolation | string interpolation |
| 7 | `\g<>` subexpression call | later regex slice |
| 7 | `new` on a built-in class | [#15](https://github.com/ar4mirez/spinel/issues/15) |
| 3 | `\p{}` unicode property | Encoding |
| 3 | conditional group `(?(...))` | later regex slice |

## Known divergence

`/^(()|a|())*?$/` — Onigmo keeps a capture that an iteration set even after backtracking
out of that iteration, but only inside a loop: at the top level `(?:(a)x|ab)` resets group
1, measured. `spinel-regex` snapshots captures at every backtrack point, so it restores in
both places. Closing it means modelling Onigmo's capture-restore records rather than whole
snapshots, which is its own slice. Reachable only from a lazy repeat over empty
alternations. Recorded in `KNOWN_DIVERGENCES` in `crates/spinel-regex/tests/oracle.rs` and
in `spec/tags/skip.txt`, and printed on every test run.

## Deliberate shortcuts

Each marked `// ponytail:` in the source with its ceiling and upgrade path.

- Backtrack entries clone the capture and mark vectors. Onigmo tracks only the groups
  inside the loop; narrow it the same way if a pattern ever makes it the hot path.
- Lookbehind walks back one character at a time asking the body to finish at the anchor,
  rather than computing the body's possible widths.
- Simple one-to-one case folding. Full Unicode folding needs a table, and arrives with
  Encoding.
- `[[:punct:]]` is ASCII punctuation plus "not anything else", the shape of Unicode's `P*`
  without the table.
- `$~` is one slot per heap, not per frame and per thread.
