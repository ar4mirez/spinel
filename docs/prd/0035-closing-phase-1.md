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

`bench/spec-status.md` moved on exactly two rows, which is the delta this PRD
owes:

| | before | after |
|---|---:|---:|
| `language` | 1164 passed · 1235 blocked · 47% | 1202 passed · 1197 blocked · 49% |
| **total** | 2190 passed · 21515 blocked | 2228 passed · 21477 blocked |

`scripts/spec.sh --platform=linux language`, which includes the `regexp` and
`predefined` subdirectories the table lists separately: 1332 → 1370 passed, 0
failed throughout.

Per slice, and honestly — three of them moved no counter at all:

| issue | reason removed | examples gained | note |
|---|---|---:|---|
| #238 | — (a wrong answer) | 0 | 7 silent wrong answers fixed |
| #219 | a splat in `when` | +12 | |
| #229 | a splat or keyword in an index target | +12 | 2 of them were *failures* |
| #226 | a compound constant assignment | 0 | the 11 behind it are blocked on #39 |
| #228 | an elided hash value | +1 | 5 more are outside `language/` |
| #237 | an interpolated method name here | 0 | the 4 want `class_eval`/`ruby_exe` |
| #217 | — (a wrong answer) | 0 | numbering, which #232 needed |
| #232 | a regexp that writes its named captures to locals | +1 | |
| #234 | `alias` on a global variable | +4 | |
| #230 | a flip-flop | +8 | |
| #184 | — | 0 | closed by its own gate, below |

Eight refusal rows leave `scripts/spec.sh --blocked=0`. One reason was renamed
rather than removed: #229's row named a splat *and* a keyword and refused only
the splat, so what is left is `a keyword in an index target` — the read form,
since the assignment form is a syntax error in CRuby too.

## Validation

### What the measurements corrected

Three of the issues described a shape that measurement contradicted. Each is
worth recording, because in each case building what the issue described would
have produced a passing spec and a wrong engine.

**#230 said the flip-flop bit belongs on the iseq, "not on the frame".** It is
on the frame. Measured three ways: a flip-flop in a method resets on every call,
one in a block keeps its bit across the block's calls, and two procs made by two
calls of one method have separate bits. That is an ordinary local of the
enclosing scope, so the parser declares `%ff<offset>` and the compiler reads it
by name. An iseq slot would have passed all eight examples and shared state
between two procs that Ruby keeps apart.

**#229's reason named two constructs and refused one.** `h[*k] = 2` already
compiled; only the op-assign path refused. The keyword half was never a slice —
`h[:a, b: 1] = 2` is a syntax error in CRuby and the parser already says so.

**#228 expected `m(x:)` to be a second refusal site.** It is the same node.

### What #238 turned out to be

The issue reported one asymmetry in `==`. The same root — narrowing the integer
to an `f64` before comparing — produced **seven** wrong answers across `==`,
`!=`, `<`, `<=`, `>` and `>=`, in both directions across the fixnum boundary.
`ruby_eq`'s bignum arm and `wide_op`'s escape path both route through one exact
comparison now. Arithmetic still promotes to Float, which is Ruby's answer
there.

Found while measuring it and **not** fixed here: `Integer#<=>` is inexact for
the same reason, and it is inexact for *fixnums* too — `(2**54 + 1) <=> (2**54).to_f`
is 0 and should be 1. That one is a different layer: `<=>` is not a `BinOp`, so
it reaches `Numeric#<=>` in `core/numeric.rb`, which coerces both sides to
Float. Filed rather than folded in.

### #184: the benchmark answered, and the answer is no

The issue makes the benchmark the gating item and says, in as many words, that
if the clone cost cannot be shown to matter the right outcome is to close the
issue rather than do the rewrite. `bench/regex_captures.rs` already existed;
this ran it.

Holding the backtracking fixed and varying the group count, the clone is real:

| groups | per match | vs 1 group |
|---:|---:|---:|
| 1 | 5.04ms | 1.00x |
| 8 | 7.79ms | 1.55x |
| 32 | 10.33ms | 2.05x |

But on the patterns real code writes, against system `ruby --yjit`:

| pattern | groups | spinel | ruby | ratio | captures' share |
|---|---:|---:|---:|---:|---:|
| date | 3 | 110ns | 271ns | **0.4x** | 7% |
| log line | 9 | 14.99µs | 567ns | 26.4x | 28% |
| nested alternation | 3 | 1.06µs | 296ns | 3.6x | -7% |
| email-ish | 3 | 1.90µs | 339ns | 5.6x | -5% |

On three of the four, the whole capture machinery is at or below the noise. On
the fourth it is 28% of a number that is 26x off — and the restore-record
rewrite removes only part of that 28%. Deleting the capture cost entirely still
leaves 19x. So the measurement does not ask for this rewrite; it asks for a
different investigation, which is filed instead. #184 closed.

### Left for later

- `Integer#<=>` is inexact against a Float on both sides of the fixnum
  boundary. Filed.
- `spinel-regex` is ~26x slower than CRuby on a 9-group anchored pattern for
  reasons that are not capture cloning. Filed.
- `X ||= 1` on a constant this heap has never seen still refuses, under #39
  rather than under its own name: a fresh heap cannot tell "undefined" from
  "not required yet", and guessing would be a wrong answer wearing a passing
  spec.
- Constant reassignment still does not warn. `class.rs` marks it, plain
  `X = 2` shares it, and the specs asking for the warning are blocked on
  mspec's `complain`.
- `**=` is still refused, under `this compound assignment operator`: `**` is
  not one of the fast-path `BinOp`s.
