# PRD 0026 — argument evaluation order: a splat expands before what follows it

Issue: [#160](https://github.com/ar4mirez/spinel/issues/160) · Phase 1 · `area:engine`

## Objective

Ruby evaluates arguments left to right, and a splat is *expanded* at the point
it is written — before anything to its right is evaluated. Spinel evaluated
every argument onto the stack, block included, and expanded splats in
`expand_splats` when the call was made. Anything to the right that changed the
array ran first, and the splat saw the changed one:

```ruby
def m(*args, &block) = [args, block]
args = [1, nil]
m(*args, &args.pop)       # ruby [[1, nil], nil]   spinel [[1], nil]
```

Two examples in `language/send_spec.rb`, tagged in
`spec/tags/language/send_tags.txt`.

## Baseline

Measured on this branch at d71b2de, after #164 and #161.

| | |
|---|---|
| Rust tests | 285 passing · 0 failing |
| ruby/spec corpus | 25,624 examples · 1,606 passed · 0 failed |
| `verify-passes.rb` | 920 re-run on ruby 4.0.6, all agree |
| tags naming #160 | 2, both in `language/send_tags.txt` |

## Decisions

### The issue names the block argument; the bug is any argument

The title and both tags are about `&args.pop`, and the first thing this slice
did was ask whether the block is special. It is not:

```ruby
def m(*a) = a
x = [1, 2]; m(*x, x.pop)             # ruby [1, 2, 2]   spinel [1, 2]
x = [1, 2]; m(*x, x.push(9).size)    # ruby [1, 2, 3]   spinel [1, 2, 9, 3]
```

Three shapes rather than two, and a fix aimed only at the block argument would
have left the other two wrong while both tags came off. Measuring the family
before choosing a design is also what made the design smaller.

### A snapshot at the splat, not a new calling convention

The issue offers three designs and calls the third — build the argument list
into an `Array` at splatted sites, as CRuby does — "probably right". All three
change the calling convention, which is what the issue means by "not a one-line
fix": `Insn::Send` carries a *static* argument count, and expanding at the point
the splat is written makes that count dynamic.

None of that is necessary. The count only has to be static; the *contents* are
what arrive late. `Insn::CaptureSplat` replaces the splatted array with a copy
of its elements at the moment the splat is written, so the late expansion sees
exactly what an early one would have. The argument count stays static, `Send` is
untouched, and every one of the three shapes above is fixed by the same
instruction.

It is not a cheaper approximation of the issue's option 3 — it is the same
observable semantics. `expand_splats` already copies elements into a fresh
`Vec`; the only thing that changes is *when* they are read.

### It is emitted only where something can outrun it

An extra instruction on every splatted call would be a real cost for the
overwhelming majority, which are `m(*args)` with nothing to the right. The
issue's second requirement — "a non-splatted call site is unchanged" — is the
weaker half of what this needs, so the rule is: emit the capture only when
something written after the splat can run Ruby.

A literal block is not such a thing. `{ }` and `do end` build a `Proc` from a
child `Iseq` at call time and run no user code; `&expr` is a pass whose
expression is evaluated right there, which is the shape the issue was filed for.
`only_a_splat_something_can_outrun_is_captured` in `tests/bytecode.rs` asserts
the instruction count for seven call shapes, including the two that must have
none.

## Plan

1. Measure the family, not just the reported shape. ✅
2. `Insn::CaptureSplat`, and its entry in the stack-effect table. ✅
3. Emit it only when `mutable_after` says something can run Ruby. ✅
4. Oracle rows for all three shapes and for what must not change. ✅
5. A bytecode test that the instruction stays off the paths that cannot want it. ✅
6. Both tags removed, `send_tags.txt` deleted. ✅

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| corpus passed | 1,606 | **1,608** |
| corpus failed | 0 | **0** |
| `verify-passes.rb` | 920 agree | **922 agree** |
| tags naming #160 | 2 | **0** |

The two examples the issue names now pass rather than being skipped. `+2` is the
whole visible delta because `send_spec.rb`'s other 71 examples are blocked
behind `LangSendSpecs`, whose fixture needs attribute assignment — the same
blocker #164 surfaced.

### The definition of done

- [x] Both `send_spec.rb` examples pass, and their entries have left
      `spec/tags/language/send_tags.txt`, which is deleted
- [x] A non-splatted call site is unchanged — same instructions, same static
      count, asserted in `tests/bytecode.rs`
- [x] `language/{send,block,yield,def}_spec.rb` do not regress: 7, 91, 33, 38
      passing, 0 failing

### Eleven oracle rows, and what they are for

`tests/eval.txt` gained eleven rows measured by `scripts/eval-oracle.rb` against
ruby 4.0.6. Eight are cases where the two evaluation orders give different
answers; three are cases that must stay the same — `m(*x)` with nothing after
it, and a splat of a non-`Array`, which is passed through and must not be
copied into something else.

### Left for later

- **A splat of a non-`Array` does not call `to_a`.** Ruby does; Spinel passes
  the value through. Visible only for an object that defines `to_a` and is not
  an `Array`, and it predates this slice — `expand_splats` has always done this.
  Not folded in here because it is a conversion-protocol question rather than an
  ordering one.
