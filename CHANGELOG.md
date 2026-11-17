# Changelog

Notable changes to Spinel, newest first. There are no releases yet, so every
entry is under Unreleased. A change marked **Breaking** changes something a
tool outside the engine can see: a command's output, a file format, or the
bytecode a tool reads.

## Unreleased

### Added

- **`spinel test` workers report each test as a JSON event**
  ([#93](https://github.com/ar4mirez/spinel/issues/93)). A Minitest reporter
  and an RSpec formatter, registered in every worker, write one line per test
  or example to the runner, `{"event":"pass|fail|error|skip","file":..,
  "line":..,"name":..,"ms":..,"message":..,"backtrace":[..]}`, each line
  whole however many workers write at once. Minitest's skips, errors and
  failures are told apart as its own summary tells them; RSpec's pending and
  skipped examples are skips and every other failing example a failure. A
  failing test now fails the run even when its worker exits 0.
  `SPINEL_TEST_EVENTS=path` appends every event to `path`.

- **`require "prism"` and `require "ripper"` work, so RSpec reports
  failures** ([#837](https://github.com/ar4mirez/spinel/issues/837)). The
  prism gem's Ruby half is vendored at the release Spinel's parser links
  (1.9.0, in place of the 1.8.1 copy in Ruby's `lib/`) and reads trees
  serialized by that same library, so `Prism.parse`, `Prism.lex` and the
  rest of the extension's API answer what CRuby's do with prism 1.9.0.
  `ripper` is Prism's Ripper translation, reading its source as CRuby's
  Ripper does, so an unmodified RSpec suite with a failing example prints
  CRuby's report and exits 1. Where Prism's translation is not yet parse.y's
  (a few lexer states, two tree shapes) Spinel answers as Prism does.

- **`spinel test` runs the Minitest suite on forked workers**
  ([#92](https://github.com/ar4mirez/spinel/issues/92)). It finds
  `test/**/*_test.rb`, requires `test/test_helper.rb` once, then forks `-j N`
  workers (the physical cores by default) that take one file at a time, so
  what the helper boots is booted once. A worker that dies mid-file is
  reported with that file and replaced, and the run exits 1. RSpec files under
  `spec/` run after them since
  [#837](https://github.com/ar4mirez/spinel/issues/837): `rspec/core` and
  `spec/spec_helper.rb` are required once, and each worker runs its files'
  examples through RSpec's own runner.

- **`spinel install` uses Spinel's built-in gems for the ones a lockfile
  pins** ([#83](https://github.com/ar4mirez/spinel/issues/83)). A lock
  pinning `json`, `stringio`, `strscan`, `date`, `digest`, `zlib`, `etc`,
  `io-console` or `fcntl` installs the built-in in its place: the gem is not
  fetched and its `ext/` is not built, the GEM_HOME gets the locked name and
  version so `Bundler.setup` activates it, `.spinel/substitutions.json`
  records it, and the lockfile is unchanged. A minor version apart from the
  built-in warns; a major version apart fails the install before anything is
  written.

- **The require map: `require` passes over the gem directories that do not
  have a file** ([#66](https://github.com/ar4mirez/spinel/issues/66)).
  `spinel install` writes `.spinel/require_map.bin`, what every installed
  gem's `require_paths` and the standard library hold, and programs in the
  project read it: a load path entry the map lists is passed over without a
  look on disk when it does not have the file. Entries it does not list,
  `-I` and a program's own `$LOAD_PATH` edits are searched as before, in
  the same order, and a directory changed since the map was written is
  looked at on disk, so the map changes how fast `require` answers, never
  what. On a project of 100 gems a run of 608 requires goes from about 5 s
  to about 0.8 s. The hidden `spinel require-map <file> [dir...]` writes one
  for directories given.

- **An existing `Gemfile.lock` keeps its versions, as Bundler keeps them**
  ([#79](https://github.com/ar4mirez/spinel/issues/79)). When no update is
  asked for, a Gemfile that asks for nothing new resolves to the lockfile
  itself without reading the index, and the lockfile is written back byte
  for byte, even one from an earlier Bundler 4. Otherwise each locked gem
  the edit does not touch is tried at its locked version first and moves
  only when the rest needs it to: up, or down only when a conflict loosens
  its floor, as Bundler 4.0.20 does. The
  hidden `spinel resolve --locked Gemfile.lock` runs it.

- **`Gemfile.lock`, read and written as Bundler does**
  ([#80](https://github.com/ar4mirez/spinel/issues/80)). A lockfile Bundler
  4 wrote reads back and writes out byte for byte, GIT and PATH sections,
  `CHECKSUMS` and `RUBY VERSION` included; a new one written from a
  resolution is the one `bundle lock` writes, down to the extra platforms
  Bundler adds for native gems, and `bundle install` leaves it unchanged.
  `BUNDLED WITH` is 4.0.20. A dependency on `bundler` resolves to that
  version. The hidden `spinel lockfile <file>` reads one and writes it back;
  `spinel resolve --lockfile` prints a new one.

- **A project's GEM_HOME, linked from the gem store**
  ([#82](https://github.com/ar4mirez/spinel/issues/82)). Stored gems are
  linked into `.spinel/gem_home/` as `gem install` lays one out:
  `gems/<gem>` a symlink into the store, `specifications/<gem>.gemspec` the
  file RubyGems itself writes, byte for byte, and `bin/` stubs that start
  `#!/usr/bin/env spinel`. RubyGems, `Bundler.setup` and `bundle exec` read
  it unchanged. Linking again writes nothing, and a gem no longer asked for
  is removed. The hidden `spinel fetch --gem-home DIR` runs it. A program
  whose `#!` line names `spinel` now runs as Ruby, as one naming `ruby` does.

- **The compact index client**
  ([#77](https://github.com/ar4mirez/spinel/issues/77)). The first piece of
  `spinel install`: rubygems.org's `/versions`, `/names` and `/info/<gem>`,
  cached under `~/.spinel/index/<host>/` (`$SPINEL_HOME/index/` when set)
  and kept current as Bundler keeps its copy — a `Range` request for what was
  appended, checked against the server's SHA-256 before it is kept, a full
  refetch when it does not match, and no `/info` request for a gem whose
  cached file is current. At most 32 requests are in flight. The hidden
  `spinel index [--source URL] [--names] [gem...]` runs it.

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
  US-ASCII or BINARY too, but UTF-8 under `-E UTF-8` or `RUBYOPT=-EUTF-8`,
  and transcoded into the internal encoding `-E ext:int` names, as `ruby`
  does; its elements are frozen under every locale. A program that reads UTF-8
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
