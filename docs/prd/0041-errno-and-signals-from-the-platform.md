# PRD 0041 — `Errno` and signals, from the platform

Issue: [#29](https://github.com/ar4mirez/spinel/issues/29) · Phase 2 ·
`area:core-lib`

## Objective

Second slice of #29. [PRD 0039](0039-exception-initialize-in-ruby.md) wrote the
exception constructors that are plain Ruby and left two refusing by name,
because both need a platform table: `SystemCallError`/`Errno` (error numbers and
`strerror`) and `SignalException`/`Interrupt` (signal numbers). It also left
**how** `Errno` gets its table as a question for the owner.

The owner answered: **a Rust primitive over libc** (PRD 0039's option 2). The
numbers and messages come from the platform at build and run time, and nothing
platform-specific is committed as data.

## Baseline

`core/exception`: 233 examples · 31 passed · 13%. `core/signal`: 47 · 0.
Corpus 3002, 0 failed. Top rows in `core/exception`: `Errno` 32, "`new` on an
exception class with its own `initialize`" 17.

## What this slice writes

### `Errno` — `crates/spinel-vm/src/errno.rs`

- **Names are CRuby's and fixed.** CRuby defines one `Errno` constant per entry
  of `known_errors.def` on every platform, 158 of them, and aliases a name the
  platform lacks to `Errno::NOERROR`. The list is in the file.
- **Numbers are the `libc` crate's.** Three `cfg` lists (common, Linux-only,
  Apple-only) name the constants; the numbers are `libc::EAGAIN` and so on for
  the target being built. A name in the wrong list is a compile error on that
  target, not a wrong number. The lists were derived by `cargo check`ing all
  157 names against `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`.
- **Messages are `strerror(3)`**, through `std::io::Error` (which picks the
  right one of glibc's two `strerror_r`s) with `std`'s " (os error N)" removed.
- **Bootstrap** defines `Errno`, one `SystemCallError` subclass per distinct
  number carrying its `Errno` constant, and an alias constant for every other
  name with that number. The first name alphabetically names the class, as in
  `known_errors.def`: on Linux `Errno::EWOULDBLOCK` *is* `Errno::EAGAIN`.
- **Two primitives:** `__strerror__(n)` and `__errno_class__(n)`. Everything
  else is Ruby in `core/exception.rb`: `SystemCallError.new` picks the `Errno`
  class for a number before allocating, which is how Ruby spells CRuby's
  "change the object's class inside `initialize`"; `initialize` builds
  `"<strerror> @ <func> - <message>"`; `errno` reads it back.

Measured against CRuby rather than read out of `error.c`, and all agree:
`SystemCallError.new(0)` is an `Errno::NOERROR` whose message is "Success" (0
*is* in the lookup); `func` appears only beside a message; a lone Integer is
the errno, not the message; an out-of-`int` errno is a `RangeError` at
`strerror`, not at the class lookup.

### Signals — `crates/spinel-vm/src/signal.rs`, `core/signal.rb`

The same decision applied to the same shape of problem. CRuby's `siglist` names
in CRuby's order, numbers from `libc`, and `NSIG` — which the `libc` crate does
not export — as a per-target constant. One primitive, `__signal_list__`.

`Signal.list`, `Signal.signame`, `SignalException#initialize`, `#signo`,
`#signm` and `Interrupt#initialize` are Ruby. Order matters twice: `Signal.list`
iterates in it, and `Signal.signame(6)` is "ABRT" rather than "IOT" because
`ABRT` comes first. `glibc` spells `SIGCLD` as a macro for `SIGCHLD` that the
`libc` crate does not export, so the Linux list spells it out.

### `raise Klass, msg` now runs `Klass#initialize`

Ruby's `raise Klass, msg` is `Klass.exception(msg)`, which is `new(msg)`.
Spinel built the exception directly and wrote `@message`, which is right only
for a class with no `initialize` of its own. With `Errno::ENOENT` written in
Ruby that became visible — `raise Errno::ENOENT, "x"` answered "x" rather than
"No such file or directory - x" — but it was already wrong for **every user
exception class with its own `initialize`**.

A native cannot push a frame and then raise, so the `initialize` frame carries
a new flag, `raises_receiver`, beside `keeps_receiver`: when it leaves (or
`return`s), the object below its base is raised instead of answered. Same
shape as `Class#new`, and no Rust recursion.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/exception` | 31 / 233 (13%) | **53 / 233 (23%)** |
| `core/signal` | 0 / 47 | **6 / 47 (13%)** |
| **corpus** | 3002 | **3030** |

0 failed. `scripts/verify-passes.rb` re-ran the claimed passes on ruby 4.0.7
and all agree. Two hand-written differentials — every constructor shape and
error message above, plus `raise` through a Ruby `initialize` with and without
an explicit `return` — produce output identical to CRuby's.

## What is left in #29

- **`Errno.constants`** (18 examples) is `Module#constants`, #28's reflection
  surface.
- **`full_message`, `detailed_message`, `backtrace_locations`, `caller_locations`**
  need real source positions, which PRD 0012 named a non-goal. That is #29's
  last slice and has engine surface: the compiler has to record lines.
- **`SystemCallError.new("m", 2.9)`** reaches `Float#to_int`, which is #18's.
- **`SystemCallError.===`** compares errno numbers in CRuby. Not written:
  `rescue` refuses a class with its own `===` (`Insn::CheckMatch` cannot make a
  Ruby call), so defining it would block every `rescue Errno::X`. The ancestry
  test gives the same answer unless an exception's class and its `errno`
  disagree, which only a user subclass that sets `@errno` by hand can arrange.
- **`UncaughtThrowError`** still refuses `new`: its constructor takes
  `(tag, value)` and the VM's `throw` path would have to write both.
- **Windows** (#139) has its own errno and signal subsets; both files carry a
  `ponytail:` naming that.
