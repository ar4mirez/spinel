# PRD 0056 — operators as methods

Issue: [#239](https://github.com/ar4mirez/spinel/issues/239) · Phase 2 ·
`area:engine`

## Objective

`2 + 1` answered 3 and `2.send(:+, 1)` raised `NoMethodError`. The arithmetic
and relational operators existed only as instructions, so anything that
reaches one by name — `send`, `public_send`, `&.+`, `inject(:+)`,
`alias_method` — found nothing. This slice puts the operators in the method
table and makes the instruction and the method agree, in both directions.

## Baseline

Corpus 14186 passed, 0 failed. On `main`:

```ruby
2.send(:+, 1)          # NoMethodError: undefined method '+' for an instance of Integer
[1, 2, 3].inject(:+)   # the same
"ab"&.size&.+(1)       # the same
2.send(:-@)            # NoMethodError
"a".send(:!~, /a/)     # NoMethodError
-Foo.new               # refused: `-@` on an operand that is not a number
!Foo.new               # false, whatever Foo#! says
```

`Numeric#+` was there, and is where the `NoMethodError` came from: it is the
coercing half, written to refuse a pair of numbers because reaching it meant
the fast path had already declined them.

## Design

**The operator is a primitive on `Integer` and on `Float`.** `Native::NumOp`
for `+ - * / % < <= > >=` and `Native::NumNeg` for `-@`, each calling the
function its instruction calls. When that declines — the operand is not a
number — the primitive dispatches `super` under the operator's own name,
which is `Numeric`'s coercing operator. The name is the operator's and not
the call's, because `alias_method :old_plus, :+` makes them differ.

`==` and `!=` are left as they were: `BasicObject`'s and `Comparable`'s
already answer a send, and the instruction compares every type the VM can
without asking.

**The instruction asks before it answers.** The issue's first question was
whether redefining `Integer#+` changes `1 + 1`. It does — 42, measured, and
`core/integer/plus_spec.rb` has an example for it. So the class table keeps
the bodies the VM installed for each numeric class (`seal_operator`) and a
mask of which names still resolve to them. `Classes::invalidate` clears one
flag; the next operator instruction re-reads the names of whichever class's
serial moved. In a running loop that is one flag and one mask per
instruction. A `def`, a `prepend`, an `undef` and an alias back to the
original are all heard, because what is compared is the body a lookup finds.

**`core/*.rb` is exempt.** It stands where CRuby has C, and C adds two
integers without asking `Integer#+`. Without the exemption a program's
`def <(o)` on `Integer` would run inside every `while i < n` in the core
library. The check reads the frame's path, and only once the mask has
already said no.

**`-@` and `!` send.** `Insn::Neg` falls back to a send of `-@` for an operand
that is not a number, as `Insn::BinOp` already did for its operators.
`Insn::Not` sends when the operand's class resolves `!` to something other
than `BasicObject#!`.

**`Kernel#!~`** is Ruby: the negation of the receiver's own `=~`.

**`defined?` in a void context is not evaluated.** `defined?(a.b / 2); c`
never calls `a.b`, measured. The compiler drops a `defined?` whose value
nothing reads.

## Found on the way

- `1 + nil` said `NilClass can't be coerced into Integer`; Ruby names `nil`,
  `true`, `false` and a Symbol by `inspect`. `Comparable` already had the
  helper.
- `docs/engine.md` and `BinOp`'s own documentation still said the send
  behind the fast path did not exist. Both rewritten.

## Not in this slice

- `2.method(:+)` and `Integer.instance_method(:+)`: there is no `Method`
  object yet (#27). The table entry they will find is here.
- `[1, 2].map(&:-@)`: `&` on a Symbol is refused, operators or not.
- `Integer.instance_methods(false)` does not list `==`, which is still
  inherited.
- `!` on an immediate is answered without a lookup, so a `!` defined on
  `NilClass`, `Integer` or `Symbol` is not seen. Marked `ponytail:`.

## Results

| | passed | failed | blocked |
|---|---:|---:|---:|
| `main` | 14186 | 0 | 16414 |
| this slice | **14208** | **0** | 16393 |

By directory: `core/kernel` 1051 to 1056, `core/exception` 178 to 182,
`core/numeric` 13 to 16, `language` 2130 to 2133, `core/enumerable` 479 to
481, `core/integer` 417 to 419, and one each in `core/array`, `core/float`
and `library/rbconfig`. The two `defined?` tags that named #239 are deleted.
One tag is new: `Enumerable#inject ignores the block if two arguments` runs
now and expects a warning, which is #268.

The guard costs about 3% on a loop that does nothing but fixnum arithmetic
(`s = s + i * 2 - 1` three million times: 2.81s to 2.91s user, ten alternated
runs). `ruby --yjit` runs the same loop in 0.32s.

`tests/eval.txt` holds 36 new rows, measured on ruby 4.0.6. A `prepend` and
an `undef` cannot be taken back in the oracle's one process, so those two
are a test in `tests/eval.rs`.
