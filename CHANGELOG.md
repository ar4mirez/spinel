# Changelog

Notable changes to Spinel, newest first. There are no releases yet, so every
entry is under Unreleased. A change marked **Breaking** changes something a
tool outside the engine can see: a command's output, a file format, or the
bytecode a tool reads.

## Unreleased

### Added

- **The single-byte code pages**
  ([#41](https://github.com/ar4mirez/spinel/issues/41)). `encode`,
  `Encoding::Converter` and an IO's encodings convert to and from
  ISO-8859-2 to -16, KOI8-R and KOI8-U, Windows-1250 to -1257 and
  Windows-874, the IBM and CP code pages from 437 to 869, the Mac ones and
  TIS-620, by CRuby's own tables, so `File.open(path, "r:Windows-1252:UTF-8")`
  reads a legacy file as it does under `ruby`, and a character a page lacks is
  CRuby's `Encoding::UndefinedConversionError`, named as CRuby names the
  converter: `U+3042 from UTF-8 to WINDOWS-1252`.

- **Blocking calls that let the other threads run, and end when asked**
  ([#41](https://github.com/ar4mirez/spinel/issues/41)). `File#flock`,
  `fsync`, `fdatasync`, opening a FIFO, and reading or writing a blocking
  pipe or terminal Spinel did not make leave the other threads running while
  they wait, and `Thread#kill`, `Thread#raise`, a signal, closing the IO in
  another thread (`IOError: stream closed in another thread`) and the
  program's end cut the wait short, as under `ruby`. A program writing to a
  full standard output pipe ends on SIGTERM, and Ctrl-C ends a `flock` wait.
  SIGVTALRM is Spinel's own, as it is Ruby's.

- **Nonblocking pipes**
  ([#41](https://github.com/ar4mirez/spinel/issues/41)). The ends of
  `IO.pipe` and `IO.popen` are `O_NONBLOCK`, as CRuby's are, so
  `nonblock?` is true for them, and a write bigger than the pipe holds waits
  for room without stopping the other threads: a thread reading the pipe
  drains it, and a signal ends the wait. A child program is given blocking
  standard streams, which leaves the parent's end of the same pipe blocking
  afterwards, as under `ruby`.

- **A write buffer for `IO`**
  ([#41](https://github.com/ar4mirez/spinel/issues/41)). An IO that is
  neither `sync` nor a terminal holds up to 8 KiB of what it writes, as
  CRuby's does, so `$stdout` to a pipe or a file reaches it when it is
  flushed, by `flush`, a read, a seek, `close`, `fork`, `system` or `spawn`,
  `p`, or the program's end, rather than at each `write`; `exit!` loses it.
  A program that writes lines to a pipe another process reads as they come
  sets `sync = true` on it, as under `ruby`. `docs/roadmap.md` has the rest.

- **RubyGems and `did_you_mean` at boot**
  ([#319](https://github.com/ar4mirez/spinel/issues/319)). Every program,
  from `spinel run` or `ruby`'s command line, starts with `Gem` and
  `DidYouMean` loaded, as `ruby` does, unless `--disable-gems` or
  `--disable=did_you_mean` says otherwise; `shims/gem_prelude.rb` is CRuby's
  `gem_prelude.rb`. A feature flag may name its feature by a prefix
  (`--disable=gem`). `RbConfig::CONFIG` gains CRuby's install directories,
  under the Spinel home, and is no longer frozen; `$LOAD_PATH` gains its
  `sitelibdir`, before the stdlib. `Thread.each_caller_location`,
  `String#codepoints` and `String#each_codepoint` came with it, as
  `bundled_gems.rb` and `did_you_mean` call them. `docs/cli.md` has the rest.

- **The bytecode cache** ([#40](https://github.com/ar4mirez/spinel/issues/40)).
  Inside a project, a required file's compiled bytecode is kept in
  `.spinel/bytecode/<sha256 of source>.sbc` and read back on the next run
  instead of parsing. The file format is new and private to one build of
  Spinel: its header carries a fingerprint of the compiler, and a file
  written by another build is ignored and rewritten. `docs/cli.md` has the
  rest.

### Breaking: the default external encoding follows the locale

- **`Encoding.default_external` is the locale's**
  ([#41](https://github.com/ar4mirez/spinel/issues/41)), as under `ruby`, not
  UTF-8 whatever the environment. Under any UTF-8 locale nothing changes.
  Under `LANG=C`, `POSIX`, or no `LANG` at all, as in many Docker images,
  `Encoding.default_external`, `Encoding.find("locale")` and
  `find("filesystem")` are US-ASCII, so `File.read` returns US-ASCII strings,
  `$0` and `ENV` are US-ASCII (BINARY where their bytes are not ASCII), and
  `Dir.pwd` is BINARY. `ARGV` is in the default external encoding, so it is
  US-ASCII or BINARY too, but UTF-8 under `-E UTF-8` or `RUBYOPT=-EUTF-8`;
  its elements are frozen under every locale. A program that reads UTF-8
  files in such an environment sets `LANG=C.UTF-8`, passes `-E UTF-8`, or
  names the encoding when it opens the file, as it would under `ruby`.

### Breaking: bytecode format

- **Superinstructions** ([#746](https://github.com/ar4mirez/spinel/issues/746),
  [#750](https://github.com/ar4mirez/spinel/issues/750)). `Insn` has five new
  variants: `GetLocalPushIntBinOp`, `GetLocalGetLocalBinOp`,
  `GetLocalGetIvarBinOp`, `GetLocalBinOp` and `PushSelfGetLocal`. Each is
  named for the run of instructions it stands for. Once a body is compiled,
  `bytecode::fuse` writes each over the first instruction of its run, and
  leaves the rest of the run in place after it, so no `pc` moves.
  - `spinel run --dump-bytecode` prints them, followed by the instructions they
    stand for.
  - A tool that reads `Iseq::insns` must read a superinstruction as its run's
    first instruction (`Insn::head`), or know the new variants.
  - Nothing on disk changed then, because the bytecode cache `docs/cli.md`
    describes was not built yet. Its files (#40) carry a fingerprint of the
    compiler, so a later change to the format makes them stale rather than
    wrong.
  - What each one answers is in `docs/engine.md` (Bytecode), and how to read
    the dump is in `docs/cli.md` (`--dump-bytecode`).
