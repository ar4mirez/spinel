# PRD 0051 — String `eval` and `Binding`

Issue: [#38](https://github.com/ar4mirez/spinel/issues/38) · Phase 2 ·
`area:engine`

## Objective

Compiling at run time: `Kernel#eval` of a String, `Kernel#binding` and the
`Binding` it answers, and the String forms of `instance_eval`, `class_eval`
and `module_eval`. mspec evaluates strings and passes bindings around, so
this is the first of #145's prerequisites.

## Baseline

`core/binding` 0 / 53, `core/kernel` 289 / 1277, `core/basicobject` 65 / 108,
`core/module` 444 / 1021, `language` 1470 / 2464. Corpus 6546, 0 failed.

## Design

**The parser is a hook.** `spinel-vm` does not depend on Prism and still
does not: `Heap` holds a `Parser` function pointer that `spinel_core::boot`
installs, and `eval` without one is refused. `spinel_parse::parse_eval`
takes the `file` and `lineno` arguments — `SourceMap` gained a first line, and
`__LINE__` and backtrace lines became signed, because `lineno` can be zero or
negative.

**Prism does not know the caller's locals.** `ruby-prism` exposes no parse
options, so a string is parsed as a file: a bare `a` arrives as a method call
and `a = 1` declares a new local. `compile::eval` takes the local names of
every environment the string runs inside and resolves against them — a bare
name the caller has is a local read, and a name Prism declared that the caller
already has is the caller's. Prism's copy keeps its slot, so no index moves,
under a name Ruby cannot spell. Block parameters are always their own.

**A body knows its outer names.** An environment at run time is only slots,
so every block `Iseq` now records the local names of the scopes around it —
the compiler already built that list to resolve depths, and `finish` moves it
into the `Iseq` instead of dropping it.

**A `Binding` is a frame's snapshot.** Eleven slots: the body naming its
locals, the environment, `self`, the lexical scope, the block, the frame a
`return` leaves, file and line, the method's owner and name (for `__method__`
and `super`), and the visibility a `def` takes. `eval` runs the string in a
new environment inside the binding's; when the string declared a local, the
binding moves to that environment, so the next `eval` through it sees the
local and the frame it came from does not. `local_variable_set` of a new name
does the same with a one-slot environment. `Kernel#eval` is Ruby: it captures
its caller's frame with `__caller_binding__` and converts its arguments.

**`yield` inside the string** is valid when the binding belongs to a method,
unless a `class` or `module` body in the string encloses it; Prism's
diagnostic is dropped for exactly those.

**A string `instance_eval`** resolves constants in the receiver's singleton
class if it already has one and in its class if not, then in the caller's
scopes, and class variables in the caller's alone — measured on 4.0.7. A
constants-only scope node under the eval node `def` goes through is that
rule. A string `class_eval` is the class body reopened.

## Found on the way

- **A default that reads its own parameter** — `def m(a = a)` — answered
  the binder's undef marker; the parameter is now nil while its default runs.
- **`exit` and `abort`** only raise `SystemExit`, so they are Ruby;
  `exit!`, `fork` and `system` exist and refuse, naming `Process` (#43).
  `core/kernel/fixtures/classes.rb` makes them public, and now gets as far as
  `Kernel.instance_method` (#27), which is what blocks most of
  `eval_spec.rb`.
- **Three newly reachable examples are someone else's**, tagged: `Hash#inspect`
  evaluated back (#216), an unknown POSIX bracket class (#33), and `+@` on a
  chilled literal — filed as #263.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/binding` | 0 / 53 | **47 / 53** |
| `core/kernel` | 289 / 1277 | **346 / 1277** |
| `core/basicobject` | 65 / 108 | **85 / 108** |
| `core/module` | 444 / 1021 | **460 / 1021** |
| `language` | 1470 / 2464 | **1611 / 2464** |
| **corpus** | 6546 | **6858** |

0 failed; all passes re-run on ruby 4.0.7 agree.

## What is left of #38

`Binding#irb`, `Kernel#eval` through a `Proc#binding` (#27), and converting
`instance_eval`'s file and line with `to_str`/`to_int`: the native refuses
anything that is not already a String or an Integer.
