# CLI and runtime plumbing

What `spinel` does outside the engine: how it runs a file, how it reports itself to Ruby code, and how the small Ruby shims glue the tooling to the VM.

## Subcommands

| command | does |
|---|---|
| `spinel run <file> [args]` | run a file on the VM with the project's gems visible |
| `spinel run --watch <file> [args]` | run it again whenever the project changes; see `--watch` below |
| `spinel run --dump-bytecode <file>` | compile the file and print its bytecode instead of running it; see below |
| `spinel <file.rb>` | same as `run`. Subcommand names win: `spinel install` is the subcommand even if `install` exists as a file; use `spinel run install` or `spinel ./install` for the file. |
| `spinel -e CODE`, `-r LIB`, `-I DIR`, `-w`, `-v`, … | `ruby`'s command line, parsed as `ruby.c`'s `proc_options` parses it: `-e`, `-r`, `-I`, `-w`/`-W[level]`/`-W:category`, `-v`, `-d`, `-n`/`-p`/`-l`/`-a`/`-F`, `-0[octal]`, `-s`, `-x[dir]`, `-C dir`, `-c`, `-E`, and `-` for a program on standard input; `RUBYOPT`, `RUBYLIB` and, for `-S`, `RUBYPATH` are read as `ruby` reads them (#314). Feature switches Spinel has nothing behind (`--disable-gems`, `--enable=…`, the JITs) are accepted and do nothing. Any first word that starts with `-`, other than `-h`, `--help` and `-V`, means this form; a flag `ruby` does not have is its `invalid option` error (#62). |
| `spinel x <gem>[@req] [args]` | run a gem's executable from an ephemeral GEM_HOME under `~/.spinel/x/`, project untouched |
| `spinel init` | in the current directory: Gemfile, `.ruby-version` containing `spinel-<version>`, `test/`, `.gitignore` entry for `.spinel/`. Never overwrites: an existing Gemfile, `.ruby-version` or `test/` is kept as it is, and an existing `.gitignore` gets the `.spinel/` line appended only when no line already names `.spinel` (with or without slashes, or negated). Prints one line per path saying whether it was created, appended to, or already existed. |
| `spinel install/add/remove/update` | package-manager.md |
| `spinel test` | test-runner.md |
| `spinel build` | build.md |
| `spinel parse <file>` | print the `spinel_ast` tree for one file; `--format debug` for the derived `Debug`. Exits 1 on a syntax error, 2 if the file cannot be read. |
| `spinel parse <dir>` | lower every `.rb` file under a directory and report only what failed. Exits non-zero on an unhandled node, never on a syntax error, because a corpus may legitimately contain invalid Ruby. This is the check behind the `sweep` CI job. |
| `spinel spec [dir]` | run ruby/spec (development only, hidden from `--help` in release builds) |

## What `spinel run` sets up

Before user code, and before any `-r` file, the VM runs the embedded `shims/setup.rb`. `spinel run` and `spinel <file>` run it; `ruby`'s command line (`spinel -e`, `spinel -w app.rb`) does not, as `ruby` looks for no project.

1. Walk up from the current directory to find `.spinel/gem_home/`. If found, set `GEM_HOME` and `GEM_PATH` to it, as `bundle exec` does for a `BUNDLE_PATH`, and put its `bin/` first on `PATH` (once, so a nested `spinel` does not grow it). They are set in the environment, so child processes see the same gems and executables; RubyGems reads them when it first needs its paths, and a `Gem` already loaded is told with `Gem.paths=`. RubyGems does not load at boot yet (#319), so today the environment is what carries the gem home. Outside a project nothing changes.
2. Load `.spinel/require_map.bin` if present (see below). Not yet: #66.
3. Register the bytecode cache directory, `.spinel/bytecode/` beside the gem home (#40), so `require` reads and writes it. Outside a project there is none, and nothing is read or written.

Bin stubs in `.spinel/gem_home/bin/` start with `#!/usr/bin/env spinel` and call `Gem.activate_bin_path`, the same shape RubyGems generates. `Gem.ruby` returns the absolute path of the running `spinel` binary so anything that spawns "the current Ruby" spawns Spinel.

## Bytecode cache and require map

- `.spinel/bytecode/<sha256 of source>.sbc` holds compiled bytecode (#40, `crates/spinel-vm/src/sbc.rs`). `require`, `require_relative` and `load` check it before parsing a file from disk, and write it after compiling one. The format is position-independent: symbols are stored by name and relinked on load, so a cache file is valid across processes and Ractors; what a read gives back is the same immutable, `Send + Sync` `Iseq` the compiler makes.
  - A file is used only when its header matches: the format, a fingerprint of the compiler (a hash, made by `crates/spinel-vm/build.rs`, of the sources of `spinel-vm`, `spinel-parse` and `spinel-ast` and of `Cargo.lock`), the source's hash, the path the source was compiled under — `__FILE__`, `__dir__` and backtraces carry it — and the `frozen_string_literal` setting. Then the payload's length and SHA-256 are checked, and the payload is decoded with every length and tag checked. Anything else — another build's file, a truncated or corrupted one — is parsed past as if there were no cache, and overwritten.
  - The parser's warnings about the file are kept in the entry and given again on a hit, so a run with the cache prints what a run without it prints. A file that does not parse, or that the compiler refused part of, is never written.
  - Writes go to a temporary file in the same directory and are renamed into place, so processes sharing a cache never read half a file. A file that cannot be written is a slower next run, never an error.
  - The main script, `-e` and standard input are compiled every time, so `--dump-bytecode` and syntax errors in them read as before; and a file shipped inside the binary is not the project's to cache.
  - `// ponytail:` two files with the same bytes at different paths share one cache file name, and each run that loads both rewrites it for the other. The upgrade, if a project shows it, is the path's hash in the name.
- `.spinel/require_map.bin` maps feature name to absolute path for every file under every installed gem's `require_paths` plus the stdlib. `require` consults it first and falls back to a normal `$LOAD_PATH` search on a miss, so the map can be stale without being wrong. `spinel install` regenerates both.

## How Spinel identifies itself

| constant | value |
|---|---|
| `RUBY_ENGINE` | `"spinel"` |
| `RUBY_ENGINE_VERSION` | Spinel's own version, e.g. `"0.3.0"` |
| `RUBY_VERSION` | the Ruby language version Spinel implements, `"4.0.0"` at launch. This drives ruby/spec's `ruby_version_is` guards and gems' version checks. |
| `RUBY_PLATFORM` | the host triple as RubyGems spells it, e.g. `"arm64-darwin24"` |
| `RUBY_PATCHLEVEL` | `-1` |
| `RUBY_RELEASE_DATE`, `RUBY_COPYRIGHT`, `RUBY_DESCRIPTION` | Spinel's |

Ruby 4.0's `Ruby` module holds the same objects (`Ruby::VERSION.equal?(RUBY_VERSION)`). Like CRuby, Spinel fixes `RUBY_PLATFORM` when it is built (`crates/spinel-vm/build.rs`), and `RbConfig::CONFIG["arch"]` repeats it, which is what `Gem::Platform.local` parses.

Reporting `"spinel"` rather than `"ruby"` is honest and is what JRuby and TruffleRuby do. The cost: Bundler's `platforms :ruby` / `:mri` groups are defined as a list of known engines, and unknown engines are excluded from those groups. Two mitigations, both required: Spinel's own Gemfile evaluator treats `:ruby` as matching, and we upstream a one-line Bundler change adding `spinel` to `Bundler::CurrentRuby` so `require "bundler/setup"` inside apps behaves the same. Until that lands, `bundler/setup` on Spinel logs a warning naming any gem it skipped for platform reasons.

**Language version.** Spinel implements one Ruby language version at a time, the latest stable, and follows Ruby's December releases. `ruby "3.3"` in a Gemfile and a `.ruby-version` that names a CRuby version are read and produce a warning, never an error, because the intent is "this app expects at least that Ruby". `--strict-ruby-version` turns the warning into an error for CI.

- Who reads them: `spinel run` and `spinel <file>`, before the program starts. Only `run` takes `--strict-ruby-version`, which exits 1 without running anything. `ruby`'s command line (`spinel -e`, `spinel -w app.rb`) reads neither, as `ruby` does not.
- Where: the nearest `.ruby-version` and the nearest `Gemfile`, each looked for from the current directory upward. Each request that is not met is one line on stderr naming the file.
- `.ruby-version`, first word: `spinel-<version>` (what `spinel init` writes) is met by this Spinel or a later one; `<version>` or `ruby-<version>` is a CRuby version, met when `RUBY_VERSION` is that version or later; bare `spinel` or `ruby` is always met. Any other word, such as `jruby-9.4` or `ruby-head`, is not.
- Versions compare as RubyGems compares them (`Gem::Version`): a prerelease such as `4.0.0-preview1` or `4.0.0.rc1` comes before `4.0.0`. `3.3.0p0` is a prerelease of `3.3.0` the same way. A `.ruby-version` word RubyGems would reject, as version managers write them (`2.7.8_1`, `ruby-3.3.0@gemset`), is read by its leading numbers (`2.7.8`, `3.3.0`); only the Gemfile's `ruby` line follows Bundler in rejecting it, so such a requirement there is not met.
- Gemfile: the quoted requirements of a `ruby` line, each read as a minimum, so `ruby "~> 3.3.0"` and `ruby ">= 3.1", "< 5"` are met by Ruby 4.0. Upper bounds and `!=` are passed over, and keyword options (`engine:`, `file:`) are not read. The Gemfile is scanned line by line, not evaluated, so a version computed at runtime is not seen.

## `--dump-bytecode`

`spinel run --dump-bytecode file.rb` is to the compiler what `spinel parse` is to the tree. It compiles the file and prints its `Iseq` in Rust's `Debug` form, without running it: the instructions, one per `pc`, with the literal and symbol pools, the locals, the call sites and the nested bodies (`children`). A file that does not parse reports its syntax errors and exits with the status `spinel run` gives them.

The format is the instruction set's, and it changes when the instruction set changes; `CHANGELOG.md` records each change. A superinstruction (engine.md, Bytecode) prints as a struct named for the instructions it stands for, with their operands as fields:

```
GetLocalPushIntBinOp {
    slot: 0,
    int: 2,
    op: Lt,
},
PushInt(
    2,
),
BinOp(
    Lt,
),
JumpUnless(
    3,
),
```

The instructions after it are the ones it stands for, still in place. A jump into the middle of the run lands on them, and they are what runs when the superinstruction declines. Each line is one `pc`: read the stream with nothing skipped, and a jump's displacement counts every line.

## `--watch`

`spinel run --watch app.rb [args]` runs the program, and runs it again whenever the project changes. `spinel test --watch` is to do the same once the test runner exists, with #97 choosing the tests a change affects; the supervisor, `crates/spinel-cli/src/watch.rs`, takes any `spinel` command line to restart.

- **Watched:** the current directory and everything under it, through the `notify` crate (inotify on Linux, FSEvents on macOS). Run it from the project's root.
- **A change:** a file or directory created, written, renamed or removed, or its metadata changed. Reading is not, as every run reads the program.
- **Not a change:** a path `.gitignore` excludes, as git decides it: the `.gitignore` files in the directory and its subdirectories, the nearest first, then `.git/info/exclude`, then the user's global excludes file (`core.excludesFile`, else `~/.config/git/ignore`). A path under an excluded directory stays excluded, `.git/` never counts, and an edited `.gitignore` is read again. A `.gitignore` above the current directory is not read.
- **Debounce:** a restart waits until 100 ms pass with no further change, so one save an editor writes in several steps restarts the program once.
- **Restart:** the program runs as a child `spinel run`, without `--watch`, in a process group of its own. A restart sends that group SIGTERM, then SIGKILL to whatever is left after two seconds, and starts a fresh child; nothing is reloaded in place, as Zeitwerk and similar already do that inside the app. Each restart is one line on stderr naming what changed, such as `spinel: restarting: config/settings.rb changed`. The child reads `.ruby-version` and the Gemfile itself, so each run reports a mismatch afresh.
- **A program that ends** on its own is reported, as in `spinel: the program exited with status 1; waiting for a change`, and the next change starts it again.
- **Ctrl-C**, SIGTERM or SIGHUP end the program's group the same way, then `spinel`, which exits with 128 plus the signal's number: 130 for Ctrl-C.
- **Not yet:** the program's group is not the terminal's foreground group, so a program that reads from the terminal is stopped (SIGTTIN) rather than handed it; standard input from a pipe or a file works. One recursive watch covers ignored directories too, whose events are then dropped, so a very large ignored tree can reach the inotify watch limit.

## Bundler running on Spinel

Apps call `require "bundler/setup"` in their own boot files, so Bundler the gem must run on Spinel and read Spinel's `.spinel/gem_home` and `Gemfile.lock`. Bundler is pure Ruby, and the GEM_HOME layout is standard, so this is a compatibility test rather than new code: `crates/spinel-cli/tests/fixtures/bundler-setup` boots an app through `bundler/setup` with `BUNDLE_PATH` pointed at `.spinel/gem_home`. `spinel install` writes `.bundle/config` with that `BUNDLE_PATH` so plain Bundler commands in the same project agree with Spinel.
