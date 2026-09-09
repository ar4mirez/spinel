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

- [x] 1. #231: `Insn::Intern`, lowering, oracle table, spec delta.
- [x] 2. #224: nil-test-and-jump for call, assignment and op-assign forms.
- [x] 3. #221: `...` parameter, `*`/`**`/`&` argument sites; closes #233.
- [x] 4. #225: bignum heap type, arithmetic, literal parsing, promote/demote.
- [x] 5. Re-run the corpus, record the delta, tag any new failure with a reason.
- [x] 6. Validate the four issues against their own "measure before writing" lists.
- [x] 7. Triage the GitHub project and the issues.

## Definition of done

- Named ruby/spec files or directories newly pass, per `CLAUDE.md`.
- No example that passed before stops passing.
- Every semantics claim is in an oracle table generated from CRuby, not asserted.
- `bench/spec-status.md` regenerates.
- Any spec left failing is tagged in `spec/tags/` with a reason, never marked
  "expected failure" to make the slice green.

## Results

Measured with `scripts/spec-status.sh`, which pins `--platform=linux`, so these
are the same numbers CI regenerates and diffs.

| directory | passed before | passed after | delta |
|---|---:|---:|---:|
| `core/integer` | 64 | 76 | +12 |
| `language` | 1144 | 1164 | +20 |
| **total** | **2158** | **2190** | **+32** |

`core/integer` is #225: the bignum heap type moves 12 examples out of
`an integer wider than a fixnum`. The 20 in `language` are #221, #224 and #231
together.

Blocked falls by exactly the 32 that now pass — 21547 to 21515 — and `failed`
stays 0 in every directory. No example that passed before stopped passing, and
nothing landed in `failed`, so this slice owes no new `spec/tags/` entry.

All four refusals are gone from the blocked ranking entirely — measured, not
inferred: `scripts/spec.sh --platform=linux --blocked=0 language core/integer`
no longer lists `argument forwarding`, `a safe-navigation call`,
`an integer wider than a fixnum` or `symbol interpolation` at any count.

So the 91 examples those refusals named did not all pass, but none of them is
still waiting on this slice. They are blocked a second time, and the top
blockers under that command's scope are now mspec's own helpers rather than
compiler refusals:

| examples | now blocked by |
|---:|---|
| 194 | `mock` |
| 168 | `bignum_value` |
| 80 + 63 | `eval` |
| 72 | `defined?` before `require` (#39) |

`bignum_value` is the one to note: #225 landed the heap type, and the remaining
`core/integer` specs are held by the mspec *helper* of that name, not by the
arithmetic. An undefined method reports blocked rather than failed, so a pass count alone
hides it — the next integer slice is a helper slice, not an engine one.

`bench/spec-status.md` regenerated in this PR; it had been left stale by the
first two commits, which CI would have caught as a `git diff --exit-code`
failure.

## Validation

The four slices were re-checked against the "measure before writing" list in each
issue rather than against the PRD's own summary — a differential oracle, every
snippet run on CRuby 4.0.6 and on Spinel and the answers compared. 80 cases,
72 agreeing; the 8 that do not are accounted for one by one below and none of
them is one of these four slices.

**All four refusals are confirmed gone.** `scripts/spec.sh --platform=linux
--blocked=0 language` lists none of `argument forwarding`, `a safe-navigation
call`, `an integer wider than a fixnum` or `symbol interpolation` at any count.
Every "measure before writing" item in #221, #224 and #225 answers the way CRuby
does, including the ones the issues singled out as easy to get wrong: `false&.x`
sends rather than short-circuiting, the safe-navigation receiver is evaluated
exactly once, nothing to the right of a nil receiver runs — argument, block, or
the right-hand side of an assignment — and `a(1)` through `def a(...)` does not
manufacture an empty keyword hash.

The 49 rows added to `crates/spinel-vm/tests/eval.txt` are that check, kept.
Verified by mutation rather than assumed: changing an expected answer makes
`cargo test -p spinel-vm --test eval` fail on that line number, so the rows are
read and not skipped.

### Three defects the slice left behind

The spec counts could not have found any of them. All three are wrong *answers*
in examples that are blocked for an unrelated reason, and an example that never
runs cannot fail — the same blind spot that hides a core method which raises.

**1. `==` on integers past 2^53 (fixed here).** #225 gave bignums an integer
comparison because "past 2^53 two different bignums round to the same f64". The
identical bug was one arm away, in the *fixnum* path: `ruby_eq` compared every
numeric pair as `f64`.

```ruby
2**54 == 2**54 + 1        # ruby => false   spinel was => true
{2**54 => 1}[2**54 + 1]   # ruby => nil     spinel was => 1
```

The hash row is the one that matters: `Hash` looks a key up with `==`, so this
was returning another key's value. `binop` already split the pair the right way
for every other operator; `ruby_eq` now does too, and the mixed integer/float
arm is exact rather than a cast.

**2. `Symbol#inspect` never quoted (fixed here).** It was `":" + to_s`, so the
PRD's own measured claim `:"#{nil}" #=> :""` printed as a lone colon. #231 is
what makes this reachable — an interpolated symbol is the ordinary way to build
a name that is not an identifier. The three bare shapes are measured, not
recalled, and the sigil forms take no suffix: `:a?` is bare, `:"@a?"` is not.

**3. `Float`/`Integer` `==` is asymmetric across the fixnum boundary (filed).**
`(2**70) == (2**70).to_f` is true and `(2**70).to_f == (2**70)` is false. Needs
the `BigInt` version of the exact comparison, so it is
[#238](https://github.com/ar4mirez/spinel/issues/238) rather than a fourth fix
here.

Neither fix moves a spec count: `bench/spec-status.md` regenerates to the same
2190 passed / 0 failed / 21515 blocked. That is the finding, not a
disappointment — the progress bar is blind to a method that answers wrongly, and
the oracle table is the instrument that is not.

### The 8 that still disagree

Every one is either an issue filed here or the documented minimal core library,
and none is a wrong answer inside #221, #224, #225 or #231.

| cases | disagreement | where it belongs |
|---:|---|---|
| 4 | `{a: 1}` printed as `{:a => 1}` | [#216](https://github.com/ar4mirez/spinel/issues/216), already open |
| 1 | `alias :"m#{2}" :m1` refuses | [#237](https://github.com/ar4mirez/spinel/issues/237), filed |
| 1 | `"ab"&.size&.+(1)` — the `.+()` send, not the `&.` | [#239](https://github.com/ar4mirez/spinel/issues/239), filed |
| 2 | `Array#uniq`, `Integer#bit_length` undefined | core library is still minimal, by design |

The two undefined methods are the expected state, not a defect: `spinel --help`
says so, and an undefined method reports `NoMethodError` rather than answering
wrongly. The four `Hash#inspect` rows are the reason the forwarding rows in
`eval.txt` index the hash — `f224(a: 1)[:a]` — instead of comparing it whole.

### #231's open question, answered

The issue asked whether `an interpolated method name here` (4 examples) wanted
the same run-time intern and might be one slice with it. Measured: **no.**
`Insn::Intern` builds the symbol, but `alias` and `undef` never take a symbol as
a value — `Alias(u32, u32)` and `Undef(u32)` index `Iseq::symbols` at compile
time, so the intern opcode cannot reach them. It needs stack-operand forms of
those two instructions, which is a separate small slice:
[#237](https://github.com/ar4mirez/spinel/issues/237).

### Also filed

- [#239](https://github.com/ar4mirez/spinel/issues/239) — operators are not
  dispatchable methods, so `2.+(1)`, `2.send(:+, 1)` and `2.respond_to?(:+)` all
  fail. Found through `"ab"&.size&.+(1)`; the safe navigation was correct and the
  send underneath it was not, so #224 is unaffected.
- [#216](https://github.com/ar4mirez/spinel/issues/216) already had `Hash#inspect`
  writing `{:b => 1}` where Ruby 3.4+ writes `{b: 1}`; commented with the measured
  rule, which is a *different* predicate from `Symbol#inspect`'s, and with the
  consequence that no `eval.txt` row can evaluate to a symbol-keyed hash until it
  is fixed.

## Left for later

- The mspec helpers now at the top of the ranking — `mock` (194), `bignum_value`
  (168) and `eval` (143 across two reasons). None is a compiler refusal, so they
  do not belong to this PRD's phase; they need their own triage.
- Per-file spec granularity. `CLAUDE.md` accepts "files or directories"; this
  records directories, because per-file over ~1,932 files x 2 trees is dominated
  by process startup. [#147](https://github.com/ar4mirez/spinel/issues/147) is
  the issue for making CI publish it.
