# PRD 0080 — the environment, and the rest of the switches

Issue: [#314](https://github.com/ar4mirez/spinel/issues/314), first slice ·
Phase 3 · `area:cli`

## Objective

#43 let mspec's `ruby_exe` start Spinel, and `command_line/` ran for the
first time: about a hundred examples were tagged for switches #62 parsed and
did not act on, and for the environment `ruby` reads. This slice is that
half of #314.

## Design

**`RUBYOPT`** is parsed with the command line's own routine after it, so
its `-I` and `-r` come after theirs. A word without a `-` is given one; only
the switches that change how a program runs are allowed (`-d -E -I -K -r -U
-v -w -W`, `--debug`, `--disable`, `--enable`, the encodings), and anything
else is `invalid switch in RUBYOPT` with exit 1, measured. `-v` there prints
the banner and goes on. `--disable=rubyopt` and `--disable=all` turn it off.

**`RUBYLIB`** goes on `$LOAD_PATH` after every `-I`, kept as written.
**`-S`** looks for a bare script name on `RUBYPATH`, then `PATH`. A script
whose `#!` line names something other than ruby is read as if `-x` had been
given, and `-x dir` changes directory before the script is read, as `-C`
and its old name `-X` do.

**Encodings.** `-K` sets the main script's source encoding, when it has no
magic comment, and the default external encoding with it: the CLI writes it
into the parsed program's source map before compiling. `-U` sets the
default internal encoding to UTF-8; two switches naming different internal
encodings are an error that names the first, measured. `--encoding`,
`--external-encoding` and `--internal-encoding` take their value as the
next word too, and `--encoding a:b:c` is `extra argument for --encoding`.

**The switches that are globals.** `-a`, `-l` and `-p` set `$-a`, `$-l` and
`$-p`; `-d` sets `$DEBUG`, its other name `$-d`, and `$VERBOSE`; `-0` sets
a frozen `$/`. `-w` and the levels meaning it turn `Warning[:deprecated]`
on.

**Features.** `--enable`/`--disable` with `=list`, `-name` or a separate
word, `all` included, are recorded; an unknown one is `ruby`'s two-line
warning naming the features it has. Only `rubyopt` acts yet.

## Not in this slice

`--enable=frozen-string-literal` needs a default every compile of every
file reads; `gems` and `did_you_mean` need RubyGems and `did_you_mean`
loaded; `BEGIN`/`END`, which `-n` examples use, `DATA`, `TOPLEVEL_BINDING`
and top-level backtraces are #314's next slices.

## Check

Five more integration tests in `crates/spinel-cli/tests/ruby_flags.rs`,
measured on ruby 4.0.6, and the `command_line/` tags this removes.

## Results

Corpus 22,689 to **22,767** passing, 0 failed. Compared example by example
with `main`, 77 tagged examples now pass, one blocked one passes, and none
that passed fails.
