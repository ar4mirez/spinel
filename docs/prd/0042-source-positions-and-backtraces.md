# PRD 0042 — Source positions, and the backtrace built from them

Issue: [#29](https://github.com/ar4mirez/spinel/issues/29) · Phase 2 ·
`area:core-lib`, engine surface

## Objective

The last slice of #29. Its definition of done asks for `caller_locations` with
correct file, line and label, and backtraces that survive a re-raise. PRD 0012
named source positions a non-goal, and `Exception#backtrace` has answered nil
since. PRD 0041 listed what that blocks: `backtrace_locations`,
`full_message`, `detailed_message`, `caller`, `caller_locations`.

## Baseline

`core/exception` 53 / 233. `core/kernel` 119, `core/thread` 0, corpus 3030,
0 failed.

## The engine change

**Lines, compiled in.** A `Span` is a byte offset and only the parser has the
bytes, so `Program` now carries a `SourceMap` — the path and the offset every
line starts at, which the parser already computed for `__LINE__`. The compiler
keeps a current line and appends `(pc, line)` to a new `Iseq::lines` whenever
an emitted instruction's line differs. A call is placed at its **name's** line,
not its receiver's: measured, `x = Foo\n  .new\n  .inst` raising in `inst`
reports the third line. `Iseq` also gains its `path` and a `block_level`.

**The backtrace is the frame stack at the raise.** Every exception starts
unwinding in one place in the interpreter loop, with the raising frame still on
the stack; `attach_backtrace` takes it there, once — a re-raise keeps the
first. Each frame's line is its iseq's line at `pc - 1`, since `pc` has already
moved past the instruction it is in.

**Labels are CRuby 3.4's:** `Foo#bar`, `Foo.bar` (a singleton of a named
module), a bare `bar` (an anonymous owner), `block (2 levels) in Foo#bar`,
`<main>`, `<class:Foo>`. The owner comes from the frame; a block's base is the
body it was written in, which the compiler now makes its iseq's name instead of
`block in <compiled>`. Class bodies are `<class:Foo>` / `<module:Foo>` rather
than `<class>`.

**Core frames print as CRuby 3.4 prints C methods.** `core/*.rb` is now parsed
as `<internal:array>` and friends. A core frame appears only where the program
called it, under its own label at the *caller's* position — so
`[2].map { raise }` is "block in <main>", "Array#map", "<main>", not the `each`
and the block `core/array.rb` builds `map` from. A primitive that raises gets
the same treatment at the native dispatch site (`Integer.sqrt(-1)` shows
`Integer.sqrt`).

**`cause`** is decided at the same place: `$!` when the exception is first
raised, unless `raise ..., cause:` named one. **`raise Klass, msg, backtrace`**
takes its third argument.

## The Ruby

`core/exception.rb`: `backtrace_locations`, `set_backtrace` (Strings, a String,
nil, or — 3.4 — Locations, which also set `backtrace_locations`), `cause`,
`detailed_message`, `full_message` in both orders with `highlight:`, and
`Exception.to_tty?`. `Kernel#caller` and `#caller_locations` with a start, a
length or a Range, as module functions. `core/thread.rb` gives
`Thread::Backtrace::Location` (`path`, `lineno`, `label`, `base_label`,
`absolute_path`, `to_s`, `inspect`). `$@` compiles to the `$!&.backtrace` it
means. Three primitives: `__backtrace_here__`, `__stderr_tty__`,
`__absolute_path__`.

Every format string was measured against ruby 4.0.7 — two probe scripts, one
for backtraces and labels, one for `full_message`/`detailed_message`/`cause`,
produce CRuby's output except where noted below.

## Things this surfaced

- **`Kernel#inspect` was `#<Object>`.** CRuby's is `#<Object:0x... @a=1>`, with
  a guard for an object that holds itself and 4.0's
  `instance_variables_to_inspect`. Written; `Object`'s own copies of `to_s` and
  `inspect` deleted, since CRuby defines both on `Kernel`.
- **A module defined under an anonymous one was called plain `N`.** It is
  `#<Module:0x...>::N` now. CRuby renames it once the outer module is assigned
  a constant; that rename is marked `ponytail:` for #28.
- **`RuntimeError.new(nil).message` was "nil".** It is the class name.
- **`puts [1, [2]]`** printed the Array's `to_s`; it prints one element per
  line, `[]` prints nothing, and a self-containing Array prints `[...]`.
- **`Thread` had to exist** for the location class's constant path. Starting
  one refuses through a new `Unknowable` primitive rather than a Ruby
  `NotImplementedError`: four `core/thread` specs expect `ThreadError` or
  `ArgumentError`, and read a Ruby exception as a wrong answer.

## The harness

`spec/harness` learned mspec's `x.should =~ /re/` — every backtrace example
asserts with it — and `scripts/verify-passes.rb` learned the same matcher in
the same commit, so its new passes are checked. The harness passes each spec
file's `SourceMap` to the compiler, and names its frames `<top (required)>`,
the `base_label` mspec's example frames have.

`scripts/verify-passes.rb` had to become more faithful in the same way. It
joined an example's slices and `eval`ed them from line 1, which CRuby then
answered with line 1 and `<main>` — so it reported two of this slice's passes as
false, when Spinel's answers were the ones real mspec gives. It now places each
slice on its original line and `load`s the result from a file at the spec's
relative path, which is how mspec runs a spec file. All 3094 passes re-run and
agree.

Two examples are tagged for #145: each defines a method inside an example and
asserts its label is `foo`. mspec's `instance_exec` makes that a singleton; this
harness runs examples on `main`, where it is `Object#foo`.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/exception` | 53 / 233 (23%) | **78 / 233 (33%)** |
| `core/kernel` | 119 / 1277 | **134 / 1277** |
| `core/thread` | 0 / 363 | **21 / 363** |
| `core/module` | 109 / 1021 | **112 / 1021** |
| **corpus** | 3030 | **3094** |

0 failed.

## What is left in #29

- **`BinOp` raises have no C-method frame.** `1 / 0` compiles to an operator
  instruction, not a dispatch, so its backtrace lacks CRuby's `'Integer#/'`
  line. The frame below it is right.
- **`Errno.constants`** is `Module#constants` (#28).
- **`SystemCallError.new("m", 2.9)`** needs `Float#to_int` (#18).
- **`UncaughtThrowError.new`** still refuses.
- **`raise Klass, msg, cause:`** on a class with a Ruby `initialize` does not
  take the keyword yet: the exception exists only once that `initialize`
  frame leaves.
- **`String#inspect` does not escape `\e`** (#19), so a highlighted
  `full_message` *prints* differently from CRuby's while being the same string.
