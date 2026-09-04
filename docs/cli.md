# CLI and runtime plumbing

What `spinel` does outside the engine: how it runs a file, how it reports itself to Ruby code, and how the small Ruby shims glue the tooling to the VM.

## Subcommands

| command | does |
|---|---|
| `spinel run <file> [args]` | run a file on the VM with the project's gems visible |
| `spinel <file.rb>` | same as `run`. Subcommand names win: `spinel install` is the subcommand even if `install` exists as a file; use `spinel run install` or `spinel ./install` for the file. |
| `spinel -e CODE`, `-r LIB`, `-I DIR`, `-w`, `-v` | same meaning as `ruby`'s flags so scripts and gems that shell out keep working |
| `spinel x <gem>[@req] [args]` | run a gem's executable from an ephemeral GEM_HOME under `~/.spinel/x/`, project untouched |
| `spinel init` | Gemfile, `.ruby-version` containing `spinel-<version>`, `test/`, `.gitignore` entry for `.spinel/` |
| `spinel install/add/remove/update` | package-manager.md |
| `spinel test` | test-runner.md |
| `spinel build` | build.md |
| `spinel parse <file>` | print the `spinel_ast` tree for one file; `--format debug` for the derived `Debug`. Exits 1 on a syntax error, 2 if the file cannot be read. |
| `spinel parse <dir>` | lower every `.rb` file under a directory and report only what failed. Exits non-zero on an unhandled node, never on a syntax error, because a corpus may legitimately contain invalid Ruby. This is the check behind the `sweep` CI job. |
| `spinel spec [dir]` | run ruby/spec (development only, hidden from `--help` in release builds) |

## What `spinel run` sets up

Before user code, the VM runs the embedded `shims/setup.rb`:

1. Walk up from the current directory to find `.spinel/gem_home/`. If found, set `Gem.paths` to it and prepend its `bin/` to `PATH` for child processes.
2. Load `.spinel/require_map.bin` if present (see below).
3. Register the bytecode cache directory.

Bin stubs in `.spinel/gem_home/bin/` start with `#!/usr/bin/env spinel` and call `Gem.activate_bin_path`, the same shape RubyGems generates. `Gem.ruby` returns the absolute path of the running `spinel` binary so anything that spawns "the current Ruby" spawns Spinel.

## Bytecode cache and require map

- `.spinel/bytecode/<sha256 of source>.sbc` holds compiled bytecode. `require` checks it before parsing. The format is position-independent: symbols are stored by name and relinked on load, so a cache file is valid across processes and Ractors.
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

Reporting `"spinel"` rather than `"ruby"` is honest and is what JRuby and TruffleRuby do. The cost: Bundler's `platforms :ruby` / `:mri` groups are defined as a list of known engines, and unknown engines are excluded from those groups. Two mitigations, both required: Spinel's own Gemfile evaluator treats `:ruby` as matching, and we upstream a one-line Bundler change adding `spinel` to `Bundler::CurrentRuby` so `require "bundler/setup"` inside apps behaves the same. Until that lands, `bundler/setup` on Spinel logs a warning naming any gem it skipped for platform reasons.

**Language version.** Spinel implements one Ruby language version at a time, the latest stable, and follows Ruby's December releases. `ruby "3.3"` in a Gemfile and a `.ruby-version` that names a CRuby version are read and produce a warning, never an error, because the intent is "this app expects at least that Ruby". `--strict-ruby-version` turns the warning into an error for CI.

## `--watch`

`spinel run --watch app.rb` and `spinel test --watch` use the `notify` crate on the project directory, honor `.gitignore`, debounce 100 ms, and restart the child process. There is no in-process reload; Zeitwerk and similar already do that inside the app.

## Bundler running on Spinel

Apps call `require "bundler/setup"` in their own boot files, so Bundler the gem must run on Spinel and read Spinel's `.spinel/gem_home` and `Gemfile.lock`. Bundler is pure Ruby, and the GEM_HOME layout is standard, so this is a compatibility test rather than new code: `crates/spinel-cli/tests/fixtures/bundler-setup` boots an app through `bundler/setup` with `BUNDLE_PATH` pointed at `.spinel/gem_home`. `spinel install` writes `.bundle/config` with that `BUNDLE_PATH` so plain Bundler commands in the same project agree with Spinel.
