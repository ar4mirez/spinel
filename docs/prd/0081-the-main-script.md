# PRD 0081 — the main script: `BEGIN`, `END`, `DATA`, `TOPLEVEL_BINDING`, and how it ends

Issue: [#314](https://github.com/ar4mirez/spinel/issues/314), second slice ·
Phase 3 · `area:cli`, `area:engine`

## Objective

The things only a main script has, which `ruby_exe` let ruby/spec ask about
for the first time: the compiler refused `BEGIN` and `END`, `DATA` and
`TOPLEVEL_BINDING` did not exist, and an exception that ended a program was
printed as one line with no place in the file.

## Design

**`BEGIN { }`** bodies move to the front of the program when it is compiled,
in order, in the program's own scope: CRuby runs them first and their locals
are the program's.

**`END { }`** compiles to a call to `Kernel#__end_block__` with the `END`'s
offset in the file and the body as its block; the first call from a place
registers the block as an exit handler and later ones do nothing, so an
`END` in a loop runs once. Prism resolves the body's names as though no
scope opened, as it does a `for` body's, so the block's environment is
marked synthetic the way `for`'s is.

**`-n` and `-p`** put a program that parses on its own in their loop after
parsing, with its `BEGIN`s outside it; a program that does not parse alone
(a top-level `next`) is wrapped as text, as before.

**`DATA`** is the main script's file, opened in the default external
encoding at the byte after its `__END__` line, defined after `-r`'s files
and before the script runs. `ponytail:` the `__END__` line is found by line,
not by Prism's token, so one inside a heredoc counts.

**`TOPLEVEL_BINDING`** is the `binding` of the main script's own frame,
recorded by a call the CLI puts first in the program.

**How a program ends.** The exception that ended it is printed by Ruby,
`full_message` with its cause and backtrace, after the `at_exit` handlers,
as CRuby's top level prints it; a `SystemExit` of any class and a signal
other than `Interrupt` print nothing. The CLI adds its one hint, that a
`NoMethodError` may be Spinel's, and reports the engine's own refusals as
before.

## Not in this slice

`TOPLEVEL_BINDING` before the main script runs, which `-r` files see in
CRuby, and the dynamic variables they set on it: CRuby runs the main script
inside that binding. `END` in a method's warning, which is a parse-time one.
`--backtrace-limit`, frozen string literals and RubyGems.

## Results

Corpus 22,767 to **22,809** passing, 0 failed. Compared example by
example with `main`, none that passed fails. `BEGIN` in an `eval` string is
hoisted in that string too. Two examples that `TOPLEVEL_BINDING` lets run
for the first time disagree and are tagged with their owners: a
`SyntaxError` from `eval` without its path (#38), and `main.ruby2_keywords`
(#200).
