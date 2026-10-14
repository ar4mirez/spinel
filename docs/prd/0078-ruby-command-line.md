# PRD 0078 — `ruby`'s command line

Issue: [#62](https://github.com/ar4mirez/spinel/issues/62) · Phase 3 ·
`area:cli`

## Objective

Scripts, gems and spec helpers run Ruby by shelling out to `RUBY_EXE` with
`ruby`'s flags: mspec's `ruby_cmd` writes `-e "code"`, `--disable-gems`,
`-W0 -n -l`, `-I fixtures`, `-r...`. `spinel` answered all of them with
clap's "unexpected argument". After this slice it answers them as `ruby`
does.

## Design

**Which form.** A first word that starts with `-` — except `-h`, `--help`
and `-V`, which stay `spinel`'s own — is a `ruby` command line; so is
`spinel script.rb`. Subcommands (`run`, `parse`) are unchanged.

**Parsing is `proc_options`.** Short flags combine (`-wn`, `-lane`); a flag
that takes a value takes the rest of its word or the next word, where `ruby`
allows the next word; `--` and the first word that is not a flag end the
options; what follows the program is `ARGV`. `-0` keeps its digits as
written, because `-0` (NUL) and `-00` (paragraph mode) are different.
Feature switches with nothing behind them in Spinel — `--disable-gems`,
`--enable=…`, `--yjit` — are accepted and ignored. Anything else is `ruby`'s
`invalid option` error, exit 1.

**Meaning is Ruby.** The flags become a prelude, compiled and run in the
program's heap before it: `$VERBOSE` for `-w`/`-W`/`-v`, `$DEBUG`,
`Encoding.default_*` for `-E`, `$/` for `-0`, `$;` for `-F`, `$\` for `-l`,
`$LOAD_PATH` for `-I`, `require` for `-r`, and `-s`'s switches as globals.
`-n` and `-p` wrap the program in `while gets; …; end`, opened on the
program's first line so every line keeps its number; `-a` and `-l` add
their split and chomp inside it. `-C` changes directory before the program
is read, `-x` skips to the `#!…ruby` line, `-c` parses and says `Syntax OK`,
and `-` reads the program from standard input.

**`Kernel#gets`** reads the files named in `ARGV`, each taken off as it is
opened, then standard input when there were none: the reading half of
`ARGF`, which is all `-n` needs. `ponytail:` `ARGF` itself is #41's.

## Check

`crates/spinel-cli/tests/ruby_flags.rs`, thirteen tests over a fixture
directory, each expectation measured on ruby 4.0.6 with the same arguments.
`command_line/` does not move yet: `ruby_exe` runs its command through
`IO.popen`, which is #43, next.
