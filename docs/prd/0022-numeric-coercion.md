# PRD 0022 — `Numeric#coerce` and the coerce-then-retry path

Issue: [#179](https://github.com/ar4mirez/spinel/issues/179) · Phase 2 · `area:core-lib`

## Objective

Give Spinel the protocol Ruby's numeric operators actually implement. An operand
a numeric operator does not recognise is not an error: it is asked to `coerce`
itself, and the operator is retried on the pair it answers.

`spec/tags/core/float/comparison_tags.txt` skipped one example, and the issue
described `core/float.rb` as answering `nil` for everything non-`Numeric`.

## Baseline

Measured on `main` at 7e7c24a, in a worktree, before any change.

| | |
|---|---|
| Rust tests | 254 passing · 0 failing |
| ruby/spec | 25624 examples · 1291 passed · 0 failed · 1861 skipped |
| `core/float/` | 234 examples · 38 passed · 0 failed · 2 skipped |
| `core/integer/` | 526 examples · 60 passed · 0 failed · 4 skipped |
| `core/numeric/` | 132 examples · 8 passed · 0 failed · 1 skipped |
| `core/comparable/` | 54 examples · 0 passed · 54 blocked |
| `Numeric#coerce` | does not exist |
| `4.2 + obj` where obj has `coerce` | `NoMethodError` |

### One premise of the issue was already stale

The issue says `core/float.rb` answers `nil` for anything that is not a
`Numeric`. It does not: #167 had already put an ad-hoc `coerce` path inside
`Float#<=>`, and the tagged example passes without any of this work. Measured
first, so the slice did not spend itself re-fixing something already fixed.

What was actually missing was everything around it: `Numeric#coerce`, and the
retry path in the other ten operators.

## Decisions

### The protocol lives in `core/numeric.rb`, reached only after the fast path declines

`Insn::BinOp` answers fixnum and flonum pairs without dispatching, and falls back
to an ordinary send when it cannot. So a Ruby-level `Numeric#+` is only ever
entered with an operand the fast path could not use — which is exactly the
condition the coercion protocol is for. No Rust was needed for the protocol
itself, which is what `CLAUDE.md` asks for.

### Three rules, not one, because they disagree on purpose

Measured into `crates/spinel-vm/tests/coerce.txt` by `scripts/coerce-oracle.rb`:

| the operand | `+ - * / %` | `<=>` | `< <= > >=` |
|---|---|---|---|
| has no `coerce` | `TypeError: X can't be coerced into Y` | `nil` | `ArgumentError: comparison of Y with X failed` |
| `coerce` answers `nil` | `TypeError: coerce must return [x, y]` | `nil` | `ArgumentError` |
| `coerce` answers a non-array, or an array of length ≠ 2 | `TypeError: coerce must return [x, y]` | same | same |
| `coerce` raises | the operand's own error, propagated | same | same |

The `nil` row is the one no reading settles: a `coerce` answering `nil` is "no
opinion" to `<=>` and a hard error to `+`, while a `coerce` answering `"not an
array"` is a hard error to both. That is CRuby's `err` flag, and it governs only
the *absent* answer, never the malformed one.

### The retry is written as operator syntax, not `send`

The first attempt routed every retry through `pair[0].send(op, pair[1])`. That
cannot work here: Spinel has no `Float#+` *method* — arithmetic on two numbers
exists only inside `Insn::BinOp` — so the `send` came straight back into
`Numeric#+` and refused. Writing `pair[0] + pair[1]` compiles to the instruction,
whose fast path adds the pair; a non-numeric pair still falls through to a real
send, which is what makes `4.2 + obj` legitimately end in `String#+` when
`coerce` answered a pair of Strings.

### A numeric operand that the fast path declined is refused, not coerced

If both operands are numbers and `BinOp` still declined, the pair has no
representation here — an Integer wider than a fixnum, or a result outside flonum
range. Coercing would hand the same pair back and recurse forever. So
`__refuse_unrepresentable__` raises the very `NoMethodError` the VM raised before
this file existed, byte for byte, which keeps those examples reported *blocked*
rather than hanging or answering wrongly. Its `ponytail` comment names the
ceiling: the guard goes when bignums land.

### The naming rule for a refused operand is shared with `Comparable`

`rb_cmperr` names an immediate by its `inspect` and everything else by its class
— "comparison of Float with **nil** failed", but "comparison of Float with
**Object** failed", not `#<Object:0x...>`. `core/comparable.rb` had `other.inspect`
unconditionally, which is wrong for the second case, so the rule moved into
`Comparable#__operand_name__` and both callers use it.

### `Float()` is stood in for, and the gap is named

`Numeric#coerce` answers `[Float(other), Float(self)]`, and Spinel has no
`Kernel#Float` and no `String#to_f`. `to_f` stands in — right for every type the
VM has, wrong for `String`, and it cannot be made right by adding `String#to_f`
later because `":)".to_f` is `0.0` where `Float(":)")` raises. That is
[#181](https://github.com/ar4mirez/spinel/issues/181), with a tag and a
`ponytail` comment pointing at it.

## Plan

1. `core/numeric.rb`: `coerce`, the ten operators, the shared helpers. — **done**
2. `Integer#to_f` as `self * 1.0`; `<=>` moves to `Numeric`; the ad-hoc block
   leaves `Float#<=>`. — **done**
3. `Comparable#__cmp_failed__` / `__operand_name__`, measured. — **done**
4. `scripts/coerce-oracle.rb` + `coerce.txt` + `coerce.rs`; CI job. — **done**
5. Fix whatever the newly-running examples reveal. — **done**, see below
6. Delete the float tag; tag the one example #181 owns. — **done**

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| `core/float/` | 38 passed · 0 failed · 2 skipped | **42 passed** · 0 failed · 1 skipped |
| `core/integer/` | 60 passed · 0 failed · 4 skipped | **64 passed** · 0 failed · 5 skipped |
| `core/float/comparison_spec.rb` | 3 passed · 1 skipped | **4 passed** · 0 skipped |
| `core/numeric/`, `core/comparable/` | 8 / 0 passed | 8 / 0 passed — unchanged |
| whole corpus | 1292 passed · 0 failed | **1301 passed** · 0 failed |
| Rust tests | 255 passing | **256 passing** |

### A bug the unblocking revealed: `%` by zero

Making the operators run turned three examples from blocked into *failing*, which
is the point of unblocking. Two were one real VM bug: `float_op` checked for a
zero divisor on `/` but not on `%`, so `4.2 % 0` computed a NaN it then could not
represent. Measured — `4.2 / 0` is `Infinity` but `4.2 % 0` and `4.2 % 0.0` are
both `ZeroDivisionError` — and fixed in `interp.rs`, which is where the other
zero check already lived. `core/float/modulo_spec.rb` and
`core/integer/modulo_spec.rb` pass as a result.

The third was `1.coerce(":)")`, which is #181's.

### The definition of done

- [x] `Numeric#coerce`, answering `[Float(other), Float(self)]`, and `TypeError`
      when `other` cannot be made a Float — with the same-class case answering
      `[other, self]` untouched, which the issue did not mention and the oracle
      caught
- [x] The coerce-then-retry path in `+ - * / % ** <=> < <= > >=` on `Integer` and
      `Float` — via `Numeric`, which both inherit. `**` reaches the protocol but
      still refuses on a numeric pair: the operator has no implementation at all
      yet, which is a separate gap, marked `ponytail`
- [x] `<=>` answers `nil` where the others raise
- [x] `core/float/comparison_spec.rb` passes, and its entry leaves
      `spec/tags/core/float/comparison_tags.txt` — the file is deleted
- [x] The behaviour is measured from CRuby into an oracle table — 52 rows in
      `crates/spinel-vm/tests/coerce.txt`, covering `coerce` raising, answering
      the wrong types, and answering the wrong length, plus the naming rule for
      the operand in each error
- [x] `core/{integer,float,numeric}/` do not regress — all three gained or held,
      0 failed

### Verified by mutation, not just by green

Two mutations, each caught by `coerce.rs`:

- relational operators coercing with `strict = true` — the `ArgumentError` rows
  become `TypeError`;
- `coerce` dropping its same-class case — `1.coerce(2)` becomes `[2.0, 1.0]`.

### Left for later

- [#181](https://github.com/ar4mirez/spinel/issues/181) — `Kernel#Float` and
  `String#to_f`, the one tag this slice adds.
- `**` on a numeric pair, which needs the operator to exist at all.
- `4.2.send(:+, 2.0)` still refuses, because arithmetic on two numbers is an
  instruction and not a method. Unchanged by this slice, and invisible to the
  operator syntax; it becomes reachable the day a `Float#+` primitive exists.
