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

## Results

_Filled in as each slice lands._
