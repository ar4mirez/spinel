# PRD 0039 — the exception constructors that are Ruby, and the two that are a platform table

Issue: [#29](https://github.com/ar4mirez/spinel/issues/29) · Phase 2 ·
`area:core-lib`

## Objective

Second slice on the order [PRD 0038](0038-phase-2-p0-order-and-the-lazy-chain.md)
measured. #29's reachable half is the exception classes CRuby gives an
`initialize` of their own; its other half is `Errno` and `SignalException`,
which are platform tables and are left refusing by name.

## Baseline

`core/exception`: 233 examples · 22 passed · 9%. `core/systemexit`: 6 · 0.
Corpus 2901, 0 failed.

The 27 examples blocked on "`new` on an exception class with its own
`initialize`" split by what the constructor needs:

| examples | classes | needs |
|---:|---|---|
| 14 | `SystemExit`, `NameError`, `NoMethodError`, `FrozenError`, `KeyError`, `NoMatchingPatternKeyError`, `SyntaxError` | nothing — ordinary Ruby |
| 9 | `SignalException`, `Interrupt` | a signal name/number table |
| 6 | `SystemCallError` | the errno table, with `Errno` |

## The engine change, and why it is one line of policy

`exceptions.txt` marks a class `# own initialize` and `Native::New` refuses it,
because Spinel had only `Exception#initialize` and going ahead would answer as
if it had validated arguments it never looked at. That refusal has to become
conditional now that `core/exception.rb` writes some of them.

**`Exception#initialize` moved into Ruby**, which is `docs/roadmap.md`'s rule —
if a method can be written in Ruby, it is — and is what lets a subclass reach it
with `super`. The VM still writes `@message` directly on the path where *it*
raises; this is only the path where a program calls `new`.

The refusal then keys on whether an `initialize` is owned **below `Exception`**,
not on whether one exists. Keying on existence looked right and was wrong the
first time it ran: once `Exception#initialize` is Ruby, every exception class
has one, and `SignalException.new(:NOSIG)` came off its refusal and started
succeeding — four specs went red and said so.

## What this slice writes

`Exception#initialize`, and then `SystemExit` (`status`, `success?`),
`NameError` (`name`, `receiver`), `NoMethodError` (`args`, `private_call?`),
`FrozenError` (`receiver`), `KeyError` (`key`, `receiver`),
`NoMatchingPatternKeyError` (`matchee`, `key`), `SyntaxError` (`path`), and
`StopIteration#result`.

Deliberately not written:

- **`SignalException` and `Interrupt`** need a signal name/number table. Still
  refusing by name.
- **`SystemCallError` and `Errno`** need an errno table. See below — this is a
  question for the owner, not a decision this slice should make.
- **`UncaughtThrowError`** was written and then removed. Its CRuby constructor
  takes `(tag, value)` and requires both — `UncaughtThrowError.new("x")` is an
  ArgumentError — and the VM's `throw` path raises it with a message only, so a
  Ruby `initialize` here would have accepted a call Ruby refuses and answered
  `nil` for `tag` and `value`. Carrying them needs the raise path to write two
  more ivars, the way it already writes `@name` for `NameError`.
- **`full_message`, `detailed_message`, `backtrace_locations`** (23 examples)
  need real source positions, which `core/exception.rb` already says at
  `backtrace` and PRD 0012 named a non-goal.

## The open question: how `Errno` gets its table

`Errno` blocks 210 examples under `core/`, but **only 32 are in
`core/exception`** — 167 are `core/dir`, where the `Dir` call that would raise
one does not exist either. The 32 plus `SystemCallError`'s 6 are what this
issue can actually reach.

What makes it more than a class list: `Errno::EINVAL::Errno` is the platform's
errno number, `Errno::EAGAIN` and `Errno::EWOULDBLOCK` **are the same class**
when the platform gives them the same number, and the default message is
`strerror(3)`. The numbers differ across the two platforms CI runs —
`EAGAIN` is 35 on macOS and 11 on Linux — so unlike `exceptions.txt` this table
cannot be one committed file measured once.

Three shapes, none obviously right:

1. **A generated table per platform** — `errno-linux.txt`, `errno-macos.txt`,
   each written by an oracle and `--check`ed by that platform's CI job. Matches
   the `exceptions.txt` pattern; adds a file per platform Spinel ever targets.
2. **A Rust primitive over `strerror`/`errno`** — the numbers and messages come
   from libc at run time, so nothing is committed and nothing can drift. Costs
   a primitive, and `docs/roadmap.md` asks that a core-library slice justify
   one; "the platform's own table" is arguably raw-memory-shaped.
3. **Generate at build time** from libc headers into a `build.rs` output. No
   committed table, no run-time primitive, but a build script that reads the
   host's headers is a new kind of dependency for this workspace.

Left to the owner — it is a settled-decisions-shaped question, and #29 cannot
close without it.

## Check

`scripts/spec.sh --platform=linux` 0 failed, `bench/spec-status.md` regenerated,
`scripts/verify-passes.rb` re-runs every claimed pass, `cargo test` green in
debug, `cargo clippy --all-targets` clean.

## Results

`core/exception` **22 → 31 of 233 (9% → 13%)**, `core/systemexit` **0 → 4 of 6
(67%)**, corpus **2901 → 2914**, 0 failed. All 31 claimed passes re-run on
ruby 4.0.6 agree. A 25-probe differential against CRuby agrees on 24; the 25th
is `Hash#inspect` printing `{:a => 1}` where Ruby 3.4+ prints `{a: 1}`, which is
[#216](https://github.com/ar4mirez/spinel/issues/216) and not this slice.

### What had to be measured rather than reasoned about

- **`SystemExit`'s two arguments are positional-but-either.** `SystemExit.new(1)`,
  `SystemExit.new("m")` and `SystemExit.new(2, "m")` are all valid, so the first
  argument is read by type rather than by position — and a `true` status is 0
  while `false` is 1, the shell convention, inverted from Ruby's truthiness.
- **An absent `receiver` is an ArgumentError, not nil.** `NameError.new("m")
  .receiver` raises "no receiver is available", so the ivar cannot simply
  default to nil and be read back — absent and `nil` are different states.
  Same for `FrozenError#receiver` and `KeyError#key`.
- **`SyntaxError`'s default message is "compile error"**, the one class where
  `Exception#initialize`'s class-name rule does not hold.
