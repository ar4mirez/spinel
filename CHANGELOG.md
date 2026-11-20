# Changelog

Notable changes to Spinel, newest first. There are no releases yet, so every
entry is under Unreleased. A change marked **Breaking** changes something a
tool outside the engine can see: a command's output, a file format, or the
bytecode a tool reads.

## Unreleased

### Added

- **`puma-spinel`, the puma gem for Spinel; the Sinatra fixture serves on Puma**
  ([#116](https://github.com/ar4mirez/spinel/issues/116)).
  `gems/puma-spinel/` is Puma 8.1.2's Ruby API, upstream's `lib/` and
  `bin/` unchanged, over a `spinel-ext` extension that runs Puma's own HTTP
  parser state machine (the tables Ragel makes of upstream's grammar) and
  makes MiniSSL's OpenSSL calls. `spinel install` puts it in place of a
  locked `puma 8.1.2` when `SPINEL_SUBSTITUTES=puma`, leaving the lockfile
  as Bundler wrote it; the Sinatra fixture serves on it beside
  nio4r-spinel, and Puma's own test suite ends test for test as it does on
  CRuby 4.0.9, but for 33 tests neither run runs (rubygems.org access, a C
  compiler, MRI-only checks). `gems/puma-spinel/build.rb` builds
  `puma-spinel-8.1.2.1-<platform>.gem`; it is not on rubygems.org yet.

- **`nio4r-spinel`, the nio4r gem for Spinel**
  ([#115](https://github.com/ar4mirez/spinel/issues/115)).
  `gems/nio4r-spinel/` is nio4r 2.7.5's Ruby API, upstream's `lib/`
  unchanged, over a `spinel-ext` extension that makes the epoll (Linux) or
  kqueue (macOS) calls libev makes for upstream, plus `poll(2)` for the
  `:poll` and `:select` backends; a `select` waits through Spinel's
  scheduler, so other threads run meanwhile. `spinel install` puts it in
  place of a locked `nio4r 2.7.5` when `SPINEL_SUBSTITUTES=nio4r`, leaving
  the lockfile as Bundler wrote it, and nio4r's own test suite, all 112
  examples, runs green on it under RSpec 3.13.
  `gems/nio4r-spinel/build.rb` builds `nio4r-spinel-2.7.5.1-<platform>.gem`;
  it is not on rubygems.org yet.

- **`openssl` TLS, so `net/https` works**
  ([#107](https://github.com/ar4mirez/spinel/issues/107)).
  The built-in openssl 4.0.2 gains keys, certificates and TLS on the
  system's OpenSSL 3: `PKey` (`PKey::RSA.new(2048)`, `PKey.read`, sign and
  verify), `X509::Certificate` built and `#sign`ed, `Name`, `Extension`,
  `Store`, `SSLContext` and `SSLSocket`, blocking and nonblocking, so
  `Net::HTTP` with `use_ssl` fetches an HTTPS page. Certificate
  verification is CRuby's: off on a bare context, on with `set_params`
  against the system's trust store with CRuby's host name check, and a
  failure raises CRuby's `SSLError`. `verify_callback`, `servername_cb`,
  `alpn_select_cb` and `client_cert_cb` run Ruby mid-handshake; the
  callbacks OpenSSL cannot pause for (`session_get_cb`,
  `session_remove_cb`, `renegotiation_cb`, `tmp_dh_callback`, NPN, a
  server's `verify_callback`) raise `NotImplementedError`. `Cipher` and
  `KDF` are not there yet.

- **`pg-spinel`, the pg gem for Spinel**
  ([#114](https://github.com/ar4mirez/spinel/issues/114)).
  `gems/pg-spinel/` is pg 1.7.0's Ruby API, upstream's `lib/` unchanged,
  over a `spinel-ext` extension linked with the system's libpq, as
  upstream's source gem is (only building that extension needs libpq; it is
  not a default workspace member).
  `spinel install` puts it in place of a locked `pg 1.7.0` when
  `SPINEL_SUBSTITUTES=pg`, leaving the lockfile as Bundler wrote it, and a
  fixture connects to PostgreSQL and round-trips a query through it.
  `gems/pg-spinel/build.rb` builds `pg-spinel-1.7.0.1-<platform>.gem`; it
  is not on rubygems.org yet. Values come back as Strings, as under
  upstream's default type map; the other type maps, the coders and notice
  callbacks raise `NotImplementedError`.

- **`sqlite3-spinel`, the sqlite3 gem for Spinel**
  ([#113](https://github.com/ar4mirez/spinel/issues/113)).
  `gems/sqlite3-spinel/` is sqlite3 2.10.2's Ruby API, upstream's `lib/`
  unchanged, over a `spinel-ext` extension with SQLite 3.54.1 compiled in,
  the release upstream bundles. `spinel install` puts it in place of a
  locked `sqlite3 2.10.2` when `SPINEL_SUBSTITUTES=sqlite3`, leaving the
  lockfile as Bundler wrote it, and a Sinatra app reads and writes SQLite
  through it. `gems/sqlite3-spinel/build.rb` builds
  `sqlite3-spinel-2.10.2.1-<platform>.gem`; it is not on rubygems.org yet.
  Callbacks from SQLite into Ruby (`create_function`, aggregates,
  collations, `trace`, busy handler blocks) raise `NotImplementedError`.

- **`bigdecimal`**
  ([#109](https://github.com/ar4mirez/spinel/issues/109)).
  `require "bigdecimal"` is bigdecimal 4.0.1, the version Ruby 4.0 bundles,
  built into `spinel`: `BigDecimal()` from Strings, Integers, Floats and
  Rationals, arithmetic at a precision or to the precision limit in every
  rounding mode, `round`, `floor`, `ceil` and `truncate`, NaN, the
  infinities and signed zeros with the exception modes, `to_s` in every
  format, Marshal, `sqrt`, `**`, `BigMath`, and `bigdecimal/util`'s `to_d`.
  Results, printed digits and error messages are CRuby's, down to the word
  count `_dump` writes. A lockfile pinning `bigdecimal` installs the
  built-in. `[0] == [obj]` now asks `obj`, as `0 == obj` does;
  `Rational#coerce` takes a Complex; and `Rational(x)` calls `x.to_r` for
  a number of another class.

- **`psych` and `yaml`, on libyaml**
  ([#108](https://github.com/ar4mirez/spinel/issues/108)). `require "yaml"`
  is psych 5.3.1, the version Ruby 4.0 bundles, over libyaml 0.2.5, the
  release CRuby's Psych links, built into `spinel`: `YAML.load`,
  `safe_load` and `unsafe_load` with their permitted classes, symbols and
  aliases, `YAML.dump` writing what CRuby writes byte for byte,
  `Psych::Parser` and its handlers with libyaml's events and marks,
  `Psych::Emitter`, `Psych::Stream` and the node tree. `YAML.load` is as safe
  as Psych 4 makes it, aliases load as shared objects rather than copies, and
  loading a document nested more than 10,000 collections deep is refused
  with a `Psych::SyntaxError`, while a `Psych::Parser` handing events to a
  handler that builds nothing parses any depth. A lockfile pinning `psych`
  installs the built-in. A `Set` made with `Set.allocate` is now an empty
  Set, as on CRuby.

- **`fiddle`, on `Spinel::FFI` and libffi**
  ([#105](https://github.com/ar4mirez/spinel/issues/105)). `require "fiddle"`
  is fiddle 1.1.8, the version Ruby 4.0 bundles: load a C library, declare
  its functions, call them, lay out C structs with `Fiddle::Importer`, work
  with `Fiddle::Pointer`, and hand C a `Fiddle::Closure` that calls a Ruby
  block — `qsort` comparing in Ruby, a block raising out of C, callbacks on
  several threads at once. `require "spinel/ffi"` gives the layer under it,
  `Spinel::FFI`, directly. libffi is built into `spinel`, so nothing needs to
  be installed. Spinel never hands C an address inside its own heap: a String
  argument is a copy, written back into the String when the call returns.
  A lockfile pinning `fiddle` installs the built-in. `Class#new` now runs an
  `initialize` defined with `define_method`.

- **`require` loads Spinel native extensions**
  ([#104](https://github.com/ar4mirez/spinel/issues/104)). A `.spinel` file
  `require` finds — `require "foo"` with no `foo.rb`, or `require "foo.so"` —
  is opened, checked against this `spinel`'s extension ABI, and initialised,
  and recorded in `$LOADED_FEATURES` once. An extension built for another ABI,
  or a file that is not one, is a `LoadError` saying why, raised before any of
  its code runs. A gem ships its extension built in its `lib/`, as a
  precompiled native gem does; `crates/spinel-cli/tests/fixtures/native-gem`
  is the example. A CRuby `.so` is still refused.

- **`spinel-ext`: the native-extension API and its versioned ABI**
  ([#103](https://github.com/ar4mirez/spinel/issues/103)). An extension is a
  Rust `cdylib` built against the `spinel-ext` crate alone, naming its init
  with `spinel_ext::extension!`; its crate documentation is the worked
  example. The ABI the extension was built against is embedded in a frozen
  header and checked before any of its code runs, and a mismatch is a
  `LoadError` saying what to do, never a crash. `require` loading a
  `.spinel` file is #104. `docs/engine.md`, "Native extensions and FFI", has
  the details. **Breaking** for compiled apps' `manifest.json`:
  `spinel_ext_abi` is now 1.

- **A compiled app runs** ([#101](https://github.com/ar4mirez/spinel/issues/101)).
  An app `spinel build --compile` writes extracts its payload once, to
  `$XDG_CACHE_HOME/spinel/apps/<sha256>/` (`~/.cache` by default), and runs
  its entry point with the app's gems, environment and warmed bytecode cache;
  later runs reuse the extracted copy without reading the payload. Every
  argument is the app's: `mytool --help` is the app's help, never Spinel's.
  `docs/build.md`, "Runtime behavior of a compiled app", has the details.
  **Breaking** for `--dump-bytecode`: `__FILE__` is a `SourceFile` literal
  rather than a `Str`, so the bytecode cache can move a compiled file.

- **`spinel build --compile <entry> -o <app>` packs a project into one
  executable** ([#100](https://github.com/ar4mirez/spinel/issues/100)). The
  app is this `spinel` with a payload of the project's files (as
  `.gitignore` keeps them, `--include`/`--exclude` on top), the gems of the
  lockfile's `default` group and each `--group` as real copies, a warmed
  bytecode cache and a manifest (entry point, version, `spinel-ext` ABI,
  `--env`, and the arguments after `--`). The same project builds the same
  bytes. `docs/build.md`, "The payload", has the details.

- **`spinel test` keeps each file's time and hands out the slowest first**
  ([#96](https://github.com/ar4mirez/spinel/issues/96)). After a run, each
  test file's time is kept in `.spinel/test_timings.json`; the next run hands
  out files never timed first, then the rest longest first, each to the
  worker given the least so far, so the workers finish together. A timings
  file that is missing, unreadable or stale only leaves files untimed; one
  that cannot be written is told on stderr and does not fail the run.

- **`spinel test --reporter json` and `--reporter junit`**
  ([#95](https://github.com/ar4mirez/spinel/issues/95)). `json`, for tools,
  prints each event as one JSON object per line as it arrives, then a
  `problem` event for each crash or file that raised loading, and a
  `summary` event with the counts. `junit`, for CI, prints one JUnit XML
  document in the shape Jenkins' schema gives and GitLab and GitHub
  Actions' reporters read: a `testsuite` per file and a `testcase` per test
  or example, with its `file` and `line`, and a `failure`, `error` or
  `skipped`; each crash is a `testcase` with an `error`. Under either,
  stdout holds only that, and what tests print goes to stderr. The exit
  status is the default report's. This takes the place of
  `SPINEL_TEST_EVENTS`.

- **`spinel test` reports the run and prints commands to re-run failures**
  ([#94](https://github.com/ar4mirez/spinel/issues/94)). It prints a dot per
  passing test, then each failure and error with its `file:line`, message and
  backtrace, each run of gem frames folded to one line, then a summary line
  and a `spinel test file:line # name` command for each failure that runs it
  again when pasted into a shell. It exits 1 on any failure, error or crash,
  0 otherwise. `spinel test FILE[:LINE]...` runs only the files given, a line
  picking the test or example defined at or nearest above it. This report
  takes the place of Minitest's and RSpec's own, and each JSON event gains a
  `rerun` field naming where to run it again.

- **`spinel test` workers report each test as a JSON event**
  ([#93](https://github.com/ar4mirez/spinel/issues/93)). A Minitest reporter
  and an RSpec formatter, registered in every worker, write one line per test
  or example to the runner, `{"event":"pass|fail|error|skip","file":..,
  "line":..,"name":..,"ms":..,"message":..,"backtrace":[..]}`, each line
  whole however many workers write at once. Minitest's skips, errors and
  failures are told apart as its own summary tells them; RSpec's pending and
  skipped examples are skips and every other failing example a failure.
  `spinel test --reporter json` prints them (#95).

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
