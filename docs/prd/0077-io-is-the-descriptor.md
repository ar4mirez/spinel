# PRD 0077 — `IO` is the descriptor

Issue: [#41](https://github.com/ar4mirez/spinel/issues/41), second slice ·
pulled forward from Phase 3 · `area:runtime`

## Objective

`core/io` and `core/file` held 1,776 blocked examples between them, the
largest share of any two directories under `core/` and on their own enough
to carry `core/` past the Phase 2 milestone's 80%. PRD 0058 had made `File` a
real descriptor; `IO` was still only the three standard streams, so
everything a spec opens through `IO` itself — `IO.sysopen`, `IO.new(fd)`,
`IO.pipe`, `IO.read` and the class-level readers and writers — had nothing
under it. This slice moves the descriptor from `File` to `IO`, where CRuby
has it, and fills in what the ranking named.

## Baseline

`core/io` + `core/file`: 733 passed, 0 failed, 1,776 blocked of 2,542.
Corpus: the `bench/spec-status.md` on `main`.

## Design

**`IO` is the descriptor; `File` opens one.** The read buffer, `gets` with
its limits and paragraph mode, `read`, `seek`, `pos`, `write` and the rest
move from `File` to `IO` unchanged. `File#initialize` reads the mode with
`IO#__modeenc__`, opens a path and hands the descriptor to `IO#__open_fd__`,
the two halves of `IO#__setup__`, which every way of making an IO shares:
`IO.new` checks its descriptor with `fcntl(F_GETFL)` and refuses a mode the
descriptor does not allow (`EINVAL`, measured), `IO.pipe` makes two without
calling `new` (measured: a class that redefines `new` is not asked), and the
standard streams are made at boot without checking, since a process may
start with one closed.

**`$stdout` is a descriptor like any other.** It wrote through
`Kernel#__write__` and Rust's buffered stdout; now every write is one
`write(2)` on fd 1, and `__write__` is gone. `puts` joins its lines into one
write, as `rb_io_writev` does, except for a `write` that takes one argument —
the interface before Ruby 2.5 — which gets them one at a time.

**Encodings are CRuby's two slots.** `@enc` is what reads answer in, nil for
the default external as it is when asked; `@enc2`, when there is one, is
what the bytes are in. `rb_io_ext_int_to_encs` fills them, at open, from the
mode's `:ext:int`, the options, binmode or the defaults, in that order;
`set_encoding` refills them branch by branch as `io_encoding_set` does;
`external_encoding` and `internal_encoding` read them back. A read with
`@enc2` converts a whole read or a whole line with `String#encode`.
`ponytail:` a stream decoder is the upgrade, for when a line is split across
a conversion.

**Fourteen new primitives,** each one system call but the table of
`fcntl`'s command numbers, which are the platform's and read from libc as
the open flags are: `pipe`, `dup`, `dup2`, `pread`, `pwrite`, `fcntl` with
an integer argument, `flock`, `fsync` and `fdatasync`, `truncate` and
`ftruncate`, `link`, `mkfifo`, `utimensat`, `chown` and `lchown`. Beside
them, `__fs_stat__` given a descriptor is `fstat`, `isatty` takes any
descriptor, and `access` asks by the effective ids for `File.readable?` and
by the real ones for `readable_real?`.

**What a path adds.** `File.fnmatch` is `dir.c`'s `fnmatch_helper`,
`fnmatch` and `bracket` ported over characters, with `ruby_brace_expand`
for `FNM_EXTGLOB`. `File::Stat` gains its times, `<=>`, device numbers and
the permission questions of `rb_stat_r` and friends. `File.utime`, `lutime`,
`chown`, `truncate`, `link`, `mkfifo`, `split`, `identical?`, and the
`File.pipe?` family asked of a path that may not exist. `Dir.home` reads
`HOME` and then `/etc/passwd`, and `expand_path` takes `~user`.
`ponytail:` users only that file lists; NSS is `etc`'s (#111).

**Two built-in libraries,** `fcntl` and `io/nonblock`, each a few lines in
`lib/` over primitives `IO` already uses, because fixtures load them.

## What running these specs found elsewhere

- **The compiler's own references to core classes were lexical.** A
  multiple assignment converts its right side through `Array`, and keywords
  through `Hash`, looked up as a bare constant. Inside a `BasicObject`
  subclass that lookup fails — and mspec's `should.raise` is one: every
  `raise` expectation that was *not* met reported `uninitialized constant
  SpecPositiveOperatorMatcher::Array` and counted as blocked. They are `::X`
  now. Thirty-odd examples in these two directories were failures that could
  not say so.
- **`IO#dup` shared its descriptor.** `Kernel#dup` copied `@fileno`, so the
  copy and the original closed one descriptor twice, and the second close
  could land on whatever had been opened in between — in one run, mspec's
  own output. `initialize_copy` now `dup(2)`s it.
- **A `to_path` was not held to answering a String,** so a mock whose
  `to_path` was 1 opened descriptor 1, wrote a spec file's contents to the
  terminal and closed it. `File.__path__` also refuses a NUL and an
  ASCII-incompatible encoding now, as `rb_get_path` does.
- **`format`'s Ruby 3.4 wordings.** A `%` before a newline, a NUL or the end
  of the string after a flag is `malformed format string`.

## Not in this slice

`IO.popen` and `open("|cmd")` are processes (#43). `IO::Buffer` (177
examples) is a class of its own. `IO.select` and nonblocking pipes need a
`poll` primitive and waiting in the VM. There is still no write buffer, so
`sync` is recorded, `flush` has nothing to do and `syswrite` has nothing to
warn about. `reopen` keeps the receiver's class, where CRuby changes it.
Each of these is tagged or blocked with its reason.

## Results

| | passed | failed | blocked |
|---|---:|---:|---:|
| `core/io` + `core/file` before | 733 | 0 | 1,776 |
| this slice | **2,112** | **0** | 423 |
| corpus before | 20,597 | 0 | 10,634 |
| this slice | **22,043** | **0** | 9,114 |

`core/` as a whole goes from 74.9% to **81.0%**, past the Phase 2
milestone's 80%.

Compared example by example with `main`, 1,414 go from blocked to passing,
and none that passed fails. Four that passed are blocked now: they read a
file with an `euc-jp:utf-8` mode, and passed because the old `File` never
converted. Now the conversion runs and reaches #19's refusal of the
table-driven encodings.

**Tags.** Twenty-one written for the stand-in `File` this replaces now pass
and are deleted. Six new ones in these two directories name their cause: no
write buffer (two), nonblocking pipes, `reopen`'s class change,
`rb_check_funcall`'s `method_missing` fallback, and a path checked against
the external encoding.

A hundred more, across the corpus, are the `should.raise` expectations the
`::Array` fix lets report. Every one was blocked on `main` and is a real
disagreement: an error Spinel does not raise yet (`String#byteslice`'s
range checks, `Time`'s argument checks, `Regexp.union`'s encodings and the
rest), or a library that does not exist yet (`ObjectSpace`, `ARGF`, `Logger`,
`IPAddr`). Each is tagged with the issue that owns its class.
