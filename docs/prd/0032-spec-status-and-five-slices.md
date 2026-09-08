# PRD 0032 — the progress bar, and five slices measured against it

Issues: [#147](https://github.com/ar4mirez/spinel/issues/147),
[#204](https://github.com/ar4mirez/spinel/issues/204),
[#215](https://github.com/ar4mirez/spinel/issues/215),
[#205](https://github.com/ar4mirez/spinel/issues/205),
[#180](https://github.com/ar4mirez/spinel/issues/180),
[#184](https://github.com/ar4mirez/spinel/issues/184) · Phase 1 ·
`area:infra`, `area:engine`

## Objective

Build the per-directory progress bar `README.md` has been promising, then land
four engine slices measured against it, and answer the fifth — a performance
rewrite — with a benchmark rather than with a hypothesis.

## Baseline

Measured on this branch at 7065511, before any change.

| | |
|---|---|
| ruby/spec | 3835 files · 25624 examples · 2150 passed · 0 failed · 21558 blocked · 1916 skipped |
| `language/` | 2735 examples · 1304 passed · 0 failed · 1361 blocked · 70 skipped |
| `language/regexp/` | 257 examples · 163 passed · 0 failed · 90 blocked · 4 skipped |
| `language/rescue_spec.rb` | 60 examples · 30 passed · 30 blocked |
| `language/class_spec.rb` | 45 examples · 15 passed · 29 blocked · 1 skipped |
| `back-references_spec.rb` | 20 examples · 10 passed · 8 blocked · 2 skipped |
| `bench/spec-status.md` | does not exist |
| `KNOWN_DIVERGENCES` | 6 entries, all POSIX brackets (#180) |

## The order, and why

`#147` is first because it is the instrument the other five report through: a
per-directory table is what turns "1304 passed" into a statement about which
directory moved. Its *table* is regenerated last, though — every engine slice
below staleness-checks it, so writing the numbers before the engine work would
mean writing them twice.

Then the three `size:S` engine slices, cheapest first and grouped by the crate
they touch, so a build is reused: `#204` (VM unwind), `#215` (compiler),
`#205` (`spinel-regex`, compile-time validation).

`#180` follows `#205` — same crate, and `#205` is the smaller of the two, so a
regression in the regex dialect has one suspect rather than two.

`#184` is last and is **not** assumed to be a code change. Its own definition of
done makes a benchmark the gating item and says, in as many words, that if the
clone cost cannot be shown to matter the right outcome is to close the issue.
So the benchmark is the deliverable; the rewrite happens only if the number asks
for it.

## Plan

1. **#147** — a `--by-directory` mode on the harness, a script that renders
   `bench/spec-status.md` from it, and a CI job that regenerates and diffs.
2. **#204** — `return` in a `class << obj` body unwinds to the enclosing method;
   a plain `class` body keeps its `LocalJumpError`.
3. **#215** — `rescue *classes` compiles, combines with a literal list, and
   raises `TypeError` for a non-`Module` where Ruby does. Measure the rule first.
4. **#205** — `\k<0>` and a numeric backreference beside a named group are
   `RegexpError` at compile time, with Ruby's wording.
5. **#180** — close the six POSIX brackets against real general-category data.
6. **#184** — write the regex benchmark. Rewrite only if it asks.
7. Regenerate `bench/spec-status.md` and reconcile it against `scripts/spec.sh`.

## Decisions

### The progress bar is rendered by the harness, not assembled by a script

`scripts/spec-status.sh` runs the harness with `--by-directory` and redirects it.
The table is printed by the run that counted it, so a row cannot disagree with
the total it is part of — and the script asserts the rows add up before it writes
anything, which is the reconciliation #147 asks for and which a mutation confirms
fires.

There is no `--check` mode. CI runs the script and then `git diff --exit-code`,
which is already a staleness check and cannot drift from the thing it checks.

Rows are the directory a spec file is in, capped at two components: `core/array`
and `language/regexp`, with `core/array/pack` folding into its parent. That is
the unit ruby/spec is organised in — 126 rows, one screen.

`README.md` lost its hand-typed pass counts in the same change. They were already
stale and rule 7 forbids them; the file now points at the table.

### The table has to name a platform, which CI found and the laptop could not

The first CI run failed the new staleness job, and the reason is the interesting
part: ruby/spec's `platform_is` guards are answered against the *host*, so the
same corpus reports a different split on `darwin` and on `linux` — 21,553 blocked
and 1,913 skipped here, 21,547 and 1,919 there. Six examples, enough to make a
committed file disagree with its own check forever.

Generated-and-diffed only works if the generator is deterministic across the
machines that run it, and this one was not. `--platform` pins it; the script
passes `linux`, because that is where the CI job runs, and the file says so in
its own header so nobody wonders why their laptop disagrees. A plain
`scripts/spec.sh` still answers for the machine you are on, which is what someone
debugging locally wants.

Worth noting that no amount of local testing would have found this: it needed two
platforms, which is what CI is.

### `return` in a `class << obj` body: the issue's second bullet was wrong

The issue says a plain `class` body's `return` "stays a `LocalJumpError`".
Measured on ruby 4.0.6, it is a **`SyntaxError`** — "Invalid return in
class/module body" — and Prism already refuses it, so Spinel agreed before this
slice and still does.

What was actually wrong is one line: a singleton body homed its `return` to
itself. It is transparent instead, inheriting its opener's home, which was
measured seven ways — written in the body, from a block, from a `proc`, through a
nested singleton body, and the three shapes that must *not* return from the
method. At the top level it inherits nothing (`home: 0`), because there is no
method to leave and Ruby raises `LocalJumpError` there — the one shape that
reaches the VM, since a `return` written directly in a top-level singleton body
does not parse.

### `rescue *classes` is one array literal per splat, not one per clause

Ruby tests a `rescue` list left to right and **stops at the first match**, so the
whole list cannot become one array: `rescue A, *rest` never evaluates `rest` when
`A` matched, and `rescue RuntimeError, *[42]` catches without the 42 ever being
a `TypeError`. Both measured.

So each splat becomes the one-element array literal `[*classes]` and one
`CheckMatchAny`, and stays one step in the existing straight line. The splat's
`to_a` conversion is therefore the array literal's own rather than a rule
invented here, which is what makes `*(RuntimeError)` and an object with `to_a`
both work without either being special-cased.

`when *values` is deliberately **not** fixed with it, and now says why in its own
refusal: `rescue` matches with an ancestor walk the interpreter can do inside one
instruction, `when` matches with `===`, which is a send and needs a frame. 12
examples, a different slice.

### The six POSIX brackets: generated from the oracle, not from a dependency

#180 left the choice open between embedding the measured ranges, taking a
Unicode-tables dependency, and generating Rust from the oracle. This is the
third.

The dependency is the one rejected on purpose: its Unicode version has to be kept
in step with the one CRuby was built against, and a mismatch surfaces as exactly
the quiet per-codepoint disagreement #178's audit existed to find. Generating
from `tests/posix.txt` keeps a measurement as the source of truth — that file is
produced from a live CRuby and re-checked against one by CI — and a `build.rs`
turns it into sorted range tables, so a lookup is a binary search over ~19 KB of
`.rodata` and costs nothing at boot. Boot cost matters: the harness boots one
heap per example.

Eight brackets stay on `char`'s predicates. They were measured as exact on all
1,114,112 scalar values, they are cheaper than a binary search, and replacing
working code with 30 KB of tables is not an improvement.

This makes the `posix_oracle` replay partly a tautology — six brackets are now
checked against the file they were generated from — and the test says so rather
than implying otherwise. The oracle that still bites is
`scripts/regexp-oracle.rb --check` against a live CRuby, which CI already runs.

**This was the owner decision #180 reserved.** It is cheap to reverse: the
alternative is one dependency and deleting `build.rs`.

### #184 is answered "no", by its own gating rule

The issue makes a benchmark the gating item and says that if the clone cost
cannot be shown to matter, the right outcome is to close it rather than do the
rewrite. `bench/regex_captures.rs` holds the backtracking fixed and varies the
group count, then measures the same real patterns with and without captures:

| | |
|---|---|
| 32 groups vs 1, identical walk | **2.06x** — not the `O(n)` the issue assumed |
| 8 groups vs 1, identical walk | 1.61x |
| whole capture machinery, 3-group patterns | **0-11%** of the match |
| whole capture machinery, 9-group `log line` | **31%** |
| that same pattern vs `ruby --yjit` | **27x slower** |

31% is the *entire* capture machinery on the worst real pattern, and a restore
record removes only part of it — against an engine that is off by 27x for
reasons the clone is not. The rewrite was not done. Both `ponytail` comments
stay, now citing the measurement so the next reader does not re-derive it.

The benchmark is the deliverable, and `scripts/bench.sh` exists at last:
`CLAUDE.md` and `docs/architecture.md` had been naming it for six phases.

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| `language/class_spec.rb` | 15 passed · 1 skipped | **16 passed** · 0 skipped |
| `language/rescue_spec.rb` | 30 passed · 30 blocked | **32 passed** · 28 blocked |
| `back-references_spec.rb` | 10 passed · 2 skipped | **12 passed** · 0 skipped |
| `character_classes_spec.rb` | 106 passed | **115 passed** |
| `language/regexp/` | 163 passed · 0 failed · 4 skipped | **168 passed** · 0 failed · 2 skipped |
| `language/` | 1304 passed · 0 failed | **1312 passed** · 0 failed |
| whole corpus | 2150 passed · 0 failed | **2158 passed** · 0 failed |
| `KNOWN_DIVERGENCES` | 6 entries | **empty** |
| Rust tests | 30 targets green | 30 targets green |

`language/regexp/` gains five rather than two because #205's oracle line found a
sixth bug on the way past — see below.

### The definition of done

- [x] **#147** — `bench/spec-status.md`, 126 rows, regenerated by
      `scripts/spec-status.sh`, CI job diffs it, totals reconcile with
      `scripts/spec.sh` by construction and by an assertion in the script
- [x] **#204** — `return` in `class << obj` leaves the enclosing method; the
      plain-class case was already right, for a different reason than the issue
      gave; rows in `eval.txt`, the two shapes that raise or do not parse in
      `eval.rs`
- [x] **#215** — `rescue *classes` compiles and combines with a literal list;
      `TypeError` where Ruby raises it; the reason has left
      `scripts/spec.sh --blocked=0`; `when *values` explicitly left with the
      reason named; 11 rows in `eval.txt`
- [x] **#205** — both patterns are `RegexpError` at compile time with Ruby's
      wording; 12 patterns joined `scripts/regexp-oracle.rb`; both examples pass
- [x] **#180** — all six brackets closed, `KNOWN_DIVERGENCES` empty,
      `language/regexp/` did not regress
- [x] **#184** — benchmark written and answered; the rewrite is not done and the
      issue should be closed, which is what its own text asks for

### Two bugs found by the new checks

Neither was being looked for; both were found by a table added for something
else, which is the argument for the tables.

1. **`[]` in a character class.** Added as a *guard* — a pattern #205's new
   check must not reject — it turned out the engine had it wrong anyway: inside a
   class, ``-`` are octal escapes, so `/[]/` matches U+0001 and not `"1"`.
   Fixed in the same slice, one line: `escaped_char` handled ` ` and stopped.
   This is most of the `character_classes_spec.rb` gain.
2. **Unnamed groups are still numbered beside a named one.**
   `/(a)(?<b>b)/.match("ab").to_a` is `["ab", "b"]` in Ruby and `["ab", "a", "b"]`
   here. It is the reason #205's rule exists, which is why #205's check had to be
   written against the named-group table rather than falling out of the
   numbering. Filed as
   [#217](https://github.com/ar4mirez/spinel/issues/217) rather than guessed at.

### Verified by mutation, not just by green

Every slice was checked by breaking it on purpose and confirming a test failed:

| mutation | what caught it |
|---|---|
| singleton body homes to itself again | `eval.txt` replay **and** the `LocalJumpError` test |
| `CheckMatchAny` does not stop at the first hit | `eval.txt` — after two rows were added, because the first nine did not pin it |
| `\k<0>` check removed | `oracle.txt` replay |
| named/numeric check disabled | `oracle.txt` replay |
| class octal escape reverted | `oracle.txt` replay |
| `digit` back to `is_numeric` | `posix_oracle` |
| one `punct` range dropped in `build.rs` | `posix_oracle` |
| a directory row corrupted | `spec-status.sh`'s reconciliation |

The second row is worth keeping: the first nine `rescue` rows all passed against
a mutant that never stopped scanning, because every one of them short-circuited
in the *straight line* instead of inside the array. `rescue *[RuntimeError, 42]`
is the row that pins it, and it only exists because the mutation was run.

### Left for later

- **#217**, the capture numbering underneath #205's rule.
- **`when *values`**, 12 examples, needs a compiled loop rather than an opcode.
- The restore-record rewrite, if a pattern ever makes the clone the hot path.
  `bench/regex_captures.rs` is now the thing that would show it.
