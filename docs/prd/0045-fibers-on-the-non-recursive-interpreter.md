# PRD 0045 — Fibers on the non-recursive interpreter

Issue: [#16](https://github.com/ar4mirez/spinel/issues/16) · Phase 2 ·
`area:engine`

## Objective

`core/fiber/` newly passes; the interpreter loop is non-recursive; a fiber
switch's cost is in `bench/`. PRD 0038 called this the hinge of the Phase 2
`P0` set: `Enumerator#next` (#26), mspec (#145), and — through the re-entrant
half — #28's hooks and `define_method` wait on it.

## Baseline

`core/fiber` 0 / 124, 86 of them blocked on `Fiber` not existing. Corpus 4178,
0 failed.

## Design: a fiber is a pair of vectors

The interpreter has never recursed on the Rust stack for a Ruby call: a call is
a `Call` pushed onto `frames`, its operands on `stack`, and the loop carries on.
Deep recursion was already safe — `def f(n) = n.zero? ? 0 : 1 + f(n - 1);
f(200_000)` runs to the end where CRuby raises `SystemStackError`.

So a fiber is a second `(stack, frames)`, and switching is swapping which pair
the loop is running. Every native is handed both vectors by `&mut`, so
`resume`, `Fiber.yield`, `raise`, `kill` and `transfer` are natives that do the
swap with two `mem::take`s, and nothing above them knows a switch happened. No
Rust coroutine is involved: the `corosensei` half of this issue's title is for
*primitives* that must call Ruby mid-operation (a definition hook, the binder
calling `to_ary`), which is #28's work and stays there.

- **`FiberTable`, per heap**, holds every fiber: its object, its block, its
  state (created, resumed, suspended with its vectors, resuming another,
  terminated) and its resumer's vectors. Per heap rather than per evaluation,
  because the spec harness runs an example one statement at a time and a fiber
  made by one statement is resumed by the next. Suspended vectors are traced by
  the collector — a fifth root source.
- **A fiber's last frame ending** is `resume` returning in its resumer, with the
  block's value. The three places the loop treated "no frames left" as the end
  of the run (`Leave`, and the two exits of `unwind_to_handler`) now ask
  `leave_fiber` first. An exception reaching a fiber's last frame keeps
  unwinding in the resumer; `break` and `return` out of a fiber's block become
  CRuby's LocalJumpErrors there; `kill` is a throw tagged with the fiber
  itself, which runs every `ensure` and no `rescue` on its way out.
- **Frame ids are heap-wide now.** They named frames uniquely within one
  evaluation, and a fiber resumed by a later evaluation holds frames from an
  earlier one.
- **`transfer`** follows CRuby: a transferred fiber that ends returns to the
  root — or, when the root is part-way through resuming a fiber that
  transferred away, to that fiber. A fiber suspended by `Fiber.yield` cannot be
  transferred to; one entered by transfer cannot be resumed.
- **`Fiber#raise` on a fiber that is resuming another** reaches the innermost of
  that chain: raised from inside the chain it is the caller's own `raise`.

`core/fiber.rb` is the Ruby around the primitives: argument packing (none is
nil, one is itself, several are an Array), `blocking:`, `Fiber.blocking?`
answering `1`, storage (`Fiber[]`, inherited as a copy, Symbol keys only, nil
deletes), and `inspect` with the block's location — `Iseq` now records the line
a body starts on, read through a `__proc_location__` primitive.

Every message and rule was measured against ruby 4.0.7, and a 30-line
differential produces CRuby's output.

## Found on the way

- **`raise "msg", backtrace`** — a String with a second argument — is a
  TypeError in CRuby and was a RuntimeError here. Fixed in the shared
  `raise_argument`.

## Cost

`bench/fiber_switch.rs`: a resume/yield round trip inside a Ruby loop, net of
a block call. On the machine this was written on, **4.3µs in Spinel against
124ns for `ruby --yjit`**. Most of it is not the swap: it is the two Ruby
wrappers in `core/fiber.rb`, the argument Arrays they build, and the `loop`
around the `Fiber.yield`. Calling the primitives without the wrappers is the
first optimisation when a benchmark asks for one.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/fiber` | 0 / 124 | **72 / 124** |
| **corpus** | 4178 | **4252** |

0 failed; all passes re-run on ruby 4.0.7 agree. What blocks the rest: the fiber scheduler API (`set_scheduler`,
`scheduler`, 19), `Thread` (#45, 7), `define_singleton_method` (#28), and
mspec's helpers (#145).
