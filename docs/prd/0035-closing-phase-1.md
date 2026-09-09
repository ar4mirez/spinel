# PRD 0035 — closing Phase 1: eight compiler refusals, a regexp numbering rule, and a comparison that lied

Issues: [#238](https://github.com/ar4mirez/spinel/issues/238) ·
[#219](https://github.com/ar4mirez/spinel/issues/219) ·
[#229](https://github.com/ar4mirez/spinel/issues/229) ·
[#226](https://github.com/ar4mirez/spinel/issues/226) ·
[#230](https://github.com/ar4mirez/spinel/issues/230) ·
[#228](https://github.com/ar4mirez/spinel/issues/228) ·
[#234](https://github.com/ar4mirez/spinel/issues/234) ·
[#237](https://github.com/ar4mirez/spinel/issues/237) ·
[#232](https://github.com/ar4mirez/spinel/issues/232) ·
[#217](https://github.com/ar4mirez/spinel/issues/217) ·
[#184](https://github.com/ar4mirez/spinel/issues/184) · Phase 1 · `area:engine`

## Objective

Every open issue in the Phase 1 milestone. Ten of them are small and one — #184
— is deliberately gated on a measurement that may say "do not write this".

They are taken in one pass because they are the whole remaining ceiling of the
milestone, and because eight of them are the same kind of work: a construct
Prism parses that `compile.rs` refuses under its own name. PRD 0033 established
that the ranking is only trustworthy when each refusal names exactly one
construct, so a pass that empties eight rows at once is also a pass that
re-checks that property eight times.

## Baseline

The committed `bench/spec-status.md`, which `scripts/spec-status.sh` regenerates
and CI diffs. `--platform=linux` is pinned there, and spec counts are
platform-dependent, so every number in this PRD is that platform.

| | |
|---|---|
| ruby/spec | 3835 files · 25624 examples · 2190 passed · 0 failed · 21515 blocked · 1919 skipped |
| `language/` | 80 files · 2735 examples · 1332 passed |

Refusal counts from `scripts/spec.sh --platform=linux --blocked=0 language`.
These are `language/` only; the issues quote corpus-wide numbers, which are
larger for the rows that also block `core/`.

| examples | refusal | issue |
|---:|---|---|
| 12 | a splat in `when` | #219 |
| 12 | a splat or keyword in an index target | #229 |
| 11 | a compound constant assignment | #226 |
| 8 | a flip-flop | #230 |
| 6 | an elided hash value | #228 |
| 4 | `alias` on a global variable | #234 |
| 4 | an interpolated method name here | #237 |
| 2 | a regexp that writes its named captures to locals | #232 |
| — | (a wrong answer, not a refusal) | #238 |
| — | (a wrong answer, not a refusal) | #217 |
| — | (gated on a benchmark) | #184 |

## Order, and why

1. **#238 first** — a silent wrong answer outranks a refusal. A refusal blocks
   an example, which the ranking counts; a wrong answer passes an example for
   the wrong reason, which nothing counts. It is also independent of the rest.
2. **The compiler refusals by descending count**, except where a dependency
   reorders them: #219, #229, #226, #230, #228, #234, #237.
3. **#217 before #232.** #232 has to know which group a name maps to, and #217
   is the issue that fixes the numbering it would map against. Doing #232 first
   would pin the wrong rule in a test.
4. **#184 last, and possibly not at all.** Its own definition of done makes the
   benchmark the gating item and says to close the issue rather than do the
   rewrite if no measurement asks for it.

## Plan

One commit per issue, each with its own measurement against CRuby 4.0.6 before
any code is written. A slice that does not move a spec count says so.

## Definition of done

- [ ] Each issue's construct compiles, or the issue is closed with the
      measurement that says it should not be written
- [ ] Each reason leaves `scripts/spec.sh --platform=linux --blocked=0`
- [ ] Rows in `crates/spinel-vm/tests/eval.txt` (or `crates/spinel-regex/tests/oracle.txt`),
      measured from CRuby, not written by hand
- [ ] `bench/spec-status.md` regenerated; the delta is the diff of that file
- [ ] `cargo test` green in debug, which is where `compile.rs`'s `debug_assert`
      stack-depth checks run

## Results

_Filled in as each slice lands._

## Validation

_Filled in after the slices._
