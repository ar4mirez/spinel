# PRD 0082 — frozen literals, hash seeds, and the rest of what a program shows

Issue: [#314](https://github.com/ar4mirez/spinel/issues/314), third slice ·
Phase 3 · `area:cli`, `area:engine`

## Objective

The smaller things from #314's list that each take one change: a switch
that freezes string literals, a hash that differs between processes, the
locale's charmap, a limit on printed backtraces, and the parser's warnings
about the main script.

## Design

**`--enable=frozen-string-literal`** is a heap setting the compiler reads for
every program it compiles after it is set — the main script, each file
`require` loads, and each `eval` string — and nested scopes inherit it. A
literal takes it only when its file says nothing; `# frozen_string_literal:
false` and `+"..."` still win.

**`Object#hash` is seeded per process** (CVE-2011-4815). The keys are a
`RandomState` made once per process in `spinel-vm/src/shared/hash_seed.rs`:
immutable once made, so every Ractor in the process hashes alike, as
CRuby's do.

**`Encoding.locale_charmap`** is `nl_langinfo(CODESET)` for the locale the
environment names, read once and cached, as CRuby reads it at startup; the
C locale is restored right after.

**`--backtrace-limit=N`** shows N lines after the first and then CRuby's
` ... M levels...`, in `full_message` and at the top level;
`Thread::Backtrace.limit` answers it.

**Warnings.** The parser's warnings about the main script go through
`Warning.warn` as a required file's do, so `END` in a method warns, and
`-w` shows the verbose ones. A top-level `return` with an argument warns
that it is ignored. `ponytail:` when it runs, where CRuby's is the parser's.

## Not in this slice

RubyGems and `did_you_mean` (`defined?(Gem)`), chilled strings (#263),
`-n`'s integer ranges against `$.`, and `TOPLEVEL_BINDING` as `-r` files
see it.

## Results

Corpus 22,809 to **22,830** passing, 0 failed; none that passed fails.
Twenty-one tags go, ten of them on `DATA` and `TOPLEVEL_BINDING` examples
that the last slice made pass and left tagged. Three on chilled strings move
to #263, which owns them.
