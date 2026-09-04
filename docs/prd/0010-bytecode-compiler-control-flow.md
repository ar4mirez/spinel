# PRD 0010 — Bytecode format and compiler: literals, locals, control flow

Tracks [#10](https://github.com/ar4mirez/spinel/issues/10). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:L`, `area:engine`.

## Objective

[#6](https://github.com/ar4mirez/spinel/issues/6), [#7](https://github.com/ar4mirez/spinel/issues/7)
and [#8](https://github.com/ar4mirez/spinel/issues/8) built a heap that can hold Ruby
objects and a class table that can find a method on one. Nothing has ever *run*. This
slice is the first one where Ruby source reaches an answer: a bytecode format, a compiler
from `spinel_ast` down to it, and an interpreter loop that executes it.

The scope is the part of Ruby that needs no calling convention — literals, local
variables, `if`/`unless`, `while`/`until`, `case`/`when`, and the logical operators — plus
the specialised arithmetic and comparison instructions without which none of the loop
specs can even count to ten.

The measurable point of the slice is the fourth column in `scripts/spec.sh`. #5 shipped a
harness that reports every example `blocked` because there was no evaluator, and said so
rather than inventing a pass count. This slice deletes that seam for the examples it can
reach, and the `blocked → passed` delta is the definition of done.

### The honest shape of the target

The issue's definition of done reads "`if_spec.rb` … newly pass". Taken as *every example
in those five files*, it is not reachable from this slice and was never going to be: the
first example of `if_spec.rb` is

```ruby
a = []
if true
  a << 123
end
a.should == [123]
```

`Array#<<` is a method call, and the calling convention is
[#11](https://github.com/ar4mirez/spinel/issues/11); `Array` itself is
[#15](https://github.com/ar4mirez/spinel/issues/15). The issue's own triage comment says
the real target — "start moving examples out of `blocked`" — and that is what this PRD
commits to. `.should` is 201 of the calls in the five files and every other receiver is a
long tail, so the harness handling that one matcher shape is what converts control-flow
coverage into a pass count.

## Non-goals

- **Method definition, calls, blocks, `yield`.** #11. No `Insn` here pushes a frame. The
  interpreter loop is written non-recursive from the first commit — engine.md requires it
  for fibers and for Ruby's recursion limits — but it runs exactly one frame until #11
  gives it a second.
- **Exceptions, `ensure`, `rescue`, catch tables.** [#12](https://github.com/ar4mirez/spinel/issues/12).
  A failed instruction here aborts the frame with a Rust-level error; there is no Ruby
  exception object to raise, so `should.raise` stays blocked.
- **Constants, `class`/`module` bodies, `self`.** [#13](https://github.com/ar4mirez/spinel/issues/13).
  `Var::Const` compiles to `Unsupported`.
- **Instance, class, and global variables.** They need shapes
  ([#151](https://github.com/ar4mirez/spinel/issues/151)) and a global table. Locals are
  the whole variable story in this slice.
- **`String`, `Array`, `Hash` as classes with methods.** #15. String and array *literals*
  allocate here, because a literal is not a method call and `unless_spec.rb` compares
  strings, but they answer no messages.
- **`case/in` pattern matching.** `CaseBranches::In` compiles to `Unsupported`; `when` is
  this slice's half.
- **Inline caches.** engine.md puts a monomorphic cache at every call site, keyed on
  `Classes::serial()`. There are no call sites yet. The side table arrives with #11.
- **On-disk bytecode.** The format is *position-independent* here — that is a requirement,
  R4 — but serialising it is phase 3's cache and `core.image`. Position-independence is
  the property that makes those possible, and it is cheap now and expensive to retrofit.
- **`for`, `redo`, `retry`, flip-flops.** `for` needs `each`; the rest need frames.

## Users

| User | Needs from this slice |
|---|---|
| [#11](https://github.com/ar4mirez/spinel/issues/11) calls, blocks | A frame layout with a locals base and a value stack that a second frame can push onto, and an `Iseq` that a method entry can be |
| [#12](https://github.com/ar4mirez/spinel/issues/12) exceptions | Instruction indices that a catch table can name ranges of, and an unwind path that already exists as `Err` |
| [#13](https://github.com/ar4mirez/spinel/issues/13) constants, modules | `Unsupported` as a compile-time answer, so an unimplemented node is a blocked spec and never a wrong one |
| [#15](https://github.com/ar4mirez/spinel/issues/15) `core/*.rb` | A compiler that runs on the host, which is what `core.image` is built by |
| [#9](https://github.com/ar4mirez/spinel/issues/9) per-class serials | Nothing yet; noted so the call-site table lands once |
| phase 3 bytecode cache | Position-independent `Iseq`, symbols by name |
| `spec/harness` | `compile` + `eval_in`, and a shared frame so an example's locals survive from one statement to the next |

## Requirements

### R1 — Symbols get the table `src/shared/` was reserved for

`Value::symbol` has existed since #6 with a `// ponytail:` comment saying the name table
is shared append-only state that lands with `src/shared/`. Bytecode stores symbols, so it
lands here.

`Symbols` is process-global, append-only, and behind an `RwLock`: a `SymbolId` is an index
that is never reused and never invalidated, so a read is a bounds check and the lock is
uncontended in the common case. This is the exception `CLAUDE.md` names — "immutable
append-only tables (symbols, frozen literals)" under `crates/spinel-vm/src/shared/` — and
it is the *only* global this slice adds. Interning is idempotent and the table is
monotone, so no Ractor can observe another's write as a change.

### R2 — Instructions are a Rust enum, not a byte buffer

The name is "bytecode" and the representation is `Vec<Insn>`. A byte buffer buys density
and a decode step; the decode step is exactly what a `match` on an enum already does, and
Cranelift in phase 6 lowers from the enum either way. `Insn` is `Copy` and 16 bytes, so
the array is as dense as a naive encoding would have been.

The rule this trades away is that the on-disk format is then a *serialisation* of the enum
rather than the enum's own bytes. Phase 3 pays that, and pays it once.

### R3 — Jumps are relative, and the compiler patches them

A jump carries a signed displacement from the instruction after it, so an `Iseq` can be
concatenated, cached, or shared between Ractors without relocation. The compiler emits a
placeholder and patches it when the target is known; `Compiler::patch` is the one place
that writes a displacement, so an off-by-one is one bug rather than five.

### R4 — An `Iseq` names its symbols, and relinks on load

Position-independence is the issue's second checkbox. An `Iseq` carries a `symbols:
Vec<Box<str>>` pool and every instruction that means a symbol holds an *index into that
pool*, not a `SymbolId`. `Iseq::link` walks the pool once, interns each name into the
process table, and produces the `SymbolId` vector the interpreter indexes. A bytecode file
written by one process and read by another therefore agrees about symbols without agreeing
about the order they were first seen in.

Literals work the same way: a `Literal` is a value description (`Int`, `Float`, `Str`,
`Sym`), never a `Value`, because a `Value` is a pointer into one heap.

### R5 — Arithmetic and comparison are instructions, with a documented ceiling

`i += 1` and `i > 9` are method calls in Ruby. Compiling them as calls needs #11; leaving
them out leaves every loop spec blocked. So the compiler emits `Insn::BinOp` for the
operators YARV specialises — `+ - * / % == != < <= > >= <<` — and the interpreter
implements them for fixnums and flonums directly.

The ceiling is explicit, and it is a `// ponytail:` comment in the interpreter: an operand
that is not a fixnum or a flonum is not "wrong", it is *not yet dispatchable*, and the
instruction returns `Error::NoDispatch` rather than guessing. When #11 lands, `BinOp`
grows a fallback that sends the operator as a message, and every site that already emits
it starts working on every type. This is YARV's `opt_plus` shape exactly: a fast path with
a real send behind it.

Integer overflow promotes to a bignum in Ruby. There is no bignum yet, so overflow is
`NoDispatch` too, checked rather than wrapped — a wrong answer would be worse than a
blocked spec.

### R6 — `case/when` is `===`, and `===` is not `==`

`when` compares with `Object#===`, which for the literal types this slice has is `==`, and
for a `Range`, `Class`, `Regexp`, or `Proc` is not. Only the value cases are implemented;
the others return `NoDispatch`, so a `case` over classes is blocked rather than silently
matching on identity. A `when` with several conditions tries them left to right and
short-circuits, and a `case` with no predicate tests each condition for truthiness — two
different lowerings out of one `ExprKind::Case`.

### R7 — An unsupported node is a compile error, never a wrong answer

`Compiler::expr` returns `Result<(), Unsupported>` and every node this slice does not
implement returns `Unsupported` with the node's name and span. The harness turns that into
`blocked`. The property that matters: there is no path from an unimplemented construct to
a *passing* example. A pass count that can only be honest is the whole reason #5 shipped a
`blocked` column instead of matchers.

### R8 — The harness evaluates, and mspec's DSL stays out of the VM

The VM gets no knowledge of `should`. `spec/harness` walks an example's statements; a
statement shaped `<lhs>.should == <rhs>` (and `should_not`, and `.should ==` on a
multi-line receiver) compiles its two sides as separate `Iseq`s and evaluates them **in
one shared frame**, so `a = 1` in statement one is visible to statement three. Equality is
`vm::interp::ruby_eq`, the same function `Insn::BinOp` uses, so the harness cannot pass an
example the VM would fail.

Anything else in the body compiles as a statement and runs for its side effects. One
unsupported statement blocks the example; it does not fail it.

## Definition of done

1. `scripts/spec.sh language` reports a **non-zero `passed` count**, up from 0, and the PR
   states the delta per file for the five named files.
2. No example moves to `failed`. A construct Spinel does not implement is `blocked`.
3. `Iseq` is position-independent: symbols by name, jumps relative, literals as
   descriptions. A round-trip test proves it by relinking an `Iseq` into a second symbol
   table state and getting the same answers.
4. Rust unit tests for the compiler (shape of the emitted code for each control-flow form)
   and the interpreter (the value each form produces).
5. `cargo test` green, `cargo clippy` clean, no new global outside `src/shared/`.

## Tasks

- [x] `src/shared/symbols.rs` — append-only intern table, `intern`/`name`/`len`.
- [x] `src/bytecode.rs` — `Insn`, `Literal`, `Iseq`, `Iseq::link`, relative-jump helpers.
- [x] `src/compile.rs` — `spinel_ast` → `Iseq`; locals resolution, control flow, `case`.
- [x] `src/interp.rs` — non-recursive loop, `Frame`, `ruby_eq`, `BinOp`, `eval_in`.
- [x] `spec/harness` — matcher walk, shared frame, `passed`/`failed` counts, and a
      ranking of what blocked the rest.
- [x] `tests/eval.txt` + `scripts/eval-oracle.rb` — the answers, measured from CRuby.
- [x] `scripts/verify-passes.rb` — every claimed pass, re-run on CRuby. CI job.
- [x] Docs: engine.md's pipeline and conformance sections state what is now true.

## What the audit caught

- **`==` dispatched on the wrong thing.** The first `ruby_eq` tried to settle a
  pair by looking at *both* operands, and `nil == false` came out
  `NoDispatch`. Ruby dispatches `a == b` on `a`, and every immediate's `==` is
  identity once the numeric case is handled — so the left operand alone decides,
  and `nil == false`, `:a == 1` and `1 == "1"` are all simply false. Caught by a
  test written from Ruby's rule rather than from the implementation.
- **`begin ... end while c` is not an exception handler.** It arrives as
  `ExprKind::Begin` with no rescues, and the first cut blocked it along with real
  `begin`/`rescue`. A bare `begin` is a grouping, and Ruby's only do-while
  spelling; compiling it costs one guard and is worth eight examples.
- **A dead `Pop` loop.** `break` and `next` looked like they could leave a
  half-finished expression on the stack — `[1, (break 2)]` would leak a value per
  iteration — so the compiler unwound to the loop's depth before jumping. Asking
  Ruby settled it: `[1, (break 2)]` and `x = (next)` are *"unexpected void value
  expression"* at parse time. The loop could never run. It is now a
  `debug_assert`, which turns the same reasoning into a detector for a lowering
  bug rather than a silent compensation for one.
- **Unbounded roots in `==`.** `heap_kind` rooted its operand in the caller's
  scope to read the header, and a scope only pops on drop, so a loop comparing a
  thousand strings left a thousand roots behind. A nested scope pops each one
  immediately. The larger version of the same ceiling — objects allocated in a
  loop are not reclaimed until the evaluation ends — is a follow-up, not a fix
  here, because it needs the interpreter's stack to be a root source.

## Numbers

Measured on the pinned ruby/spec commit. Before is `main` at
[#150](https://github.com/ar4mirez/spinel/pull/150); after is this branch.

`scripts/spec.sh language`:

| | examples | passed | failed | blocked | skipped |
|---|---|---|---|---|---|
| before | 2735 | 0 | 0 | 2691 | 44 |
| after | 2735 | **155** | **0** | 2536 | 44 |

The five files the issue names:

| file | examples | passed | failed | blocked |
|---|---|---|---|---|
| `if_spec.rb` | 52 | 27 | 0 | 25 |
| `unless_spec.rb` | 6 | **6** | 0 | 0 |
| `while_spec.rb` | 37 | 21 | 0 | 16 |
| `until_spec.rb` | 28 | 20 | 0 | 8 |
| `case_spec.rb` | 51 | 18 | 0 | 32 |

Whole corpus, `scripts/spec.sh`: 25,624 examples, **164 passed, 0 failed**, in
0.4s.

Every one of those 164 was re-run on CRuby 4.0.6 by `scripts/verify-passes.rb`,
which slices each example back out of its spec file and executes it with a
four-line mspec shim. All 164 agree. That check is a CI job, because a pass
Spinel had no right to is worse than a blocked example and is the exact failure
mode a partial VM invites.

What the remaining `language/` examples are blocked by, most first:

| examples | blocked by | lands in |
|---|---|---|
| 1475 | a method call | [#11](https://github.com/ar4mirez/spinel/issues/11) |
| 280 | a method call with a block | #11 |
| 195 | `defined?` | [#13](https://github.com/ar4mirez/spinel/issues/13) |
| 99 | a method definition | #11 |
| 84 | `begin`/`rescue` | [#12](https://github.com/ar4mirez/spinel/issues/12) |
| 68 | `case`/`in` pattern matching | phase 2 |
| 60 | a class or module body | #13 |
| 53 | a multiple assignment | #11 |

Corpus-wide the shape is starker: 18,728 of 23,606 blocked examples are one
missing thing, a method call. #11 is not merely the next slice by roadmap order;
it is the next slice by a factor of seven over everything else combined.

## Follow-ups

- **`Array#<<` and a growable array.** The single most common shape in
  `if_spec.rb` is `a = []; if c; a << 1; end; a.should == [1]`. `<<` mutates, and
  the heap object it would grow is fixed-length, so it belongs with
  [#15](https://github.com/ar4mirez/spinel/issues/15)'s `core/array.rb` rather
  than being smuggled in as a twelfth specialised operator.
- **The interpreter's stack as a root source.** Everything allocated during an
  evaluation stays rooted until it ends, because a `HandleScope` only pops on
  drop. #7 already shaped `Heap::mark`'s `shade` to take root sources as disjoint
  borrows; the missing piece is a frame that outlives a function call, which is
  what #11 builds.
- **`defined?` is worth 195 examples in `language/` alone**, more than method
  definitions. It is currently filed under #13 with constants; that ranking says
  it deserves to be its own slice, and it needs almost nothing from #11.
- **Bignums.** Integer overflow, `IntValue::Big`, and floats outside flonum range
  are all `NoDispatch` today. Phase 2's `Integer`.
- **Serialising an `Iseq`.** The format is position-independent, which was the
  requirement; writing it to disk is phase 3's bytecode cache and `core.image`.
- **`===` for `Range`, `Class`, `Regexp`, `Proc`.** `when` refuses these rather
  than assuming `==`, which blocks 32 of `case_spec.rb`. They arrive with the
  classes themselves.
