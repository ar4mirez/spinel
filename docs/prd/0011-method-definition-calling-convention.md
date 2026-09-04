# PRD 0011 — Method definition and the full calling convention

Tracks [#11](https://github.com/ar4mirez/spinel/issues/11). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:L`, `area:engine`.

## Objective

[#10](https://github.com/ar4mirez/spinel/issues/10) made Ruby source reach an answer for
the part of the language that needs no calling convention. Everything it left out is the
same thing: a call. `BinOp` has a fast path and no send behind it, `Method.body` is a
`Value` nothing writes, `Frame` is a local variable of `eval_in` with a `receiver` field
that is always `nil`, and `compile::call` refuses every call that is not a specialised
operator.

This slice is that missing half: `def`, the argument protocol Ruby actually has, `yield`,
blocks, procs, lambdas, and `send`. It is the single largest unblocker in the corpus. At
the baseline below, *one* blocked reason — "a method call is not compiled yet" — accounts
for 18,728 of the 23,606 blocked examples across the whole of ruby/spec.

### The honest shape of the target

The issue's definition of done reads "`def_spec.rb`, `block_spec.rb`, `lambda_spec.rb`,
`proc_spec.rb`, `yield_spec.rb`, `send_spec.rb` newly pass". Taken as *every example in
those six files*, it is not reachable from this slice, and the reason is not the calling
convention. Those files also depend on slices that have not landed:

```ruby
# yield_spec.rb — needs a class body (#13) and instance variables
@y = YieldSpecs::Yielder.new
-> { @y.z }.should.raise(LocalJumpError)   # and an exception object (#12)

# def_spec.rb — needs Object.private_instance_methods, which is core/ (#15)
Object.private_instance_methods(false).should.include?(:some_toplevel_method)
```

Counting matcher shapes across the six files: ~130 expectations are `should.raise`, which
cannot pass before #12 gives the VM an exception object. Most of `yield_spec.rb` reaches
its subject through an `@ivar` set in a fixture class.

So the measurable claim this slice makes is narrower and checkable: **the calling
convention stops being the blocker.** After it lands, no example in the corpus is blocked
on "a method call", "a method call with a block", "a method definition", or "a lambda",
and the `blocked → passed` delta on those six files plus the corpus is stated in the PR.
What remains blocked is named, and every name belongs to a later slice.

## Non-goals

- **Non-local exits.** `return` from a proc, `break` from a block, `redo`, `retry`,
  `throw`/`catch`. The roadmap gives these to [#12](https://github.com/ar4mirez/spinel/issues/12)
  along with exceptions, because they share one unwinding path and building half of it
  twice is worse than building it once. A lambda's `return` *is* in scope: it is a local
  return, not an unwind. A block's `next` is in scope for the same reason.
- **`**kwrest`.** Collecting leftover keywords needs a `Hash`, and there is no `Hash`.
  Declared keywords with and without defaults are in scope; `**kw` is `Unsupported`.
- **`core/*.rb`.** `Kernel#proc`, `Proc#call`, `Proc#arity` and `Proc#lambda?` are here as
  Rust primitives, because they are *dispatch* and dispatch is on engine.md's list of what
  a primitive is for. The rest of Kernel waits for #15.
- **Escape analysis on environments.** engine.md wants a heap environment only when a
  block captures one. This slice allocates one whenever a frame declares a local. See R6.
- **Inline caches.** The call-site side table this slice adds is the place they go, and
  they are not in it yet. The method cache from #8 is what dispatch uses.

## Users

The compiler, the interpreter, and `spec/harness`. Also, for the first time, a *Ruby
programmer*: an arity mismatch is the first diagnostic this project emits that a person
reads rather than a slice-picking script. R9 is about that.

## Requirements

### R1 — A call site is a table entry, not an instruction operand

`Insn::Send(u32)` indexes `Iseq::call_sites`. The descriptor there carries the method
name, the positional count, whether a splat is present, the declared keyword names, and
what block the call passes — a literal one, a `&blk` value, or none.

This is not to keep `Insn` at 16 bytes, though it does. It is because engine.md already
decided call sites need a per-heap side table for inline caches, keyed by call-site id,
since bytecode is shared across Ractors and an inline cache cannot be. Emitting the id
now means the cache slice adds a table and not an instruction format.

### R2 — Argument binding is one function, and the caller does not know the callee's shape

`bind` takes a `ParamSpec`, the argument values, and whether the callee is a lambda or a
proc, and fills a frame's locals. Required, optional with defaults, rest, post-required,
and keywords are all bound in Ruby's order: required first, post from the right, then the
rest, then optionals left to right while values remain.

The caller pushes arguments and does not inspect the callee's parameters, which is what
makes `send`, `yield`, `Proc#call`, and an ordinary call the same code path with a
different receiver.

### R3 — Proc and lambda differ in exactly two places, and both are the binder's

A lambda checks arity and raises `ArgumentError`. A proc pads with `nil`, drops extras,
and destructures a single `Array` argument across multiple parameters. Both behaviours
live in `bind` behind one `Arity` flag rather than in two binders, because ruby/spec's
`block_spec.rb` is largely a table of the second one:

```ruby
m([1, 2]) { |a| a }.should == [1, 2]              # no destructure: one parameter
m([1, 2, 3]) { |a, b| [a, b] }.should == [1, 2]   # destructures: two parameters
```

### R4 — `yield` is a call to the frame's block, and a missing block is a `LocalJumpError`

The block reaches the callee as a field of the frame, not as a local, so `yield` costs no
parameter slot and an anonymous block needs no name. `yield` with no block is `Raise {
class: "LocalJumpError" }` — the honest thing until #12 can build the object.

### R5 — A method body is a definition id, not a heap object

`Method.body` is a fixnum indexing a per-heap `Definitions` table whose entries are either
an `Iseq` or a native function. A heap object was the other option and it is worse: there
is no payload kind that can hold a Rust `Iseq` and no finaliser to free one, so it would
mean a second lifetime scheme for a table that is already per-heap and already traced.
A fixnum body is also a body the collector never has to trace.

### R6 — Locals live in a heap environment, and a block captures its defining frame's

A frame that declares locals allocates an `Env` — a `Slots` object, so the collector
traces it with no new code. A block iseq holds a pointer to the env it was created in, and
`GetLocal` walks up `depth` links.

```
// ponytail: an env per call with locals, not per call that a block captures.
// engine.md wants a `captured` bit from the resolve pass deciding it, which is
// a compiler pass this slice does not need to be correct — only to be fast.
// Upgrade when bench/ has a call-heavy number to move.
```

The alternative — locals in a `Vec` on the frame, promoted on capture — is the eventual
design and needs the analysis pass to exist first. Allocating always is correct now and
slower now, and the slice's check is a spec delta, not a benchmark.

### R7 — Frames are a stack in the loop, and the loop is still not recursive

`Send` pushes a `Frame` and continues; `Leave` pops one and pushes the return value onto
the caller's operand stack. A Ruby-to-Ruby call never grows the Rust stack, which is what
#10's module doc promised and what fibers will need.

Native methods are the one place Rust recursion happens, and they may not call back into
Ruby in this slice — `Proc#call` is the exception and it is implemented as a frame push,
not a re-entrant `eval`.

### R8 — An unknown method raises, and never returns `nil`

`spec/harness` treats a statement that merely *evaluates* as a passing effect. If dispatch
answered an unknown name with `nil`, `x.should_receive(:y)` would evaluate cleanly and the
example would be reported as passing without asserting anything. `NoMethodError` keeps
every matcher this harness does not implement in the `blocked` column where it belongs.

This is the same safety property #10's `Unsupported` has, one layer down: there is no path
from a construct the VM cannot mean to a *passing* spec.

### R9 — Arity errors carry Ruby's message text, exactly

ruby/spec asserts on the string:

```ruby
-> { m(1, 2) }.should raise_error(ArgumentError, "wrong number of arguments (given 2, expected 1)")
```

`(given 2, expected 1)`, `(given 1, expected 2+)` for a splat, `(given 0, expected 1..2)`
for optionals. The message is built where the binder already knows the shape. It is dead
text until #12 can raise it, and writing it now costs nothing and means #12 does not have
to rediscover the format.

### R10 — The harness runs `before` blocks

`block_spec.rb` defines the method under test in a hook:

```ruby
before :all do
  def m(a) yield a end
end
```

The walk in `discover.rs` already descends into `before` bodies looking for examples and
throws the statements away. It now collects them and prepends them to each example in the
group, outermost first. `before :all` is prepended per example rather than run once, which
is the only shape a fresh heap per example allows; an example that depended on state
accumulated across examples will *fail*, not falsely pass.

## Definition of done

- [ ] No example in the corpus is blocked on "a method call", "a method call with a
      block", "a method definition", or "a lambda".
- [ ] `language/def_spec.rb`, `block_spec.rb`, `lambda_spec.rb`, `proc_spec.rb`,
      `yield_spec.rb`, `send_spec.rb`: `blocked → passed` delta stated in the PR, with the
      remaining blocked reasons named and each attributed to a later slice.
- [ ] Lambda and proc arity and `return` differ correctly; the proc half of `return` is
      `Unsupported` naming #12 rather than wrong.
- [ ] `cargo test` green, including a miri-safe interpreter test that pushes a frame.
- [ ] Arity message text matches Ruby's, checked by a unit test per shape.
- [ ] engine.md's "Calling convention" and "Frames" sections match what landed.

## Tasks

1. `ParamSpec` on `Iseq`; lower `spinel_ast::Params` into it in the compiler.
2. `Definitions` registry; `Method.body` becomes a definition id.
3. `Insn::Send`/`Yield`/`DefineMethod`/`MakeProc`/`Return`, and `Iseq::call_sites`,
   `Iseq::children`.
4. Compiler: `def`, calls with a receiver and without, blocks, `yield`, `->`.
5. Interpreter: frame stack, env objects, dispatch through `Classes::lookup`.
6. `bind`: the argument protocol, both arities, Ruby's message text.
7. Native primitives: `Proc#call`, `Proc#arity`, `Proc#lambda?`, `Kernel#proc`,
   `Kernel#lambda`, `Object#send`/`__send__`/`public_send`.
8. Harness: `before` hooks.
9. Docs: engine.md, roadmap check line, this PRD's numbers.

## Numbers

### The six files

| file | before | after |
|---|---|---|
| `block_spec.rb` | 0 passed · 166 blocked | **22 passed** · 144 blocked |
| `def_spec.rb` | 0 passed · 72 blocked | **12 passed** · 60 blocked |
| `lambda_spec.rb` | 0 passed · 15 blocked | **4 passed** · 11 blocked |
| `proc_spec.rb` | 0 passed · 38 blocked | **4 passed** · 34 blocked |
| `send_spec.rb` | 0 passed · 78 blocked | **1 passed** · 77 blocked |
| `yield_spec.rb` | 0 passed · 39 blocked | 0 passed · 39 blocked |
| **total** | **0 passed** · 408 blocked | **43 passed** · 365 blocked · **0 failed** |

Whole corpus: `164 → 238` passing, `23606 → 23532` blocked, 0 failed.

### What blocks the six files now, and whose slice it is

```
    171  assigning an instance variable        #9 shapes / #13
     63  a constant                            #13
     47  a local variable from an enclosing scope   see below
     15  a class or module body                #13
     14  NoMethodError: `mock`                 mspec's mocking; no slice
      7  a hash literal                        #15
      6  a constant path                       #13
      4  `begin`/`rescue`                      #12
      4  a keyword rest parameter              needs Hash — #15
      4  an anonymous block parameter          #11 follow-up, 4 examples
```

Not one of them is the calling convention. The four DoD phrases —
`a method call`, `a method call with a block`, `a method definition`,
`a lambda` — return zero matches across all 3,835 files.

`yield_spec.rb` stays at zero because every example in it reaches its subject
through `@y = YieldSpecs::Yielder.new`: an instance variable assigned in a hook,
holding an instance of a fixture class. Both halves are #13's. The `yield`
machinery it is meant to test is exercised instead by `block_spec.rb` and by the
measured table below.

"A local variable from an enclosing scope" is `send_spec.rb`'s `specs =
LangSendSpecs` at file level. The harness compiles each example's statements on
their own, so a file-level local is not in scope for them — and those examples
are blocked on the fixture constant regardless. It is the harness's boundary,
not the compiler's, and it moves when fixtures load.

### Checks

- `cargo test`: 22 suites green, including 55 new calling-convention cases in
  `tests/eval.txt` measured against `ruby 4.0.6` by `scripts/eval-oracle.rb`.
- `cargo +nightly miri test -p spinel-vm --lib`: 56 passed. The new heap traffic
  is an environment per frame and a four-slot `Proc`, and miri sees both.
- `cargo clippy --all-targets`: no warnings.
- `ruby scripts/verify-passes.rb spec/ruby`: **all 238 passing examples re-run
  on ruby 4.0.6 and agree.** No false passes anywhere in the corpus.

## What the audit caught

Ten things. The first five the checks found; the last five came from reading the
code against Ruby's semantics *after* CI was already green, which is the whole
argument for doing both:

1. **A wrong answer in the binder.** `{ |a, b=5, c=6, d, e| }` given six values
   bound `[1, 2, 3, 5, 6]` where Ruby binds `[1, 2, 3, 4, 5]`. Post-required
   parameters are taken from the right only when a splat is there to absorb the
   middle; without one, binding stays left-to-right and the extras are dropped.
   Caught as a *failure* rather than a blocked example, which is the column
   working as #5 designed it.
2. **`return` broke the stack-depth model.** `Insn::Return` counted as popping,
   so the `Leave` after it underflowed and a `debug_assert` fired on the whole
   corpus — in the release build it silently mis-sized `max_stack` instead.
   Control does not fall through a `return`, so it is neutral in the model.
3. **Top-level `self` was `nil`, so every receiverless call failed.** `nil` has
   no class, and the first corpus run after dispatch landed reported 63 examples
   in one file blocked on it. Ruby's answer is that top-level `self` is `main`,
   an ordinary `Object`; building one made `def foo; end; foo` work.
4. **A pre-existing race in `symbols::tests::the_table_only_grows`.** It read a
   process-global length before and after interning, which any parallel test
   could disturb. Latent before this slice and reproducible one run in seven
   after it, because `bootstrap` now interns the primitives' names. Rewritten to
   assert the property — interning is idempotent, a new name gets a new id —
   rather than a global count.
5. **The anti-false-pass verifier had gone out of sync twice**, and only CI
   caught it. `scripts/verify-passes.rb` re-runs every passing example on real
   Ruby by slicing it back out of the file; teaching the harness to run `before`
   blocks without teaching the verifier meant it eval'd examples whose helper
   method was defined in a hook it never saw — 21 reported as false passes that
   were really the two disagreeing about what an example *is*. `--list` now
   emits every span that ran, and the verifier concatenates them. The last
   remaining one was subtler: slicing an example out of a file leaves its
   `# frozen_string_literal: false` behind, and that comment decides whether
   `(+s).equal?(s)` is true. The verifier now carries magic comments across.
6. **`yield` inside a block yielded to itself.** A `Proc` took its block from
   whoever *called* it rather than from the scope that defined it, so
   `def outer; inner { yield 1 }; end` made that block yield to itself — an
   infinite loop, caught by the instruction budget rather than by a wrong
   answer. No spec reached it and CI was green; found by working through what a
   captured block *is*. The block is now a fifth `Proc` slot, captured with the
   environment and the receiver, and four cases in `eval.txt` hold it.
7. **A splat expanded every array argument, not the splatted one.** The call
   site recorded *that* there was a splat rather than *which* argument it was,
   so `f(a, *b)` with an array `a` passed `a`'s elements as separate arguments
   — a wrong answer, not a missing feature. Positions now, and four cases in
   `eval.txt` cover one splat, two, a splat between fixed arguments, and a
   splat filling required parameters.
8. **A blocked reason claimed Ruby raises where Ruby does not.** `f(x: 1)`
   against `def f(a)` reported `ArgumentError: unknown keyword: :x`. Ruby packs
   the keywords into a trailing positional `Hash` and passes them. No false pass
   — a raise blocks the example either way — but the report is how the next
   slice gets chosen, and a reason that misstates Ruby is worse than one that
   admits ignorance. It now says the truth: a `Hash` is missing. A method that
   *does* declare keywords and gets an unknown one still raises `ArgumentError`,
   which is Ruby.
9. **Two tests assumed a pristine global symbol table.** `SymbolId(8)` was a
   name nothing defined until `bootstrap` began interning the primitives onto
   `Kernel`, which is in every class's ancestry — so a lookup asserted to miss
   started hitting. Both now intern a name instead of fabricating an id.
10. **A diagnostic that named the wrong slice.** 141 examples reported
   "`a method call` on a receiver whose class the VM has not created yet", which
   reads as this slice's gap and is the class table's. It now names the receiver:
   "nil, whose NilClass the VM has not created yet". The blocked report is how
   the next slice gets chosen, so a reason that points at the wrong file is a
   real defect in it.

## Numbers that moved after the first green CI

The reading pass below found five more things once every check was already
passing, and two of them changed counts: `next` in a block took the corpus from
237 to 238, and the splat fix corrected an answer no spec had reached. The six
files stayed at 43 throughout — what moved was correctness, not coverage, which
is the more important half.

## Follow-ups

Small, and each one is a spec count rather than a guess:

- `break` out of a block (22 examples) — #12, with the other non-local exits.
- `**kwrest` (19) — needs `Hash`, so it lands with #15.
- A singleton method definition (9) — needs singleton classes, #13.
- Safe navigation `&.` (6) — needs `nil` to have a class, #13.
- A destructuring block parameter, `{ |a, (b, c)| }` (6) — needs multiple
  assignment, filed as #154.
- An anonymous block parameter, `def f(&); g(&); end` (4), and `...` (1).
- Visibility. `public_send` is registered but behaves as `send`, because nothing
  tracks public/private yet. ruby/spec checks this with `should.raise`, so it is
  blocked rather than passing wrongly — but it is wrong, and #13 owns it.
- An environment per frame rather than per captured frame, and the interpreter's
  operand stack as a root source. Both are named in `interp.rs`; the second is
  what lets a frame's roots be released on return.
