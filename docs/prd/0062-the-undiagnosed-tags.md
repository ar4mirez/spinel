# PRD 0062 — the undiagnosed tags

Follows [#27](https://github.com/ar4mirez/spinel/issues/27) (PRD 0059) ·
Phase 2 · `area:core-lib`, `area:engine`

## Objective

PRD 0059 left thirty-seven examples tagged "not yet diagnosed": real
failures first reached once mspec's mocks ran, with no cause written down. A
tag without a cause is a failure nobody owns. This slice gives each one a
cause, and fixes the ones whose fix is small.

## Baseline

Corpus 16415 passed, 0 failed. 37 tags reading "not yet diagnosed".

## Fixed

Each measured on ruby 4.0.6 and pinned in `crates/spinel-vm/tests/eval.txt`.

**Equality asks the other side.** `[1] == obj` with a `to_ary`, and `"a" ==
obj` with a `to_str`, are `obj == self` in Ruby. The operator's fast path
answered false for any String or Array against something of another kind;
it now leaves an object that might convert to `Array#==` and to a new
`String#==`. `1 == obj` already asked the object, but handed back whatever it
said: `Numeric#__eq_other__` makes the answer a boolean.

**Comparison asks the other side.** `String#<=>` converts with `to_str`, and
otherwise asks `other <=> self` and turns the answer round. `Time#<=>` does
the second. Two Strings still never leave the primitive, which is what a
sort calls; only another kind of operand is sent on to Ruby.

**A block sees its method's block, not its own.** `block_given?` and `yield`
inside a block mean the block of the method the code was written in. A proc
frame took the block it was *called* with instead, so `define_method(:m) {
block_given? }` was true when `m` got a block. The passed block still binds
`&b`.

**Conversions the language makes count private methods.** Multiple
assignment calls a private `to_ary`, a splat a private `to_a`, and a `to_ary`
answering nil declines.

**Smaller ones.**

- `Exception.new(obj)` asks `obj.to_s`, through the Ruby `initialize`.
- `Enumerable#flat_map` spreads what converts with `to_ary`.
- `Array#min`, `#max` and `#minmax` are Array's own methods.
- `Enumerator#each` takes arguments and appends them.
- `warn(uplevel: 0)` names the line that called `warn`; it named the caller's
  caller.
- `ENV.value?` converts with `to_str`.

Removing the tags found eleven more that had gone stale — examples about
`$/`, `$-0`, `$\` and `$,` that pass and were still marked.

## Diagnosed and left

Seventeen keep a tag, each now saying why:

- `Kernel#remove_instance_variable` does not exist: shapes have no removing
  transition (#28).
- `Array#flatten` never reaches a `to_ary` that only `method_missing`
  answers. Asking every element would cost an exception per non-array (#21).
- A Symbol carries no encoding (#20), a primitive no frame (#45) and no
  parameter list (#27).
- `$~` is one slot per heap (#19); `$.` is an ordinary global (#41).
- `singleton_methods` and a module prepended to `Module`; `load` with `wrap`
  (#28). `Fiber#raise` with backtrace locations (#16). A glob that may match
  `.` (#42).

## Check

The ruby/spec delta is in the pull request. No tag reads "not yet diagnosed".
