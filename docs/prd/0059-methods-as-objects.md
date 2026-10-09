# PRD 0059 — methods as objects

Issue: [#27](https://github.com/ar4mirez/spinel/issues/27) · Phase 2 ·
`area:core-lib`

## Objective

`obj.method(:name)` and `Mod.instance_method(:name)` did not exist, and
`f(&x)` refused any `x` that was not already a Proc. Fixture files across
the corpus stop at one or the other, so the cost was whole spec files, not
only `core/method`. This slice adds `Method`, `UnboundMethod`, the block
conversion, and what a `Proc` was missing beside them.

## Baseline

Corpus 15053 passed, 0 failed (on the #41 slice). 119 examples blocked on
`&` with a non-Proc; several hundred on `method` and `instance_method`,
most of them through a fixture.

## Design

**`&x` is an instruction.** `Insn::BlockToProc` leaves `nil` and a Proc
alone and sends anything else `to_proc`, before the call takes its block.
`Symbol#to_proc` is Ruby: a lambda that `public_send`s the symbol to its
first argument.

**A Method is what a lookup found.** `__reflect_method_lookup__` answers
the owner, arity, visibility, source position, parameters, the definition's
own number and the name it was first defined under. `Method` and
`UnboundMethod` are Ruby objects holding that, with hidden instance
variables so a program's `instance_variables` sees only its own.

**Calling one runs the body its owner defines.** `__method_call__(owner,
name, receiver, ...)` dispatches with a new target, `Target::At`, which
reads `owner`'s own method table and does no walk. That is what makes
`super_method`, a rebound `UnboundMethod`, and a method on a subclass that
has since overridden it all run the body they say they hold.

**Two methods are equal when they are one definition.** `==` and `hash`
compare the definition number, so an alias equals what it aliases.
`original_name` is the name the body was compiled under.

**Arity and parameters come from the parameter spec.** A Method's arity
counts keywords where a Proc's does not — CRuby's
`method_def_min_max_arity`, measured — so `ParamSpec` has `method_arity`
beside `arity`. `parameters` lists each kind with its name; a core method
written in Ruby reports kinds without names, as the C method it stands for
does. `define_method` takes a Method or an UnboundMethod and installs the
body it holds.

**`Proc#curry`, `>>` and `<<`** are Ruby, keeping lambda-ness as measured:
a curried proc stays a proc, and a composition is as lambda as whichever
runs first.

## Aliases that are aliases

Being able to compare methods exposed something unrelated: ruby/spec has an
example per alias (`Array#collect is an alias of Array#map`), and the core
library had written 59 of them as a second definition. `core/aliases.rb`,
loaded last, makes each a real alias. Seven pairs were written the other way
round in core — `Hash#include?` calls `key?` — and are aliased from the name
that has the body, since CRuby's direction would point each at the other.
`Array#to_s` and `Hash#to_s` became aliases where they are written. The
shadowed second definitions are still in their files; folding each into an
`alias` there is cleanup.

## What became visible

mspec's mocks pass blocks with `&`, so mock-based examples had been blocked
wholesale. They run now, and 86 disagree. 37 are a conversion or comparison
method Ruby calls and Spinel does not (`to_str`, `to_int`, `to_ary`, `<=>`),
which the mock counts. Each is tagged for the issue that owns its class.
Roughly thirty are tagged "not yet diagnosed": they are real, small and
unrelated to one another, and want a triage pass of their own.
*That pass is PRD 0062: twenty fixed, seventeen given a cause.*

## Not in this slice

- A Proc made by `to_proc` is a lambda around a call, so its arity is -1
  rather than the method's, and `inspect` does not show `(&:name)`.
- A primitive reports arity -1 and `[[:rest]]`, except the arithmetic
  operators.
- A method redefined on its owner after it was captured runs the new body:
  what is kept is the owner and the name. CRuby keeps the body.
- A destructuring parameter reports its first inner name.
- `Method#owner` for a `define_method` body, refinements, `Method#source`.

## Results

| | passed | failed | blocked |
|---|---:|---:|---:|
| before (#41 slice) | 15053 | 0 | 15561 |
| this slice | **15640** | **0** | 14905 |
