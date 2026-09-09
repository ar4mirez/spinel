# PRD 0034 — argument forwarding, safe navigation, bignum, and symbol interpolation

Issues: [#221](https://github.com/ar4mirez/spinel/issues/221) ·
[#224](https://github.com/ar4mirez/spinel/issues/224) ·
[#225](https://github.com/ar4mirez/spinel/issues/225) ·
[#231](https://github.com/ar4mirez/spinel/issues/231) · Phase 1 · `area:engine`

## Objective

Four compiler refusals filed out of PRD 0033's corrected ranking, taken in one
pass because three of them are small and the fourth is the only one that needs a
new heap type. Together they are 91 blocked examples, and #221 subsumes
[#233](https://github.com/ar4mirez/spinel/issues/233) (5 more) because `...`
cannot forward a block without the anonymous `&` that #233 is about.

## Baseline

Measured on this branch, before any change, `--platform=linux`, which is what
`scripts/spec-status.sh` pins.

| | |
|---|---|
| ruby/spec | 3835 files · 25624 examples · 2158 passed · 0 failed · 21547 blocked · 1919 skipped |
| `language/` | 2735 examples · 1312 passed |
| Rust tests | 11 in `tests/bytecode.rs` |

Refusal counts, from `scripts/spec.sh --platform=linux --blocked=0`:

| examples | refusal | issue |
|---:|---|---|
| 55 | `argument forwarding` | #221 |
| 15 | `a safe-navigation call` | #224 |
| 13 | `an integer wider than a fixnum` | #225 |
| 8 | `symbol interpolation` | #231 |
| 5 | `an anonymous block parameter` | #233, folded into #221 |

## Order, and why

1. **#231 symbol interpolation** — one opcode. Independent of the other three.
2. **#224 safe navigation** — one jump. Independent of the other three.
3. **#221 argument forwarding** — the largest row, and the one that unlocks #233.
4. **#225 bignum** — last, because it is the only slice that adds a dependency
   and a heap type, and doing it last means the three small slices never rebase
   on a moving `Value`.

## Measured before writing

Every claim below is CRuby 4.0.6, run, not remembered.

### #231 symbol interpolation

```ruby
b = "c"
:"a#{b}"              #=> :ac
:"a#{b}".equal?(:ac)  #=> true    — the run-time intern must hit the shared table
:"#{nil}"             #=> :""
:"a#{Foo.new}"        #=> :aF     — the *interpolated object* gets `to_s`
```

Redefining `String#to_sym` does **not** change `:"a#{b}"`, so the lowering must
not emit a `to_sym` send. It needs its own opcode over
`shared::symbols::intern`, which is already the append-only table the
no-global-state rule names as an exception.

### #224 safe navigation

```ruby
nil&.size             #=> nil
"ab"&.size            #=> 2
false&.to_s           #=> "false"  — only nil short-circuits, not false
```

The receiver is evaluated exactly once (measured with a counting lambda). When
the receiver is nil, **nothing to the right runs**: not the arguments
(`nil&.foo(side += 1)` leaves `side` at 0), not the block, and not the
right-hand side of an assignment form (`n&.foo = (side += 1)` and `h&.x += 1`
both leave `side` at 0).

### #221 argument forwarding

```ruby
def b(x, k: 0) = [x, k]
def a(...)     = b(...)
a(1, k: 2)     #=> [1, 2]
a(1)           #=> [1, 0]    — no empty keyword hash is manufactured
def a2(x, ...) = b2(x, ...)  — a leading argument before `...` is legal (3.0+)
```

`...` is exactly the anonymous `*`, `**` and `&` together, measured separately:
`def f(*) = e(*)`, `def f(**) = e(**)` and `def f(&) = e(&)` each forward their
one kind. The parameter side already gives an anonymous `*`/`**`/`&` a synthetic
slot; only the *argument* side refuses.

### #225 integer wider than a fixnum

```ruby
(2**70).class                 #=> Integer
4611686018427387903.class     #=> Integer   — fixnum max, same class
((2**70) - (2**70)).class     #=> Integer   — normalises back, still same class
```

Ruby has one `Integer` with no visible boundary, so the promotion has to be
invisible: every operation that can leave the fixnum range promotes, and every
result that fits demotes. `docs/engine.md` already settles the representation —
"Bignum arithmetic is a primitive over a pure-Rust bigint crate" — so this is
not an open design question.

## Plan

- [ ] 1. #231: `Insn::Intern`, lowering, oracle table, spec delta.
- [ ] 2. #224: nil-test-and-jump for call, assignment and op-assign forms.
- [ ] 3. #221: `...` parameter, `*`/`**`/`&` argument sites; closes #233.
- [ ] 4. #225: bignum heap type, arithmetic, literal parsing, promote/demote.
- [ ] 5. Re-run the corpus, record the delta, tag any new failure with a reason.
- [ ] 6. Triage the GitHub project and the issues.

## Definition of done

- Named ruby/spec files or directories newly pass, per `CLAUDE.md`.
- No example that passed before stops passing.
- Every semantics claim is in an oracle table generated from CRuby, not asserted.
- `bench/spec-status.md` regenerates.
- Any spec left failing is tagged in `spec/tags/` with a reason, never marked
  "expected failure" to make the slice green.

## Results

_To be filled in as the slices land._

## Left for later

_To be filled in._
