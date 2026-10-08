# PRD 0057 — warnings

Issue: [#268](https://github.com/ar4mirez/spinel/issues/268) · Phase 2 ·
`area:core-lib`

## Objective

Spinel printed no warnings. ruby/spec checks them with mspec's `complain`
matcher, and 74 examples were tagged for it. This slice adds `Warning`, sends
every warning through it, and prints the ones the VM, the parser and the core
library owe.

## Baseline

Corpus 14208 passed, 0 failed. `core/warning` 0 of 31, all blocked on
`uninitialized constant Warning`. 74 tags naming #268.

## Design

**One exit.** `Warning.warn(text)` writes to `$stderr`, and everything ends
there: `Kernel#warn`, the core library, the VM and the parser. A program that
redefines it hears all four. `Warning[]`, `Warning[]=` and `categories` hold
CRuby's four categories and their defaults. `Kernel#warn` builds its text by
`IO#puts`'s line rules, honours `uplevel:` and `category:`, and passes
`category:` on only when `Warning.warn` is not exactly one-argument, which is
CRuby's rule and needs the one new reflection primitive here, a method's
arity.

**`Kernel#__warning__(message, verbose, category)` is `rb_warn`.** It decides
whether to print — `$VERBOSE` nil prints nothing, a verbose warning needs
`true`, a category has to be on — and writes `file:line: warning: message`
at the line of the program that called in. The position is the first entry
of the backtrace, which already reports a core frame at its caller's line.

**The core library warns in Ruby.** `given block not used` on `all?`, `any?`,
`none?`, `one?`, `count`, `find_index`, `index`, `rindex`, `inject` and
`Array.new`; `block supersedes default value argument` on `Array#fetch`,
`Hash#fetch`, `ENV.fetch` and `Array.new`; `too many arguments for format
string`; `optional boolean argument is obsoleted`; `$, is set to non-nil
value` and its `$;` twin; `circular require considered harmful`. Each is one
line where CRuby has one, with the level CRuby gives it, measured.

**The VM warns by pushing a frame.** `vm_warn` sends `__warning__` to the
`Kernel` module object as a frame whose value is discarded — the hook
mechanism #28 built, with its "only when overridden" filter split off as
`call_discarded`. Used for `already initialized constant`, `global variable
'$x' not initialized` (checked against `$VERBOSE` before the frame, so a read
of a set global costs nothing; only for a name a program made up, and not
for `$a ||= 1`, which the compiler now guards as it does a class variable),
`Regexp.new`'s `flags ignored`, and a
deprecated constant.

**Some globals are methods.** A heap records which global names are hooked.
An assignment to one is `Kernel#__global_assign__(name, value)`, which
type-checks, warns and stores through `__global_store__`; a read of `$=` is
`__global_read__`. That gives `$,`, `$;`, `$/`, `$\` and `$-0` their
deprecation warning and their `TypeError`, makes `$VERBOSE = 1` store `true`,
and adds the `$-0` alias that was missing.

**Parser warnings arrive with the tree.** The parser hook now answers the
program and its warnings, each already a line of text with its level. `eval`
and `load` push one frame per warning on top of the frame they are about to
run, so the warnings print first and in order. Prism's Rust binding does not
expose a warning's level, so `spinel_parse::is_verbose_warning` is Prism's
default-level list by message. Prism is also not told a source is an `eval`
string and calls its last statement void; Ruby reads that value, so that one
warning is dropped.

**`deprecate_constant` marks.** The class table keeps a list of deprecated
`(module, name)` pairs, empty in almost every program. A constant read, a
`const_get` and a `remove_const` warn under `:deprecated`.

## Not in this slice

Thirteen of the 74 tags name a warning this slice does not print, and each
now names the issue that owns it:

- Seven are chilled strings (#263).
- Four are a block passed to a method that never uses it, which needs the
  compiler to record whether a body can use its block (#273, filed here).
- One is the regexp compiler's `nested repeat operator` (#33).
- One is an Integer literal in a flip-flop: the warning prints now, and the
  example then compares against `$.` (#41).

Also not here: `previous definition of X was here`, the second line of the
constant warning, which needs where a constant was assigned (#128); and the
main script's own parser warnings, which `spinel run` still renders itself.

## Results

Corpus 14208 to **14302** passed, 0 failed. `core/warning` 0 to 22 of 31,
`language` 2133 to 2159, `core/array` 2657 to 2670, `core/module` 619 to 628,
`core/kernel` 1056 to 1063. Tags naming #268: 74 to 0, with thirteen
renamed to the issue that owns them and one added for #273.

`tests/eval.txt` holds twelve new rows, measured on ruby 4.0.6.
